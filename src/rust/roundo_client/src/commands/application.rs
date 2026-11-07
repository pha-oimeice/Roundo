//! Main-world capability adapter for the complete client command catalog.
//!
//! The dispatcher is rebuilt over short-lived borrows for each request. This
//! keeps command definitions source-neutral while allowing handlers to mutate
//! non-`Send` Bevy/WebView state only on the owning thread.

use super::*;

/// All host capabilities available during one synchronous command dispatch.
///
/// Consuming this value bounds every borrowed capability to one request and
/// prevents handlers from retaining main-world references.
pub(super) struct ClientCommandApplication<'a> {
    pub(super) config: &'a mut ClientConfigStore,
    pub(super) network: &'a mut crate::network::ClientNetworkManager,
    pub(super) dev_level: &'a mut DevLevel,
    pub(super) camera_position: Option<[f32; 3]>,
    pub(super) webui: Option<&'a mut roundo_webui::UiLifecycleManager>,
    pub(super) navigation: Option<&'a mut roundo_webui::UiNavigationExecutor>,
    pub(super) scene: Option<&'a mut crate::scene::ClientSceneManager>,
    pub(super) hud: &'a HudCache,
    pub(super) input_registry: &'a roundo_marionette::InputFunctionRegistry,
}

/// Request-local host effects, never reconstructed from command names or JSON.
#[derive(Default)]
enum CommandEffect {
    #[default]
    Respond,
    Opening(roundo_webui::UiOpening),
    Quit,
}

