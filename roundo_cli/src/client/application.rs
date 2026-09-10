use super::*;

pub(super) struct ClientCommandApplication<'a> {
    pub(super) config: &'a mut ClientConfigStore,
    pub(super) network: &'a mut crate::client_network::ClientNetworkManager,
    pub(super) probes: &'a crate::client_network::ServerProbeManager,
    pub(super) dev_level: &'a mut DevLevel,
    pub(super) camera_position: Option<[f32; 3]>,
    pub(super) webui: Option<&'a mut roundo_webui::UiLifecycleManager>,
    pub(super) navigation: Option<&'a mut roundo_webui::UiNavigationExecutor>,
    pub(super) hud: &'a HudCache,
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
        registry.register_typed::<ServerConnectDefinition>(|input, _| {
            let config = config.borrow();
            let mut network = network.borrow_mut();
            let server = config.0.servers.get(input.index).ok_or_else(|| {
                crate::json_command::CommandError::new(
                    "index_out_of_range",
                    "server index does not exist",
                )
            })?;
            network.connect(server).map_err(|error| {
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
            config.0.servers.remove(input.index);
            config.save().map_err(|error| {
                crate::json_command::CommandError::new("config_save_failed", error)
            })?;
            probes.invalidate();
            Ok(EmptyOutput {})
        });
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
        registry.register_typed::<BindingsListDefinition>(|_, _| {
            Ok(bindings_list_output(&config.borrow()))
        });
        registry.register_typed::<BindingsBindDefinition>(|input, _| {
            let binding = ClientKeyBindingConfig::try_from(input)?;
            let mut config = config.borrow_mut();
            match config
                .0
                .settings
                .key_bindings
                .iter_mut()
                .find(|existing| existing.key == binding.key)
            {
                Some(existing) => {
                    if binding
                        .actions
                        .iter()
                        .all(|action| existing.actions.contains(action))
                    {
                        return Err(crate::json_command::CommandError::new(
                            "binding_exists",
                            "binding already exists",
                        ));
                    }
                    let additions = binding
                        .actions
                        .into_iter()
                        .filter(|action| !existing.actions.contains(action))
                        .collect::<Vec<_>>();
                    existing.actions.extend(additions);
                }
                None => config.0.settings.key_bindings.push(binding),
            }
            config.save().map_err(|error| {
                crate::json_command::CommandError::new("config_save_failed", error)
            })?;
            Ok(EmptyOutput {})
        });
        registry.register_typed::<BindingsUnbindDefinition>(|input, _| {
            let binding = ClientKeyBindingConfig::try_from(input)?;
            let mut config = config.borrow_mut();
            let Some(index) = config
                .0
                .settings
                .key_bindings
                .iter()
                .position(|existing| existing.key == binding.key)
            else {
                return Err(crate::json_command::CommandError::new(
                    "binding_not_found",
                    "binding does not exist",
                ));
            };
            let existing = &mut config.0.settings.key_bindings[index];
            let before = existing.actions.len();
            existing
                .actions
                .retain(|action| !binding.actions.contains(action));
            if before == existing.actions.len() {
                return Err(crate::json_command::CommandError::new(
                    "binding_not_found",
                    "binding does not exist",
                ));
            }
            if existing.actions.is_empty() {
                config.0.settings.key_bindings.remove(index);
            }
            config.save().map_err(|error| {
                crate::json_command::CommandError::new("config_save_failed", error)
            })?;
            Ok(EmptyOutput {})
        });
        registry.register_typed::<BindingsReplaceDefinition>(|input, _| {
            let old_binding = ClientKeyBindingConfig::try_from(input.old_binding)?;
            let replacement = ClientKeyBindingConfig::try_from(input.binding)?;
            let mut config = config.borrow_mut();
            let mut updated_config = config.0.clone();
            replace_binding(
                &mut updated_config.settings.key_bindings,
                old_binding,
                replacement,
            )?;
            roundo_user_config::save_config(CLIENT_CONFIG_FILE, &updated_config).map_err(
                |error| crate::json_command::CommandError::new("config_save_failed", error),
            )?;
            config.0 = updated_config;
            Ok(BindingMutationOutput {})
        });
        registry.register_typed::<CommandSchemaDefinition>(|input, _| {
            client_command_schema(&input.command).map(|schema| {
                CommandSchemaOutput(schema.as_object().cloned().expect("schema is an object"))
            })
        });
        registry.register_typed::<CommandHelpDefinition>(|input, _| match input.command {
            None => Ok(CommandHelpOutput::Commands {
                commands: visible_commands(dev_level.borrow().0),
            }),
            Some(command) => match command_dev_level(&command) {
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
            let mut webui = webui.borrow_mut();
            let state = webui.as_deref_mut().ok_or_else(|| {
                crate::json_command::CommandError::new(
                    "invalid_ui_resource",
                    "Web UI is unavailable",
                )
            })?;
            let target = match (input.slot, input.resource) {
                (Some(slot), None) => state.resolve_slot_path(&slot, input.path.as_deref()),
                (None, Some(resource)) => {
                    state.resolve_resource_path(&resource, input.path.as_deref())
                }
                _ => Err(roundo_webui::UiRegistryError::InvalidReference(
                    "ui.open requires exactly one slot or resource".into(),
                )),
            }
            .map_err(|error| {
                crate::json_command::CommandError::new("invalid_ui_resource", error.to_string())
            })?;
            let resource = target.resource().to_owned();
            log::debug!("Resolved Web UI navigation target `{resource}`");
            let mut navigation = navigation.borrow_mut();
            let navigation = navigation.as_deref_mut().ok_or_else(|| {
                crate::json_command::CommandError::new(
                    "ui_navigation_failed",
                    "Web UI navigation is unavailable",
                )
            })?;
            let source = ui_source
                .map(roundo_webui::UiCommandSource::WebView)
                .unwrap_or(roundo_webui::UiCommandSource::Host);
            navigation.open(state, source, target).map_err(|error| {
                log::error!("Web UI navigation to `{resource}` failed: {error}");
                map_ui_lifecycle_error(error)
            })?;
            log::info!("Web UI navigated to `{resource}`");
            Ok(UiOpenOutput { resource })
        });
        assert_catalog_matches_registry(&registry);
        registry.dispatch_value(request, &mut ())
    }
}
