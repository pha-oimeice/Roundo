//! Typed command contracts, output projections and the single discovery catalog.

use super::*;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct EmptyArguments {}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct UiOpenArguments {
    pub(super) import: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct UiBackArguments {}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct OpenExternalUrlArguments {
    pub(super) url: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CommandHelpArguments {
    pub(super) command: Option<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CommandSchemaArguments {
    pub(super) command: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct DevArguments {
    pub(super) level: Option<u8>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SettingsSetArguments {
    pub(super) key: String,
    pub(super) value: f32,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct BindingArguments {
    pub(super) slot: String,
    pub(super) key: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct BindingsReplaceArguments {
    pub(super) old_binding: BindingArguments,
    pub(super) binding: BindingArguments,
}
impl TryFrom<BindingArguments> for ClientInputBindingConfig {
    type Error = roundo_cli::json_command::CommandError;
    fn try_from(value: BindingArguments) -> Result<Self, Self::Error> {
        if value.slot.trim().is_empty() {
            return Err(roundo_cli::json_command::CommandError::new(
                "invalid_arguments",
                "binding slot cannot be empty",
            ));
        }
        let key =
            serde_json::from_value(serde_json::Value::String(value.key)).map_err(|error| {
                roundo_cli::json_command::CommandError::new("invalid_arguments", error.to_string())
            })?;
        Ok(Self::new(value.slot, key))
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SlotArguments {
    pub(super) slot: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SlotNameArguments {
    pub(super) slot: String,
    pub(super) display_name: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SlotFunctionArguments {
    pub(super) slot: String,
    pub(super) function: String,
}
#[derive(Serialize, JsonSchema)]
pub(super) struct FunctionsListOutput {
    pub(super) functions: Vec<FunctionDisplay>,
}
#[derive(Serialize, JsonSchema)]
pub(super) struct FunctionDisplay {
    pub(super) function: String,
    pub(super) display_name: String,
}
#[derive(Serialize, JsonSchema)]
pub(super) struct SlotsListOutput {
    pub(super) slots: Vec<SupportedSlotDisplay>,
}

#[derive(Serialize, JsonSchema)]
pub(super) struct BindingDisplay {
    pub(super) slot: String,
    pub(super) key: String,
}
#[derive(Serialize, JsonSchema)]
pub(super) struct SupportedKeyDisplay {
    pub(super) key: String,
    pub(super) display_name: String,
}
#[derive(Serialize, JsonSchema)]
pub(super) struct SupportedSlotDisplay {
    pub(super) slot: String,
    pub(super) display_name: String,
    pub(super) functions: Vec<String>,
}
#[derive(Serialize, JsonSchema)]
pub(super) struct BindingsListOutput {
    pub(super) bindings: Vec<BindingDisplay>,
    pub(super) supported_keys: Vec<SupportedKeyDisplay>,
    pub(super) supported_slots: Vec<SupportedSlotDisplay>,
    pub(super) supported_functions: Vec<FunctionDisplay>,
}
#[derive(Serialize, JsonSchema)]
pub(super) struct BindingMutationOutput {}

pub(super) fn key_display(key: ClientInputKey) -> SupportedKeyDisplay {
    use ClientInputKey::*;
    let (key, display_name) = match key {
        Digit0 => ("digit0", "0"),
        Digit1 => ("digit1", "1"),
        Digit2 => ("digit2", "2"),
        Digit3 => ("digit3", "3"),
        Digit4 => ("digit4", "4"),
        Digit5 => ("digit5", "5"),
        Digit6 => ("digit6", "6"),
        Digit7 => ("digit7", "7"),
        Digit8 => ("digit8", "8"),
        Digit9 => ("digit9", "9"),
        Backspace => ("backspace", "Backspace"),
        Backquote => ("backquote", "~ / `"),
        Tab => ("tab", "Tab"),
        KeyQ => ("key_q", "Q"),
        KeyW => ("key_w", "W"),
        KeyE => ("key_e", "E"),
        KeyR => ("key_r", "R"),
        KeyT => ("key_t", "T"),
        KeyY => ("key_y", "Y"),
        KeyU => ("key_u", "U"),
        KeyI => ("key_i", "I"),
        KeyO => ("key_o", "O"),
        KeyP => ("key_p", "P"),
        CapsLock => ("caps_lock", "Caps Lock"),
        KeyA => ("key_a", "A"),
        KeyS => ("key_s", "S"),
        KeyD => ("key_d", "D"),
        KeyF => ("key_f", "F"),
        KeyG => ("key_g", "G"),
        KeyH => ("key_h", "H"),
        KeyJ => ("key_j", "J"),
        KeyK => ("key_k", "K"),
        KeyL => ("key_l", "L"),
        Enter => ("enter", "Enter"),
        ShiftLeft => ("shift_left", "Left Shift"),
        KeyZ => ("key_z", "Z"),
        KeyX => ("key_x", "X"),
        KeyC => ("key_c", "C"),
        KeyV => ("key_v", "V"),
        KeyB => ("key_b", "B"),
        KeyN => ("key_n", "N"),
        KeyM => ("key_m", "M"),
        ShiftRight => ("shift_right", "Right Shift"),
        ControlLeft => ("control_left", "Left Control"),
        AltLeft => ("alt_left", "Left Alt"),
        Space => ("space", "Space"),
        AltRight => ("alt_right", "Right Alt"),
        ControlRight => ("control_right", "Right Control"),
        ArrowLeft => ("arrow_left", "Left Arrow"),
        ArrowUp => ("arrow_up", "Up Arrow"),
        ArrowDown => ("arrow_down", "Down Arrow"),
        ArrowRight => ("arrow_right", "Right Arrow"),
        F1 => ("f1", "F1"),
        MouseLeft => ("mouse_left", "Left Mouse Button"),
        MouseRight => ("mouse_right", "Right Mouse Button"),
        MouseMiddle => ("mouse_middle", "Middle Mouse Button"),
        MouseMotion => ("mouse_motion", "Mouse Motion"),
    };
    SupportedKeyDisplay {
        key: key.into(),
        display_name: display_name.into(),
    }
}

pub(super) fn supported_keys() -> Vec<SupportedKeyDisplay> {
    use ClientInputKey::*;
    [
        Digit0,
        Digit1,
        Digit2,
        Digit3,
        Digit4,
        Digit5,
        Digit6,
        Digit7,
        Digit8,
        Digit9,
        Backspace,
        Backquote,
        Tab,
        KeyQ,
        KeyW,
        KeyE,
        KeyR,
        KeyT,
        KeyY,
        KeyU,
        KeyI,
        KeyO,
        KeyP,
        CapsLock,
        KeyA,
        KeyS,
        KeyD,
        KeyF,
        KeyG,
        KeyH,
        KeyJ,
        KeyK,
        KeyL,
        Enter,
        ShiftLeft,
        KeyZ,
        KeyX,
        KeyC,
        KeyV,
        KeyB,
        KeyN,
        KeyM,
        ShiftRight,
        ControlLeft,
        AltLeft,
        Space,
        AltRight,
        ControlRight,
        ArrowLeft,
        ArrowUp,
        ArrowDown,
        ArrowRight,
        F1,
        MouseLeft,
        MouseRight,
        MouseMiddle,
        MouseMotion,
    ]
    .into_iter()
    .map(key_display)
    .collect()
}

pub(super) fn binding_display(binding: &ClientInputBindingConfig) -> BindingDisplay {
    BindingDisplay {
        slot: binding.slot.clone(),
        key: key_display(binding.key).key,
    }
}

pub(super) fn replace_binding(
    bindings: &mut Vec<ClientInputBindingConfig>,
    old_binding: ClientInputBindingConfig,
    replacement: ClientInputBindingConfig,
) -> Result<(), roundo_cli::json_command::CommandError> {
    let Some(index) = bindings
        .iter()
        .position(|existing| *existing == old_binding)
    else {
        return Err(roundo_cli::json_command::CommandError::new(
            "binding_not_found",
            "binding does not exist",
        ));
    };
    if bindings
        .iter()
        .enumerate()
        .any(|(other, existing)| other != index && *existing == replacement)
    {
        return Err(roundo_cli::json_command::CommandError::new(
            "binding_exists",
            "replacement binding already exists",
        ));
    }
    bindings[index] = replacement;
    Ok(())
}

#[derive(Serialize, JsonSchema)]
pub(super) struct EmptyOutput {}
#[derive(Serialize, JsonSchema)]
pub(super) struct AcceptedOutput {
    pub(super) status: String,
}
#[derive(Serialize, JsonSchema)]
pub(super) struct SettingDisplay {
    pub(super) key: String,
    pub(super) value: f32,
    pub(super) min: f32,
    pub(super) max: Option<f32>,
    pub(super) step: f32,
    pub(super) default: f32,
}
#[derive(Serialize, JsonSchema)]
pub(super) struct SettingsShowOutput {
    pub(super) settings: Vec<SettingDisplay>,
}
#[derive(Serialize, JsonSchema)]
#[serde(untagged)]
pub(super) enum CommandHelpOutput {
    Commands { commands: Vec<&'static str> },
    Command { command: String, dev_level: u8 },
}
/// A requested schema varies with the command, so this wrapper remains transparent on the stable wire format.
#[derive(Serialize, JsonSchema)]
#[serde(transparent)]
pub(super) struct CommandSchemaOutput(pub(super) serde_json::Map<String, serde_json::Value>);
#[derive(Serialize, JsonSchema)]
pub(super) struct DevOutput {
    pub(super) level: u8,
}
#[derive(Serialize, JsonSchema)]
pub(super) struct PositionOutput {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) z: f32,
}
#[derive(Deserialize, Serialize, JsonSchema)]
pub(super) struct HudTargetOutput {
    pub(super) voxel: String,
    pub(super) voxel_position: [i32; 3],
    pub(super) relative_position: [f32; 3],
    pub(super) chunk: [i32; 3],
    pub(super) local_coordinate: String,
}
#[derive(Serialize, JsonSchema)]
pub(super) struct HudFpsOutput {
    pub(super) current: f32,
    pub(super) average: f32,
    pub(super) min: f32,
    pub(super) max: f32,
}
#[derive(Serialize, JsonSchema)]
pub(super) struct HudPositionOutput {
    pub(super) absolute: [f32; 3],
    pub(super) chunk_relative: [i32; 3],
}
#[derive(Serialize, JsonSchema)]
pub(super) struct HudShowOutput {
    pub(super) fps: HudFpsOutput,
    pub(super) position: Option<HudPositionOutput>,
    pub(super) target: Option<HudTargetOutput>,
}
#[derive(Serialize, JsonSchema)]
pub(super) struct UiOpenOutput {
    pub(super) resource: String,
}

pub(super) fn settings_show_output(config: &ClientConfigStore) -> SettingsShowOutput {
    let settings = config.settings();
    SettingsShowOutput {
        settings: vec![
            SettingDisplay {
                key: "controls.mouse_sensitivity".into(),
                value: settings.controls.mouse_sensitivity,
                min: roundo_user_config::MIN_MOUSE_SENSITIVITY,
                max: Some(roundo_user_config::MAX_MOUSE_SENSITIVITY),
                step: 0.0005,
                default: roundo_user_config::DEFAULT_MOUSE_SENSITIVITY,
            },
            SettingDisplay {
                key: "camera.move_speed".into(),
                value: settings.camera.move_speed,
                min: roundo_user_config::MIN_CAMERA_MOVE_SPEED,
                max: Some(roundo_user_config::MAX_CAMERA_MOVE_SPEED),
                step: 1.0,
                default: roundo_user_config::DEFAULT_CAMERA_MOVE_SPEED,
            },
            SettingDisplay {
                key: "camera.voxel_raycast_distance".into(),
                value: settings.camera.voxel_raycast_distance,
                min: roundo_user_config::MIN_VOXEL_RAYCAST_DISTANCE,
                max: Some(roundo_user_config::MAX_VOXEL_RAYCAST_DISTANCE),
                step: 1.0,
                default: roundo_user_config::DEFAULT_VOXEL_RAYCAST_DISTANCE,
            },
            SettingDisplay {
                key: "world.joinable_world_radius".into(),
                value: settings.world.joinable_world_radius,
                min: roundo_user_config::MIN_JOINABLE_WORLD_RADIUS,
                max: Some(roundo_user_config::MAX_JOINABLE_WORLD_RADIUS),
                step: 0.5,
                default: roundo_user_config::DEFAULT_JOINABLE_WORLD_RADIUS,
            },
            SettingDisplay {
                key: "world.chunk_view_distance".into(),
                value: settings.world.chunk_view_distance,
                min: roundo_user_config::MIN_CHUNK_VIEW_DISTANCE,
                max: Some(roundo_user_config::MAX_CHUNK_VIEW_DISTANCE),
                step: 1.0,
                default: roundo_user_config::DEFAULT_CHUNK_VIEW_DISTANCE,
            },
            SettingDisplay {
                key: "world.geometry_lod_pixel_error".into(),
                value: settings.world.geometry_lod_pixel_error,
                min: 0.0,
                max: None,
                step: roundo_user_config::DEFAULT_GEOMETRY_LOD_PIXEL_ERROR,
                default: roundo_user_config::DEFAULT_GEOMETRY_LOD_PIXEL_ERROR,
            },
        ],
    }
}

pub(super) fn edit_input_configuration(
    config: &mut ClientConfigStore,
    registry: &InputFunctionRegistry,
    edit: impl FnOnce(
        &mut roundo_user_config::ClientSettingsConfig,
    ) -> Result<(), roundo_cli::json_command::CommandError>,
) -> Result<EmptyOutput, roundo_cli::json_command::CommandError> {
    let mut settings = config.settings().clone();
    edit(&mut settings)?;
    crate::config::validate_input_configuration(&settings, registry).map_err(|error| {
        roundo_cli::json_command::CommandError::new("invalid_input_configuration", error)
    })?;
    config.replace_input_settings(settings).map_err(|error| {
        roundo_cli::json_command::CommandError::new("config_save_failed", error)
    })?;
    Ok(EmptyOutput {})
}

pub(super) fn slots_list_output(config: &ClientConfigStore) -> SlotsListOutput {
    SlotsListOutput {
        slots: config
            .settings()
            .input_slots
            .iter()
            .map(|(id, slot)| SupportedSlotDisplay {
                slot: id.clone(),
                display_name: slot.display_name.clone(),
                functions: slot.functions.iter().cloned().collect(),
            })
            .collect(),
    }
}

pub(super) fn functions_list_output(registry: &InputFunctionRegistry) -> FunctionsListOutput {
    FunctionsListOutput {
        functions: registry
            .functions()
            .map(|(name, definition)| FunctionDisplay {
                function: name.into(),
                display_name: definition.display_name.clone(),
            })
            .collect(),
    }
}

pub(super) fn bindings_list_output(
    config: &ClientConfigStore,
    registry: &InputFunctionRegistry,
) -> BindingsListOutput {
    BindingsListOutput {
        bindings: config
            .settings()
            .input_bindings
            .iter()
            .map(binding_display)
            .collect(),
        supported_keys: supported_keys(),
        supported_slots: slots_list_output(config).slots,
        supported_functions: functions_list_output(registry).functions,
    }
}

pub(super) fn hud_show_output(hud: &HudCache) -> Result<HudShowOutput, serde_json::Error> {
    let target = hud.target.clone().map(serde_json::from_value).transpose()?;
    Ok(HudShowOutput {
        fps: HudFpsOutput {
            current: hud.fps.current,
            average: hud.fps.average,
            min: hud.fps.min,
            max: hud.fps.max,
        },
        position: hud.position.as_ref().map(|position| HudPositionOutput {
            absolute: position.absolute,
            chunk_relative: position.chunk_relative,
        }),
        target,
    })
}

macro_rules! definition {
    ($definition:ident, $input:ty, $output:ty, $name:literal, $level:literal) => {
        pub(super) struct $definition;
        impl roundo_cli::CommandDefinition for $definition {
            type Input = $input;
            type Output = $output;
            const NAME: &'static str = $name;
            const DEV_LEVEL: u8 = $level;
        }
    };
}
definition!(
    AppQuitDefinition,
    EmptyArguments,
    AcceptedOutput,
    "app.quit",
    0
);
definition!(
    OpenExternalUrlDefinition,
    OpenExternalUrlArguments,
    AcceptedOutput,
    "app.open-external-url",
    0
);
definition!(
    UiOpenDefinition,
    UiOpenArguments,
    UiOpenOutput,
    "ui.open",
    0
);
definition!(UiBackDefinition, UiBackArguments, EmptyOutput, "ui.back", 0);
definition!(
    ScenePauseDefinition,
    EmptyArguments,
    EmptyOutput,
    "scene.pause",
    0
);
definition!(
    SceneResumeDefinition,
    EmptyArguments,
    EmptyOutput,
    "scene.resume",
    0
);
definition!(
    CommandSchemaDefinition,
    CommandSchemaArguments,
    CommandSchemaOutput,
    "command.schema",
    1
);
definition!(
    CommandHelpDefinition,
    CommandHelpArguments,
    CommandHelpOutput,
    "command.help",
    0
);
definition!(DevDefinition, DevArguments, DevOutput, "dev", 0);
definition!(
    SettingsSetDefinition,
    SettingsSetArguments,
    EmptyOutput,
    "settings.set",
    0
);
definition!(
    SettingsShowDefinition,
    EmptyArguments,
    SettingsShowOutput,
    "settings.show",
    0
);
definition!(
    SlotsListDefinition,
    EmptyArguments,
    SlotsListOutput,
    "slots.list",
    0
);
definition!(
    SlotsCreateDefinition,
    SlotNameArguments,
    EmptyOutput,
    "slots.create",
    0
);
definition!(
    SlotsRenameDefinition,
    SlotNameArguments,
    EmptyOutput,
    "slots.rename",
    0
);
definition!(
    SlotsDeleteDefinition,
    SlotArguments,
    EmptyOutput,
    "slots.delete",
    0
);
definition!(
    SlotsAssignDefinition,
    SlotFunctionArguments,
    EmptyOutput,
    "slots.assign",
    0
);
definition!(
    SlotsUnassignDefinition,
    SlotFunctionArguments,
    EmptyOutput,
    "slots.unassign",
    0
);
definition!(
    FunctionsListDefinition,
    EmptyArguments,
    FunctionsListOutput,
    "functions.list",
    0
);
definition!(
    BindingsListDefinition,
    EmptyArguments,
    BindingsListOutput,
    "bindings.list",
    0
);
definition!(
    BindingsBindDefinition,
    BindingArguments,
    EmptyOutput,
    "bindings.bind",
    0
);
definition!(
    BindingsUnbindDefinition,
    BindingArguments,
    EmptyOutput,
    "bindings.unbind",
    0
);
definition!(
    BindingsReplaceDefinition,
    BindingsReplaceArguments,
    BindingMutationOutput,
    "bindings.replace",
    0
);
definition!(
    DiagnosticsPositionDefinition,
    EmptyArguments,
    PositionOutput,
    "diagnostics.position",
    1
);
definition!(
    HudShowDefinition,
    EmptyArguments,
    HudShowOutput,
    "hud.show",
    1
);

/// 命令目录统一发现、schema 与 dispatch 注册，Unix adapter 只能投影这些 typed JSON 命令。
pub(super) struct ClientCommandCatalogEntry {
    pub(super) name: &'static str,
    pub(super) dev_level: u8,
    pub(super) schema: fn() -> serde_json::Value,
}

impl ClientCommandCatalogEntry {
    const fn of<D: CommandDefinition>() -> Self {
        Self {
            name: D::NAME,
            dev_level: D::DEV_LEVEL,
            schema: roundo_cli::json_command::command_schema::<D>,
        }
    }
}

pub(super) const CLIENT_COMMANDS: &[ClientCommandCatalogEntry] = &[
    ClientCommandCatalogEntry::of::<AppQuitDefinition>(),
    ClientCommandCatalogEntry::of::<OpenExternalUrlDefinition>(),
    ClientCommandCatalogEntry::of::<UiOpenDefinition>(),
    ClientCommandCatalogEntry::of::<UiBackDefinition>(),
    ClientCommandCatalogEntry::of::<ScenePauseDefinition>(),
    ClientCommandCatalogEntry::of::<SceneResumeDefinition>(),
    ClientCommandCatalogEntry::of::<SettingsShowDefinition>(),
    ClientCommandCatalogEntry::of::<SettingsSetDefinition>(),
    ClientCommandCatalogEntry::of::<SlotsListDefinition>(),
    ClientCommandCatalogEntry::of::<SlotsCreateDefinition>(),
    ClientCommandCatalogEntry::of::<SlotsRenameDefinition>(),
    ClientCommandCatalogEntry::of::<SlotsDeleteDefinition>(),
    ClientCommandCatalogEntry::of::<SlotsAssignDefinition>(),
    ClientCommandCatalogEntry::of::<SlotsUnassignDefinition>(),
    ClientCommandCatalogEntry::of::<FunctionsListDefinition>(),
    ClientCommandCatalogEntry::of::<BindingsListDefinition>(),
    ClientCommandCatalogEntry::of::<BindingsBindDefinition>(),
    ClientCommandCatalogEntry::of::<BindingsUnbindDefinition>(),
    ClientCommandCatalogEntry::of::<BindingsReplaceDefinition>(),
    ClientCommandCatalogEntry::of::<DiagnosticsPositionDefinition>(),
    ClientCommandCatalogEntry::of::<HudShowDefinition>(),
    ClientCommandCatalogEntry::of::<CommandHelpDefinition>(),
    ClientCommandCatalogEntry::of::<CommandSchemaDefinition>(),
    ClientCommandCatalogEntry::of::<DevDefinition>(),
];

/// Owns command discovery, visibility, schemas, and dispatch completeness as
/// one deep module. Source adapters never inspect the declaration list.
pub(super) struct ClientCommandCatalog;

pub(super) const CLIENT_COMMAND_CATALOG: ClientCommandCatalog = ClientCommandCatalog;

impl ClientCommandCatalog {
    pub(super) fn visible(&self, level: u8) -> Vec<&'static str> {
        CLIENT_COMMANDS
            .iter()
            .filter_map(|command| (command.dev_level <= level).then_some(command.name))
            .collect()
    }

    pub(super) fn dev_level(&self, name: &str) -> Option<u8> {
        CLIENT_COMMANDS
            .iter()
            .find_map(|command| (command.name == name).then_some(command.dev_level))
    }

    pub(super) fn schema(
        &self,
        command: &str,
    ) -> Result<serde_json::Value, roundo_cli::json_command::CommandError> {
        CLIENT_COMMANDS
            .iter()
            .find(|entry| entry.name == command)
            .map(|entry| (entry.schema)())
            .ok_or_else(|| match command.is_empty() {
                true => roundo_cli::json_command::CommandError::new(
                    "invalid_arguments",
                    "command must be a non-empty string",
                ),
                false => roundo_cli::json_command::CommandError::new(
                    "unknown_command",
                    "schema is unavailable",
                ),
            })
    }

    /// Refuses dispatch when composition omitted a catalog command. This check
    /// is owned here rather than repeated by UI and terminal adapters.
    pub(super) fn assert_complete<Context>(
        &self,
        registry: &roundo_cli::json_command::CommandRegistry<'_, Context>,
    ) {
        let mut catalog_names = CLIENT_COMMANDS
            .iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>();
        catalog_names.sort_unstable();
        assert_eq!(
            registry.registered_names(),
            catalog_names,
            "client command catalog and dispatch registrations diverged"
        );
    }
}