impl ClientCommandApplication<'_> {
    /// Admits and executes one request, then owns its response settlement.
    ///
    /// An accepted Opening transfers this exact sender to document lifetime;
    /// all other responses are settled here. The return value requests AppExit
    /// only after an accepted Quit response has been attempted. Callers do not
    /// interpret command names, output fields, or navigation ordering.
    pub(super) fn execute(
        self,
        request: serde_json::Value,
        ui_source: Option<roundo_webui::UiInstanceId>,
        reply: roundo_toolbox::request_response_pipe::ResponseSender<serde_json::Value>,
    ) -> bool {
        let command_name = request["command"].as_str().unwrap_or_default().to_owned();
        log::debug!("Executing client command `{command_name}`: {request}");
        let Self {
            config,
            network,
            dev_level,
            camera_position,
            webui,
            navigation,
            scene,
            hud,
            input_registry,
        } = self;

        // Admission is deliberately outside individual handlers: every command
        // arriving from a WebView must prove that its host-bound source endpoint
        // is still committed, loaded, enabled, and not destroying before any
        // business capability can accept it. Visibility and focus are irrelevant.
        if let Some(source) = ui_source {
            let admitted = webui
                .as_deref()
                .is_some_and(|manager| manager.command_source_is_live(source));
            if !admitted {
                let _ = reply.respond(stale_ui_source_response(&request));
                return false;
            }
        }

        let config = RefCell::new(config);
        let network = RefCell::new(network);
        let dev_level = RefCell::new(dev_level);
        let webui = RefCell::new(webui);
        let navigation = RefCell::new(navigation);
        let scene = RefCell::new(scene);
        let execute_scene_intent = |intent, effect: &mut CommandEffect| {
            let mut scene = scene.borrow_mut();
            let scene = scene.as_deref_mut().ok_or_else(scene_unavailable)?;
            let mut webui = webui.borrow_mut();
            let ui = webui.as_deref_mut().ok_or_else(scene_unavailable)?;
            let mut navigation = navigation.borrow_mut();
            let navigation = navigation.as_deref_mut().ok_or_else(scene_unavailable)?;
            let outcome = scene
                .execute(intent, &network.borrow().status().status, ui, navigation)
                .map_err(scene_transition_error)?;
            match outcome {
                crate::scene::ClientUiOutcome::Opening { opening, resource } => {
                    *effect = CommandEffect::Opening(opening);
                    Ok(UiOpenOutput { resource })
                }
                crate::scene::ClientUiOutcome::Completed => Ok(UiOpenOutput {
                    resource: String::new(),
                }),
            }
        };
        let mut registry = roundo_cli::json_command::CommandRegistry::<CommandEffect>::default();

        // Connection handlers delegate ownership and status transitions to the
        // network manager; successful start is asynchronous, not peer admission.
        registry.register_typed::<ServerConnectDefinition>(|input, _| {
            let config = config.borrow();
            let mut network = network.borrow_mut();
            let server = config.server(input.index).map_err(server_list_error)?;
            let connection = network.start_connection(server);
            connection.map_err(|error| {
                roundo_cli::json_command::CommandError::new("connection_failed", error)
            })?;
            Ok(server_status_output(network.status()))
        });
        registry.register_typed::<ServerRetryDefinition>(|_, _| {
            let mut network = network.borrow_mut();
            network.retry().map_err(|error| {
                roundo_cli::json_command::CommandError::new("no_active_server", error)
            })?;
            Ok(server_status_output(network.status()))
        });
        registry.register_typed::<ControllerRetryAcquireDefinition>(|input, _| {
            if input.controller_id.is_empty()
                || !input
                    .controller_id
                    .bytes()
                    .all(|byte| byte.is_ascii_digit())
            {
                return Err(roundo_cli::json_command::CommandError::new(
                    "invalid_arguments",
                    "controller_id must be a non-empty ASCII decimal integer",
                ));
            }
            let controller_id = input.controller_id.parse::<u64>().map_err(|_| {
                roundo_cli::json_command::CommandError::new(
                    "invalid_arguments",
                    "controller_id must be an unsigned decimal integer",
                )
            })?;
            let network = network.borrow();
            let request = network
                .retry_failed_acquire(roundo_contracts::ControllerId(controller_id))
                .map_err(control_retry_error)?;
            Ok(ControllerRetryAcquireOutput {
                controller_id: request.controller_id.0.to_string(),
                request_id: request.id.0.to_string(),
                state: "pending".into(),
            })
        });
        registry.register_typed::<ServerDisconnectDefinition>(|_, _| {
            let mut network = network.borrow_mut();
            network.disconnect();
            Ok(server_status_output(network.status()))
        });
        registry.register_typed::<ServerStatusDefinition>(|_, _| {
            Ok(server_status_output(network.borrow().status()))
        });
        registry.register_typed::<ServerRefreshDefinition>(|_, _| {
            Ok(ServerRefreshOutput {
                accepted: true,
                revision: config.borrow().refresh_servers(),
            })
        });
        registry.register_typed::<ServerListDefinition>(|_, _| {
            Ok(config
                .borrow()
                .server_list_snapshot(None)
                .expect("unconditional snapshot")
                .data)
        });
        // The model owns validation, in-memory changes, probe invalidation and
        // persistence. Save errors still report failure without rolling back memory.
        registry.register_typed::<ServerAddDefinition>(|input, _| {
            let index = config
                .borrow_mut()
                .add_server(input.into())
                .map_err(server_list_error)?;
            Ok(ServerIndexOutput { index })
        });
        registry.register_typed::<ServerEditDefinition>(|input, _| {
            config
                .borrow_mut()
                .edit_server(input.index, input.server.into())
                .map_err(server_list_error)?;
            Ok(ServerIndexOutput { index: input.index })
        });
        registry.register_typed::<ServerDeleteDefinition>(|input, _| {
            config
                .borrow_mut()
                .delete_server(input.index)
                .map_err(server_list_error)?;
            Ok(EmptyOutput {})
        });
        // Settings mutation follows the same write-through, non-rollback model:
        // normalize memory first, then attempt persistence.
        registry.register_typed::<SettingsShowDefinition>(|_, _| {
            Ok(settings_show_output(&config.borrow()))
        });
        registry.register_typed::<SettingsSetDefinition>(|input, _| {
            let mut config = config.borrow_mut();
            let settings = config.settings_mut();
            match input.key.as_str() {
                "controls.mouse_sensitivity" => settings.controls.mouse_sensitivity = input.value,
                "camera.move_speed" => settings.camera.move_speed = input.value,
                "camera.voxel_raycast_distance" => {
                    settings.camera.voxel_raycast_distance = input.value
                }
                "world.joinable_world_radius" => settings.world.joinable_world_radius = input.value,
                "world.chunk_view_distance" => settings.world.chunk_view_distance = input.value,
                "world.geometry_lod_pixel_error" => {
                    if !input.value.is_finite() || input.value < 0.0 {
                        return Err(roundo_cli::json_command::CommandError::new(
                            "invalid_arguments",
                            "geometry pixel error must be finite and nonnegative",
                        ));
                    }
                    settings.world.geometry_lod_pixel_error = input.value;
                }
                _ => {
                    return Err(roundo_cli::json_command::CommandError::new(
                        "invalid_arguments",
                        "unknown setting key or value",
                    ));
                }
            }
            settings.normalize();
            config.save_setting(&input.key).map_err(|error| {
                roundo_cli::json_command::CommandError::new("config_save_failed", error)
            })?;
            Ok(EmptyOutput {})
        });
        // Shortcut edits validate the whole graph and persist before publication.
        registry.register_typed::<FunctionsListDefinition>(|_, _| {
            Ok(functions_list_output(input_registry))
        });
        registry
            .register_typed::<SlotsListDefinition>(|_, _| Ok(slots_list_output(&config.borrow())));
        registry.register_typed::<SlotsCreateDefinition>(|input, _| {
            edit_input_configuration(&mut config.borrow_mut(), input_registry, |settings| {
                if settings.input_slots.contains_key(&input.slot) {
                    return Err(shortcut_error(
                        "slot_exists",
                        "shortcut slot already exists",
                    ));
                }
                settings.input_slots.insert(
                    input.slot,
                    ClientInputSlotConfig {
                        display_name: input.display_name,
                        functions: Default::default(),
                    },
                );
                Ok(())
            })
        });
        registry.register_typed::<SlotsRenameDefinition>(|input, _| {
            edit_input_configuration(&mut config.borrow_mut(), input_registry, |settings| {
                let slot = settings.input_slots.get_mut(&input.slot).ok_or_else(|| {
                    shortcut_error("unknown_input_slot", "shortcut slot does not exist")
                })?;
                slot.display_name = input.display_name;
                Ok(())
            })
        });
        registry.register_typed::<SlotsDeleteDefinition>(|input, _| {
            edit_input_configuration(&mut config.borrow_mut(), input_registry, |settings| {
                let slot = settings.input_slots.get(&input.slot).ok_or_else(|| {
                    shortcut_error("unknown_input_slot", "shortcut slot does not exist")
                })?;
                if !slot.functions.is_empty()
                    || settings
                        .input_bindings
                        .iter()
                        .any(|binding| binding.slot == input.slot)
                {
                    return Err(shortcut_error(
                        "slot_in_use",
                        "remove key bindings and function assignments before deleting the slot",
                    ));
                }
                settings.input_slots.remove(&input.slot);
                Ok(())
            })
        });
        registry.register_typed::<SlotsAssignDefinition>(|input, _| {
            edit_input_configuration(&mut config.borrow_mut(), input_registry, |settings| {
                let slot = settings.input_slots.get_mut(&input.slot).ok_or_else(|| {
                    shortcut_error("unknown_input_slot", "shortcut slot does not exist")
                })?;
                if !slot.functions.insert(input.function) {
                    return Err(shortcut_error(
                        "assignment_exists",
                        "function is already assigned to this slot",
                    ));
                }
                Ok(())
            })
        });
        registry.register_typed::<SlotsUnassignDefinition>(|input, _| {
            edit_input_configuration(&mut config.borrow_mut(), input_registry, |settings| {
                let slot = settings.input_slots.get_mut(&input.slot).ok_or_else(|| {
                    shortcut_error("unknown_input_slot", "shortcut slot does not exist")
                })?;
                if !slot.functions.remove(&input.function) {
                    return Err(shortcut_error(
                        "assignment_not_found",
                        "function is not assigned to this slot",
                    ));
                }
                Ok(())
            })
        });
        registry.register_typed::<BindingsListDefinition>(|_, _| {
            Ok(bindings_list_output(&config.borrow(), input_registry))
        });
        registry.register_typed::<BindingsBindDefinition>(|input, _| {
            let binding = ClientInputBindingConfig::try_from(input)?;
            edit_input_configuration(&mut config.borrow_mut(), input_registry, |settings| {
                if settings.input_bindings.contains(&binding) {
                    return Err(shortcut_error("binding_exists", "binding already exists"));
                }
                settings.input_bindings.push(binding);
                Ok(())
            })
        });
        registry.register_typed::<BindingsUnbindDefinition>(|input, _| {
            let binding = ClientInputBindingConfig::try_from(input)?;
            edit_input_configuration(&mut config.borrow_mut(), input_registry, |settings| {
                if !settings.input_bindings.contains(&binding) {
                    return Err(shortcut_error(
                        "binding_not_found",
                        "binding does not exist",
                    ));
                }
                settings
                    .input_bindings
                    .retain(|existing| *existing != binding);
                Ok(())
            })
        });
        registry.register_typed::<BindingsReplaceDefinition>(|input, _| {
            let old_binding = ClientInputBindingConfig::try_from(input.old_binding)?;
            let replacement = ClientInputBindingConfig::try_from(input.binding)?;
            edit_input_configuration(&mut config.borrow_mut(), input_registry, |settings| {
                replace_binding(&mut settings.input_bindings, old_binding, replacement)
            })?;
            Ok(BindingMutationOutput {})
        });
        // Catalog metadata is generated from the same typed definitions used by dispatch.
        registry.register_typed::<CommandSchemaDefinition>(|input, _| {
            CLIENT_COMMAND_CATALOG.schema(&input.command).map(|schema| {
                CommandSchemaOutput(schema.as_object().cloned().expect("schema is an object"))
            })
        });
        registry.register_typed::<CommandHelpDefinition>(|input, _| match input.command {
            None => Ok(CommandHelpOutput::Commands {
                commands: CLIENT_COMMAND_CATALOG.visible(dev_level.borrow().0),
            }),
            Some(command) => match CLIENT_COMMAND_CATALOG.dev_level(&command) {
                Some(required_level) => Ok(CommandHelpOutput::Command {
                    command,
                    dev_level: required_level,
                }),
                None => Err(roundo_cli::json_command::CommandError::new(
                    "unknown_command",
                    "unknown command",
                )),
            },
        });
        registry.register_typed::<DevDefinition>(|input, _| {
            let mut dev_level = dev_level.borrow_mut();
            if let Some(level) = input.level {
                if level > 3 {
                    return Err(roundo_cli::json_command::CommandError::new(
                        "invalid_arguments",
                        "level must be 0 through 3",
                    ));
                }
                dev_level.0 = level;
            }
            Ok(DevOutput { level: dev_level.0 })
        });
        // Diagnostics consume snapshots captured before dispatch and never borrow ECS here.
        registry.register_typed::<HudShowDefinition>(|_, _| {
            hud_show_output(hud).map_err(|error| {
                roundo_cli::json_command::CommandError::new(
                    "internal_command_error",
                    error.to_string(),
                )
            })
        });
        registry.register_typed::<DiagnosticsPositionDefinition>(|_, _| {
            let [x, y, z] = camera_position.ok_or_else(|| {
                roundo_cli::json_command::CommandError::new(
                    "camera_unavailable",
                    "player camera is unavailable",
                )
            })?;
            Ok(PositionOutput { x, y, z })
        });
        registry.register_typed::<AppQuitDefinition>(|_, effect| {
            *effect = CommandEffect::Quit;
            Ok(AcceptedOutput {
                status: "accepted".into(),
            })
        });
        // External navigation validates scheme/shape before crossing the OS adapter seam.
        registry.register_typed::<OpenExternalUrlDefinition>(|input, _| {
            let url = validate_external_url(&input.url)?;
            open_external_url(url).map_err(|error| {
                roundo_cli::json_command::CommandError::new(
                    "external_url_open_failed",
                    error.to_string(),
                )
            })?;
            Ok(AcceptedOutput {
                status: "accepted".into(),
            })
        });
        // Scene intents are source-neutral game logic, not UI imports or focus effects.
        registry.register_typed::<ScenePauseDefinition>(|_, effect| {
            execute_scene_intent(crate::scene::ClientUiIntent::Pause, effect)
                .map(|_| EmptyOutput {})
        });
        registry.register_typed::<SceneResumeDefinition>(|_, effect| {
            execute_scene_intent(crate::scene::ClientUiIntent::Resume, effect)
                .map(|_| EmptyOutput {})
        });
        // Web UI navigation requires the already-admitted live source and both
        // logical lifecycle and platform navigation capabilities.
        registry.register_typed::<UiBackDefinition>(|_, _| {
            let source = ui_source.ok_or_else(|| {
                roundo_cli::json_command::CommandError::new(
                    "stale_ui_instance",
                    "ui.back requires a live WebView source",
                )
            })?;
            let mut webui = webui.borrow_mut();
            let manager = webui.as_deref_mut().ok_or_else(|| {
                roundo_cli::json_command::CommandError::new(
                    "stale_ui_instance",
                    "Web UI is unavailable",
                )
            })?;
            let mut navigation = navigation.borrow_mut();
            let navigation = navigation.as_deref_mut().ok_or_else(|| {
                roundo_cli::json_command::CommandError::new(
                    "ui_navigation_failed",
                    "Web UI navigation is unavailable",
                )
            })?;
            navigation
                .back(manager, source)
                .map_err(map_ui_lifecycle_error)?;
            Ok(EmptyOutput {})
        });
        registry.register_typed::<DebugRequestDefinition>(|input, _| {
            network
                .borrow()
                .submit_debug(input.operation)
                .map(|request_id| DebugAcceptedOutput { request_id })
                .map_err(|error| {
                    roundo_cli::json_command::CommandError::new("debug_not_submitted", error)
                })
        });
        registry.register_typed::<UiOpenDefinition>(|input, effect| {
            let source = ui_source.ok_or_else(|| {
                roundo_cli::json_command::CommandError::new(
                    "stale_ui_instance",
                    "ui.open requires a live WebView source",
                )
            })?;
            let mut webui = webui.borrow_mut();
            let state = webui.as_deref_mut().ok_or_else(|| {
                roundo_cli::json_command::CommandError::new(
                    "invalid_ui_resource",
                    "Web UI is unavailable",
                )
            })?;
            let target = state
                .resolve_import(source, &input.import)
                .map_err(map_ui_lifecycle_error)?;
            let resource = target.resource().to_owned();
            log::debug!("Resolved Web UI navigation target `{resource}`");
            let mut navigation = navigation.borrow_mut();
            let navigation = navigation.as_deref_mut().ok_or_else(|| {
                roundo_cli::json_command::CommandError::new(
                    "ui_navigation_failed",
                    "Web UI navigation is unavailable",
                )
            })?;
            let opened = navigation.navigate(
                state,
                roundo_webui::UiCommandSource::WebView(source),
                target,
            );
            let opening = opened.map_err(|error| {
                log::error!("Web UI navigation to `{resource}` failed: {error}");
                map_ui_lifecycle_error(error)
            })?;
            *effect = CommandEffect::Opening(opening);
            log::info!("Web UI navigation accepted for `{resource}`");
            Ok(UiOpenOutput { resource })
        });
        // Fail fast if declaration/catalog and executable registration drift.
        CLIENT_COMMAND_CATALOG.assert_complete(&registry);
        let mut effect = CommandEffect::default();
        let response = registry.dispatch_value(request, &mut effect);
        drop(registry);
        let command_succeeded = response["ok"] == true;
        log::debug!("Client command `{command_name}` dispatched: {response}");
        let mut quit_accepted = false;
        if command_succeeded {
            match effect {
                CommandEffect::Opening(opening) => {
                    let completed = navigation
                        .borrow_mut()
                        .as_deref_mut()
                        .expect("an accepted opening has its navigation executor")
                        .complete_open_command(opening, reply, response);
                    assert!(
                        completed.is_ok(),
                        "a fresh exact opening accepts its response"
                    );
                    return false;
                }
                CommandEffect::Quit => quit_accepted = true,
                CommandEffect::Respond => {}
            }
        }
        let source_was_destroyed = ui_source.is_some_and(|source| {
            !webui
                .borrow()
                .as_deref()
                .is_some_and(|manager| manager.command_source_is_live(source))
        });
        if reply.respond(response).is_err() {
            if command_succeeded && source_was_destroyed {
                log::debug!(
                    "Client command `{command_name}` destroyed its source before response delivery"
                );
            } else {
                log::warn!("Client command response receiver disconnected for `{command_name}`");
            }
        }
        quit_accepted
    }
}

fn shortcut_error(code: &str, message: &str) -> roundo_cli::json_command::CommandError {
    roundo_cli::json_command::CommandError::new(code, message)
}

fn scene_unavailable() -> roundo_cli::json_command::CommandError {
    roundo_cli::json_command::CommandError::new(
        "scene_unavailable",
        "Scene presentation is unavailable",
    )
}

fn scene_transition_error(
    error: crate::scene::SceneTransitionError,
) -> roundo_cli::json_command::CommandError {
    match error {
        crate::scene::SceneTransitionError::Ui(error) => map_ui_lifecycle_error(error),
        error => roundo_cli::json_command::CommandError::new(
            "scene_transition_rejected",
            error.to_string(),
        ),
    }
}

fn server_list_error(
    error: crate::server_list::ServerListError,
) -> roundo_cli::json_command::CommandError {
    use crate::server_list::ServerListError;
    let (code, message) = match error {
        ServerListError::InvalidServer(message) => ("invalid_arguments", message.to_owned()),
        ServerListError::IndexOutOfRange => {
            ("index_out_of_range", "server index does not exist".into())
        }
        ServerListError::Save(message) => ("config_save_failed", message),
    };
    roundo_cli::json_command::CommandError::new(code, message)
}
