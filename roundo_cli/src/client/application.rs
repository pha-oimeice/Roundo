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
    pub(super) network: &'a mut crate::client_network::ClientNetworkManager,
    pub(super) probes: &'a crate::client_network::ServerProbeManager,
    pub(super) dev_level: &'a mut DevLevel,
    pub(super) camera_position: Option<[f32; 3]>,
    pub(super) webui: Option<&'a mut roundo_webui::UiLifecycleManager>,
    pub(super) navigation: Option<&'a mut roundo_webui::UiNavigationExecutor>,
    pub(super) hud: &'a HudCache,
    pub(super) input_registry: Option<&'a roundo_marionette::InputRegistry>,
}

impl ClientCommandApplication<'_> {
    /// Executes one source-neutral command through the complete client
    /// capability catalog. Admission, state access, adapter calls, and result
    /// formation remain local to this implementation.
    pub(super) fn dispatch(
        self,
        request: serde_json::Value,
        ui_source: Option<roundo_webui::UiInstanceId>,
    ) -> serde_json::Value {
        let Self {
            config,
            network,
            probes,
            dev_level,
            camera_position,
            webui,
            navigation,
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
                return stale_ui_source_response(&request);
            }
        }

        let config = RefCell::new(config);
        let network = RefCell::new(network);
        let dev_level = RefCell::new(dev_level);
        let webui = RefCell::new(webui);
        let navigation = RefCell::new(navigation);
        let mut registry = crate::json_command::CommandRegistry::default();

        // Connection handlers delegate ownership and status transitions to the
        // network manager; successful start is asynchronous, not peer admission.
        registry.register_typed::<ServerConnectDefinition>(|input, _| {
            let config = config.borrow();
            let mut network = network.borrow_mut();
            let server = config.0.servers.get(input.index).ok_or_else(|| {
                crate::json_command::CommandError::new(
                    "index_out_of_range",
                    "server index does not exist",
                )
            })?;
            let connection = network.start_connection(server);
            connection.map_err(|error| {
                crate::json_command::CommandError::new("connection_failed", error)
            })?;
            Ok(server_status_output(network.status()))
        });
        registry.register_typed::<ServerRetryDefinition>(|_, _| {
            let mut network = network.borrow_mut();
            network.retry().map_err(|error| {
                crate::json_command::CommandError::new("no_active_server", error)
            })?;
            Ok(server_status_output(network.status()))
        });
        registry.register_typed::<ServerDisconnectDefinition>(|_, _| {
            let mut network = network.borrow_mut();
            network.disconnect();
            Ok(server_status_output(network.status()))
        });
        registry.register_typed::<ServerStatusDefinition>(|_, _| {
            Ok(server_status_output(network.borrow().status()))
        });
        // Discovery probes replace a cache batch and return before network I/O completes.
        registry.register_typed::<ServerRefreshDefinition>(|_, _| {
            let config = config.borrow();
            let revision = probes.refresh(&config.0.servers, &config.0.network);
            Ok(ServerRefreshOutput {
                accepted: true,
                revision,
            })
        });
        registry.register_typed::<ServerListDefinition>(|_, _| {
            Ok(server_list_output(&config.borrow(), probes))
        });
        // These mutations update in-memory configuration before saving. A save
        // error is reported but does not roll back the in-memory server list.
        registry.register_typed::<ServerAddDefinition>(|input, _| {
            let server = ServerEntry::from(input);
            if server.name.trim().is_empty() || server.address.trim().is_empty() {
                return Err(crate::json_command::CommandError::new(
                    "invalid_arguments",
                    "name and address are required",
                ));
            }
            let mut config = config.borrow_mut();
            config.0.servers.push(server);
            config.save().map_err(|error| {
                crate::json_command::CommandError::new("config_save_failed", error)
            })?;
            probes.invalidate();
            Ok(ServerIndexOutput {
                index: config.0.servers.len() - 1,
            })
        });
        registry.register_typed::<ServerEditDefinition>(|input, _| {
            let server = ServerEntry::from(input.server);
            if server.name.trim().is_empty() || server.address.trim().is_empty() {
                return Err(crate::json_command::CommandError::new(
                    "invalid_arguments",
                    "index and a complete server are required",
                ));
            }
            let mut config = config.borrow_mut();
            if input.index >= config.0.servers.len() {
                return Err(crate::json_command::CommandError::new(
                    "index_out_of_range",
                    "server index does not exist",
                ));
            }
            config.0.servers[input.index] = server;
            config.save().map_err(|error| {
                crate::json_command::CommandError::new("config_save_failed", error)
            })?;
            probes.invalidate();
            Ok(ServerIndexOutput { index: input.index })
        });
        registry.register_typed::<ServerDeleteDefinition>(|input, _| {
            let mut config = config.borrow_mut();
            if input.index >= config.0.servers.len() {
                return Err(crate::json_command::CommandError::new(
                    "index_out_of_range",
                    "server index does not exist",
                ));
            }
            let _removed_server = config.0.servers.remove(input.index);
            config.save().map_err(|error| {
                crate::json_command::CommandError::new("config_save_failed", error)
            })?;
            probes.invalidate();
            Ok(EmptyOutput {})
        });
        // Settings mutation follows the same write-through, non-rollback model:
        // normalize memory first, then attempt persistence.
        registry.register_typed::<SettingsShowDefinition>(|_, _| {
            Ok(settings_show_output(&config.borrow()))
        });
        registry.register_typed::<SettingsSetDefinition>(|input, _| {
            let mut config = config.borrow_mut();
            match input.key.as_str() {
                "controls.mouse_sensitivity" => {
                    config.0.settings.controls.mouse_sensitivity = input.value
                }
                "camera.move_speed" => config.0.settings.camera.move_speed = input.value,
                "camera.voxel_raycast_distance" => {
                    config.0.settings.camera.voxel_raycast_distance = input.value
                }
                "world.joinable_world_radius" => {
                    config.0.settings.world.joinable_world_radius = input.value
                }
                "world.chunk_view_distance" => {
                    config.0.settings.world.chunk_view_distance = input.value
                }
                _ => {
                    return Err(crate::json_command::CommandError::new(
                        "invalid_arguments",
                        "unknown setting key or value",
                    ));
                }
            }
            config.0.settings.normalize();
            config.save().map_err(|error| {
                crate::json_command::CommandError::new("config_save_failed", error)
            })?;
            Ok(EmptyOutput {})
        });
        // Incremental bind/unbind handlers mutate memory before persistence.
        // Replacement instead stages a cloned document and publishes it in memory
        // only after the file write succeeds.
        registry.register_typed::<BindingsListDefinition>(|_, _| {
            Ok(bindings_list_output(&config.borrow(), input_registry))
        });
        registry.register_typed::<BindingsBindDefinition>(|input, _| {
            let binding = ClientInputBindingConfig::try_from(input)?;
            validate_binding_slot(&binding, input_registry)?;
            let mut config = config.borrow_mut();
            if config.0.settings.input_bindings.contains(&binding) {
                return Err(crate::json_command::CommandError::new(
                    "binding_exists",
                    "binding already exists",
                ));
            }
            config.0.settings.input_bindings.push(binding);
            config.save().map_err(|error| {
                crate::json_command::CommandError::new("config_save_failed", error)
            })?;
            Ok(EmptyOutput {})
        });
        registry.register_typed::<BindingsUnbindDefinition>(|input, _| {
            let binding = ClientInputBindingConfig::try_from(input)?;
            let mut config = config.borrow_mut();
            let Some(index) = config
                .0
                .settings
                .input_bindings
                .iter()
                .position(|existing| *existing == binding)
            else {
                return Err(crate::json_command::CommandError::new(
                    "binding_not_found",
                    "binding does not exist",
                ));
            };
            config.0.settings.input_bindings.remove(index);
            config.save().map_err(|error| {
                crate::json_command::CommandError::new("config_save_failed", error)
            })?;
            Ok(EmptyOutput {})
        });
        registry.register_typed::<BindingsReplaceDefinition>(|input, _| {
            let old_binding = ClientInputBindingConfig::try_from(input.old_binding)?;
            let replacement = ClientInputBindingConfig::try_from(input.binding)?;
            validate_binding_slot(&replacement, input_registry)?;
            let mut config = config.borrow_mut();
            let mut updated_config = config.0.clone();
            replace_binding(
                &mut updated_config.settings.input_bindings,
                old_binding,
                replacement,
            )?;
            roundo_user_config::save_config(CLIENT_CONFIG_FILE, &updated_config).map_err(
                |error| crate::json_command::CommandError::new("config_save_failed", error),
            )?;
            config.0 = updated_config;
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
                None => Err(crate::json_command::CommandError::new(
                    "unknown_command",
                    "unknown command",
                )),
            },
        });
        registry.register_typed::<DevDefinition>(|input, _| {
            let mut dev_level = dev_level.borrow_mut();
            if let Some(level) = input.level {
                if level > 3 {
                    return Err(crate::json_command::CommandError::new(
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
                crate::json_command::CommandError::new("internal_command_error", error.to_string())
            })
        });
        registry.register_typed::<DiagnosticsPositionDefinition>(|_, _| {
            let [x, y, z] = camera_position.ok_or_else(|| {
                crate::json_command::CommandError::new(
                    "camera_unavailable",
                    "player camera is unavailable",
                )
            })?;
            Ok(PositionOutput { x, y, z })
        });
        registry.register_typed::<AppQuitDefinition>(|_, _| {
            Ok(AcceptedOutput {
                status: "accepted".into(),
            })
        });
        // External navigation validates scheme/shape before crossing the OS adapter seam.
        registry.register_typed::<OpenExternalUrlDefinition>(|input, _| {
            let url = validate_external_url(&input.url)?;
            open_external_url(url).map_err(|error| {
                crate::json_command::CommandError::new(
                    "external_url_open_failed",
                    error.to_string(),
                )
            })?;
            Ok(AcceptedOutput {
                status: "accepted".into(),
            })
        });
        // Web UI navigation requires the already-admitted live source and both
        // logical lifecycle and platform navigation capabilities.
        registry.register_typed::<UiBackDefinition>(|_, _| {
            let source = ui_source.ok_or_else(|| {
                crate::json_command::CommandError::new(
                    "stale_ui_instance",
                    "ui.back requires a live WebView source",
                )
            })?;
            let mut webui = webui.borrow_mut();
            let manager = webui.as_deref_mut().ok_or_else(|| {
                crate::json_command::CommandError::new("stale_ui_instance", "Web UI is unavailable")
            })?;
            let mut navigation = navigation.borrow_mut();
            let navigation = navigation.as_deref_mut().ok_or_else(|| {
                crate::json_command::CommandError::new(
                    "ui_navigation_failed",
                    "Web UI navigation is unavailable",
                )
            })?;
            navigation
                .back(manager, source)
                .map_err(map_ui_lifecycle_error)?;
            Ok(EmptyOutput {})
        });
        registry.register_typed::<UiOpenDefinition>(|input, _| {
            let source = ui_source.ok_or_else(|| {
                crate::json_command::CommandError::new(
                    "stale_ui_instance",
                    "ui.open requires a live WebView source",
                )
            })?;
            let mut webui = webui.borrow_mut();
            let state = webui.as_deref_mut().ok_or_else(|| {
                crate::json_command::CommandError::new(
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
                crate::json_command::CommandError::new(
                    "ui_navigation_failed",
                    "Web UI navigation is unavailable",
                )
            })?;
            let opened = navigation.navigate(
                state,
                roundo_webui::UiCommandSource::WebView(source),
                target,
            );
            opened.map_err(|error| {
                log::error!("Web UI navigation to `{resource}` failed: {error}");
                map_ui_lifecycle_error(error)
            })?;
            log::info!("Web UI navigated to `{resource}`");
            Ok(UiOpenOutput { resource })
        });
        // Fail fast if declaration/catalog and executable registration drift.
        CLIENT_COMMAND_CATALOG.assert_complete(&registry);
        registry.dispatch_value(request, &mut ())
    }
}
