//! UI Registry、Lifecycle core 与平台 adapter 的公开 interface 回归测试。

use super::*;
use crate::platform::{claim_webview_creation_turn, enqueue_webui_command};
use crate::registry::{LayoutRegistration, RegistryFile};
use roundo_mod_loader::{LoadedMods, ModId, parse_mod_id};
use roundo_toolbox::request_response_pipe::{
    CommandTransport, CommandTransportContext, ContextualJsonRequestResponseIo,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn fixture_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "roundo-webui-test-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mod_root = root.join("vanilla_ui");
    fs::create_dir_all(mod_root.join("assets/webui/main")).unwrap();
    fs::write(
        mod_root.join("manifest.toml"),
        "[general]\nmod_name='vanilla_ui'\nauthor='vanilla'\n",
    )
    .unwrap();
    fs::write(mod_root.join("assets/webui/main/index.html"), "ok").unwrap();
    fs::write(mod_root.join("assets/webui/main/about.html"), "about").unwrap();
    fs::write(mod_root.join("assets/webui/registry.toml"), "[[resource]]\nname='main'\nproject='main'\nentry='index.html'\ninteraction_mode='web-ui'\nworld_visibility='hidden'\nmax_instances=2\nprefetch=['roundo.main-menu']\n[slots]\n'roundo.disconnected-root'='main'
'roundo.main-menu'='main'\n").unwrap();
    root
}

#[cfg(target_os = "windows")]
#[test]
fn root_replacement_keeps_the_old_root_live_until_the_new_root_commits() {
    let root = fixture_root();
    let registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
    let mut manager = UiLifecycleManager::new(registry);
    manager
        .registry
        .slots
        .insert(CONNECTED_ROOT_SLOT.into(), "vanilla.vanilla_ui.main".into());
    let old_root = manager.open_configured_root().unwrap().unwrap();
    let obsolete_target = manager
        .resolve_resource_path("vanilla.vanilla_ui.main", Some("about.html"))
        .unwrap();
    let obsolete_pending = manager
        .begin_open(UiCommandSource::WebView(old_root), obsolete_target)
        .unwrap();
    let mut executor = UiNavigationExecutor::default();

    let pending = match executor
        .replace_root(&mut manager, UiLifecycleState::Connected)
        .unwrap()
    {
        UiRootReplacement::Pending(pending) => pending,
        UiRootReplacement::Completed(_) => panic!("configured Root must load before commit"),
    };

    assert_eq!(manager.lifecycle_state(), UiLifecycleState::Disconnected);
    assert!(manager.instance(old_root).is_some());
    assert!(manager.command_source_is_live(old_root));
    assert_eq!(
        manager.commit_open(obsolete_pending),
        Err(UiLifecycleError::StaleUiInstance),
        "a pending open owned by the old Root must not race its replacement"
    );

    let committed = manager.commit_root_replacement(pending.get()).unwrap();
    assert_eq!(manager.lifecycle_state(), UiLifecycleState::Connected);
    assert_eq!(committed.destroyed, vec![old_root]);
    assert!(manager.instance(old_root).is_none());
    assert!(manager.instance(committed.instance).is_some());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn registry_resolves_prefetch_through_the_selected_slot() {
    let root = fixture_root();
    let replacement_root = root.join("replacement_ui");
    fs::create_dir_all(replacement_root.join("assets/webui/replacement")).unwrap();
    fs::write(
        replacement_root.join("manifest.toml"),
        "[general]\nmod_name='replacement_ui'\nauthor='example'\noverride_priority=10\n",
    )
    .unwrap();
    fs::write(
        replacement_root.join("assets/webui/replacement/index.html"),
        "replacement",
    )
    .unwrap();
    fs::write(
        replacement_root.join("assets/webui/registry.toml"),
        "[[resource]]\nname='replacement'\nproject='replacement'\nentry='index.html'\ninteraction_mode='web-ui'\nworld_visibility='hidden'\nmax_instances=1\n[slots]\n'roundo.main-menu'='replacement'\n",
    )
    .unwrap();

    let registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
    assert_eq!(
        registry
            .resource("vanilla.vanilla_ui.main")
            .unwrap()
            .prefetch,
        vec!["example.replacement_ui.replacement"]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn registry_rejects_unknown_prefetch_slots_atomically() {
    let root = fixture_root();
    let path = root.join("vanilla_ui/assets/webui/registry.toml");
    let invalid = fs::read_to_string(&path).unwrap().replace(
        "prefetch=['roundo.main-menu']",
        "prefetch=['roundo.missing']",
    );
    fs::write(path, invalid).unwrap();

    assert!(matches!(
        UiRegistry::load(&LoadedMods::discover(&root).unwrap()),
        Err(UiRegistryError::UnknownSlot(slot)) if slot == "roundo.missing"
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn prefetched_document_is_not_live_until_claimed_and_committed() {
    let root = fixture_root();
    let registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
    let mut manager = UiLifecycleManager::new(registry);
    let owner = manager.open_configured_root().unwrap().unwrap();
    let target = manager
        .resolve_resource_path("vanilla.vanilla_ui.main", Some("about.html"))
        .unwrap();

    let pending = manager.begin_prefetch(target.clone()).unwrap();
    assert!(manager.pending_descriptor(pending).unwrap().prefetched);
    assert_eq!(manager.live_count("vanilla.vanilla_ui.main"), 1);
    assert_eq!(manager.parent_instance(owner), None);

    assert_eq!(
        manager
            .claim_prefetch(UiCommandSource::WebView(owner), &target)
            .unwrap(),
        Some(pending)
    );
    assert!(!manager.pending_descriptor(pending).unwrap().prefetched);
    let instance = manager.commit_open(pending).unwrap();
    assert_eq!(manager.parent_instance(instance), Some(owner));
    assert_eq!(manager.live_count("vanilla.vanilla_ui.main"), 2);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "windows")]
#[test]
fn staged_command_gate_flushes_in_order_and_never_requeues_after_commit() {
    let mut gate = StagedCommandGate::default();
    assert!(gate.queue_until_commit("first"));
    assert!(gate.queue_until_commit("second"));
    assert_eq!(gate.commit(), vec!["first", "second"]);
    assert!(!gate.queue_until_commit("third"));
    assert!(gate.commit().is_empty());
}

#[test]
fn navigation_rejects_cross_definition_slots_and_external_urls() {
    let source = "owner.mod.main";
    assert!(top_level_navigation_allowed(
        source,
        "roundo-ui://resource/owner.mod.main/about.html"
    ));
    assert!(top_level_navigation_allowed(
        source,
        "http://roundo-ui.resource/owner.mod.main/about.html"
    ));
    assert!(!top_level_navigation_allowed(
        source,
        "roundo-ui://resource/other.mod.page/index.html"
    ));
    assert!(!top_level_navigation_allowed(
        source,
        "roundo-ui://slot/roundo.settings/"
    ));
    assert!(!top_level_navigation_allowed(
        source,
        "https://example.test/"
    ));
}

#[test]
fn frame_navigation_policy_allows_only_same_definition_and_blank() {
    let source = "owner.mod.main";
    assert!(frame_navigation_allowed(source, "about:blank"));
    assert!(frame_navigation_allowed(
        source,
        "http://roundo-ui.resource/owner.mod.main/frame.html"
    ));
    assert!(!frame_navigation_allowed(
        source,
        "http://roundo-ui.resource/other.mod.page/frame.html"
    ));
    assert!(!frame_navigation_allowed(source, "https://example.test/"));
}

#[test]
fn initialization_script_installs_cross_definition_iframe_guard() {
    let script = webui_initialization_script("owner.mod.main");
    assert!(script.contains("MutationObserver"));
    assert!(script.contains("HTMLIFrameElement"));
    assert!(script.contains("/owner.mod.main/"));
    assert!(script.contains("roundo_bridge_ready"));
}

#[cfg(target_os = "windows")]
#[test]
fn prefetch_prepares_only_the_initial_blank_webview() {
    assert!(!should_start_document_load(true, false));
    assert!(!should_start_document_load(true, true));
    assert!(!should_start_document_load(false, false));
    assert!(should_start_document_load(false, true));

    let timeout = Duration::from_secs(15);
    assert_eq!(
        prepared_webview_readiness(true, false, Duration::ZERO, timeout),
        StagedReadiness::Pending
    );
    assert_eq!(
        prepared_webview_readiness(true, true, Duration::from_secs(1), timeout),
        StagedReadiness::Commit
    );
    assert_eq!(
        prepared_webview_readiness(false, true, Duration::ZERO, timeout),
        StagedReadiness::Stale
    );
}

#[cfg(target_os = "windows")]
#[test]
fn staged_commit_requires_native_success_and_bridge_and_surfaces_failure_first() {
    let timeout = Duration::from_secs(15);
    assert_eq!(
        staged_readiness(true, None, true, Duration::ZERO, timeout),
        StagedReadiness::Pending
    );
    assert_eq!(
        staged_readiness(true, Some(&Ok(())), false, Duration::from_secs(1), timeout),
        StagedReadiness::Pending
    );
    assert_eq!(
        staged_readiness(true, Some(&Ok(())), true, Duration::from_secs(1), timeout),
        StagedReadiness::Commit
    );
    assert_eq!(
        staged_readiness(
            true,
            Some(&Err("dns".into())),
            true,
            Duration::from_secs(20),
            timeout
        ),
        StagedReadiness::Failed("dns".into())
    );
    assert_eq!(
        staged_readiness(true, None, false, timeout, timeout),
        StagedReadiness::Timeout
    );
    assert_eq!(
        staged_readiness(false, Some(&Ok(())), true, Duration::ZERO, timeout),
        StagedReadiness::Stale
    );
}

#[test]
fn same_definition_navigation_extracts_only_a_local_path() {
    assert_eq!(
        same_definition_path(
            "owner.mod.main",
            "http://roundo-ui.resource/owner.mod.main/settings/page.html?x=1#part"
        ),
        Some("settings/page.html".into())
    );
    assert_eq!(
        same_definition_path(
            "owner.mod.main",
            "roundo-ui://resource/other.mod.page/index.html"
        ),
        None
    );
}

#[test]
fn bridge_handshake_is_explicit_and_cannot_be_confused_with_a_command() {
    assert!(bridge_handshake(r#"{"roundo_bridge_ready":true}"#));
    assert!(bridge_focus_request(r#"{"roundo_focus_request":true}"#));
    assert!(!bridge_focus_request(r#"{"roundo_bridge_ready":true}"#));
    assert!(!bridge_handshake(
        r#"{"request_id":1,"command":{"command":"app.quit"}}"#
    ));
    assert!(!bridge_handshake("not json"));
}

#[test]
fn response_transport_serializes_json_without_script_interpolation() {
    let script = response_script(7, &json!({"ok": true, "message": "done"}));
    assert_eq!(
        script,
        "window.__roundoResolve(7, {\"message\":\"done\",\"ok\":true});"
    );
}

#[test]
fn ipc_submits_the_inner_command_without_transport_fields() {
    let pipe = roundo_toolbox::request_response_pipe::RequestResponsePipe::<
        CommandTransport<CommandTransportContext>,
        Value,
    >::bounded(1);
    let io = ContextualJsonRequestResponseIo::new(pipe.io());
    let pending = Arc::new(Mutex::new(Vec::new()));
    enqueue_webui_command(
        &Some(io),
        &pending,
        None,
        r#"{"request_id":7,"command":{"version":1,"command":"app.quit","arguments":{}}}"#,
    );
    let (transport, _reply) = pipe.try_receive().expect("inner command was queued");
    assert_eq!(transport.context, CommandTransportContext::Host);
    assert_eq!(
        transport.command,
        json!({"version":1,"command":"app.quit","arguments":{}})
    );
    let pending = pending.lock().unwrap();
    assert_eq!(pending[0].request_id, 7);
    assert!(pending[0].call.is_some());
    assert!(pending[0].immediate.is_none());
}

#[test]
fn ipc_source_context_is_transport_owned_and_command_payload_is_unchanged() {
    let pipe = roundo_toolbox::request_response_pipe::RequestResponsePipe::<
        CommandTransport<CommandTransportContext>,
        Value,
    >::bounded(1);
    let io = ContextualJsonRequestResponseIo::new(pipe.io());
    let pending = Arc::new(Mutex::new(Vec::new()));
    let page_command = json!({
        "version": 1,
        "command": "ui.back",
        "arguments": {},
    });
    enqueue_webui_command(
        &Some(io),
        &pending,
        Some(UiInstanceId::from_host_id(9)),
        &json!({"request_id": 7, "command": page_command}).to_string(),
    );
    let (transport, _) = pipe.try_receive().unwrap();
    assert_eq!(
        transport.context,
        CommandTransportContext::WebView { instance_id: 9 }
    );
    assert_eq!(
        transport.command,
        json!({
            "version": 1,
            "command": "ui.back",
            "arguments": {},
        })
    );
}

#[test]
fn ipc_missing_transport_command_becomes_a_typed_immediate_result() {
    let pending = Arc::new(Mutex::new(Vec::new()));
    enqueue_webui_command(&None, &pending, None, r#"{"request_id":7}"#);
    assert_eq!(
        pending.lock().unwrap()[0].immediate,
        Some(json!({
            "version": 1,
            "command": "",
            "ok": false,
            "error": {
                "code": "invalid_command_envelope",
                "message": "transport command is required",
            },
        }))
    );
}

#[test]
fn ipc_queue_full_becomes_a_typed_immediate_result() {
    let pipe = roundo_toolbox::request_response_pipe::RequestResponsePipe::<
        CommandTransport<CommandTransportContext>,
        Value,
    >::bounded(1);
    let io = ContextualJsonRequestResponseIo::new(pipe.io());
    let _held_call = io
        .submit(json!({"occupied": true}), CommandTransportContext::Host)
        .unwrap();
    let pending = Arc::new(Mutex::new(Vec::new()));
    enqueue_webui_command(
        &Some(io),
        &pending,
        None,
        r#"{"request_id":7,"command":{"version":1,"command":"app.quit","arguments":{}}}"#,
    );
    let pending = pending.lock().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].request_id, 7);
    assert_eq!(
        pending[0].immediate,
        Some(json!({
            "version": 1,
            "command": "app.quit",
            "ok": false,
            "error": {"code": "command_queue_full", "message": "command queue is full"}
        }))
    );
}

#[test]
fn ipc_rejects_oversized_commands_before_queueing() {
    let pipe = roundo_toolbox::request_response_pipe::RequestResponsePipe::<
        CommandTransport<CommandTransportContext>,
        Value,
    >::bounded(1);
    let io = ContextualJsonRequestResponseIo::new(pipe.io());
    let pending = Arc::new(Mutex::new(Vec::new()));
    let body = serde_json::to_string(&json!({
        "request_id": 7,
        "command": {
            "version": 1,
            "command": "app.quit",
            "arguments": {"payload": "x".repeat(64 * 1024)},
        },
    }))
    .unwrap();
    enqueue_webui_command(&Some(io), &pending, None, &body);
    let result = pending.lock().unwrap()[0].immediate.clone().unwrap();
    assert_eq!(result["version"], 1);
    assert_eq!(result["command"], "app.quit");
    assert_eq!(result["ok"], false);
    assert_eq!(result["error"]["code"], "command_input_too_large");
    assert!(pipe.try_receive().is_none());
}

#[test]
fn ipc_unavailable_and_disconnected_errors_keep_the_inner_command_name() {
    let pending = Arc::new(Mutex::new(Vec::new()));
    let body = r#"{"request_id":7,"command":{"version":1,"command":"server.list","arguments":{}}}"#;
    enqueue_webui_command(&None, &pending, None, body);
    assert_eq!(
        pending.lock().unwrap()[0].immediate.as_ref().unwrap()["command"],
        "server.list"
    );

    let pipe = roundo_toolbox::request_response_pipe::RequestResponsePipe::<
        CommandTransport<CommandTransportContext>,
        Value,
    >::bounded(1);
    let io = ContextualJsonRequestResponseIo::new(pipe.io());
    drop(pipe);
    let pending = Arc::new(Mutex::new(Vec::new()));
    enqueue_webui_command(&Some(io), &pending, None, body);
    let result = pending.lock().unwrap()[0].immediate.clone().unwrap();
    assert_eq!(result["version"], 1);
    assert_eq!(result["command"], "server.list");
    assert_eq!(result["ok"], false);
    assert_eq!(result["error"]["code"], "internal_command_error");
}

#[test]
fn custom_protocol_rejects_cross_definition_frames_and_assets() {
    let root = fixture_root();
    let project = fs::canonicalize(root.join("vanilla_ui/assets/webui/main")).unwrap();
    let owner = parse_mod_id("owner.mod").unwrap();
    let allowed = parse_mod_id("allowed.mod").unwrap();
    let denied = parse_mod_id("denied.mod").unwrap();
    let resource = |name: &str, owner: ModId| UiResource {
        name: name.into(),
        owner,
        project_root: project.clone(),
        entry: project.join("index.html"),
        entry_path: "index.html".into(),
        interaction_mode: ClientInteractionMode::WebUi,
        world_visibility: UiWorldVisibility::Hidden,
        max_instances: 2,
        lifecycle_independent: false,
        presentation: PresentationMode::Exclusive,
        layout: UiLayout::Fullscreen,
        prefetch: Vec::new(),
    };
    let mut registry = UiRegistry::default();
    registry.resources.insert(
        "owner.mod.main".into(),
        resource("owner.mod.main", owner.clone()),
    );
    registry.resources.insert(
        "allowed.mod.shared".into(),
        resource("allowed.mod.shared", allowed.clone()),
    );
    registry.resources.insert(
        "denied.mod.shared".into(),
        resource("denied.mod.shared", denied),
    );
    registry
        .slots
        .insert("roundo.shared".into(), "allowed.mod.shared".into());
    assert!(matches!(
        registry.read_protocol_asset(Some("owner.mod.main"), "slot", "/roundo.shared/"),
        Err(UiRegistryError::InvalidReference(_))
    ));
    assert!(matches!(
        registry.read_protocol_asset(
            Some("owner.mod.main"),
            "resource",
            "/allowed.mod.shared/index.html"
        ),
        Err(UiRegistryError::InvalidReference(_))
    ));
    assert_eq!(
        registry
            .read_protocol_asset(
                Some("owner.mod.main"),
                "resource",
                "/owner.mod.main/index.html"
            )
            .unwrap()
            .0,
        b"ok"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn registry_requires_positive_max_instances_and_applies_definition_defaults() {
    assert!(toml::from_str::<RegistryFile>("[[resource]]\nname='x'\nproject='x'\nentry='index.html'\ninteraction_mode='web-ui'\nworld_visibility='hidden'").is_err());
    let root = fixture_root();
    let registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
    let definition = registry.resource("vanilla.vanilla_ui.main").unwrap();
    assert_eq!(definition.max_instances, 2);
    assert!(!definition.lifecycle_independent);
    assert_eq!(definition.presentation, PresentationMode::Exclusive);
    assert_eq!(definition.layout, UiLayout::Fullscreen);
    let path = root.join("vanilla_ui/assets/webui/registry.toml");
    let invalid = fs::read_to_string(&path)
        .unwrap()
        .replace("max_instances=2", "max_instances=0");
    fs::write(&path, invalid).unwrap();
    assert!(UiRegistry::load(&LoadedMods::discover(&root).unwrap()).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn layout_registration_validates_window_dimensions() {
    assert_eq!(
        LayoutRegistration::Windowed
            .into_layout(Some(480), Some(320))
            .unwrap(),
        UiLayout::Windowed {
            initial_width: 480,
            initial_height: 320
        }
    );
    assert!(
        LayoutRegistration::Windowed
            .into_layout(None, Some(320))
            .is_err()
    );
    assert!(
        LayoutRegistration::Windowed
            .into_layout(Some(0), Some(320))
            .is_err()
    );
    assert!(
        LayoutRegistration::Fullscreen
            .into_layout(Some(480), None)
            .is_err()
    );
}

#[test]
fn ignores_mods_without_webui_resources() {
    let root = fixture_root();
    let model_mod = root.join("vanilla_models");
    fs::create_dir_all(model_mod.join("assets")).unwrap();
    fs::write(
        model_mod.join("manifest.toml"),
        "[general]\nmod_name='vanilla_models'\nauthor='vanilla'\n",
    )
    .unwrap();

    let mods = LoadedMods::discover(&root).unwrap();
    let registry = UiRegistry::load(&mods).unwrap();

    assert_eq!(
        registry.slot(DISCONNECTED_ROOT_SLOT).unwrap().name,
        "vanilla.vanilla_ui.main"
    );
    assert!(registry.resource("vanilla.vanilla_models.main").is_none());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "windows")]
#[test]
fn webview_creation_waits_for_transactions_commands_and_a_safe_tick() {
    let mut cooldown = false;

    assert!(!claim_webview_creation_turn(1, 0, false, &mut cooldown));
    assert!(!claim_webview_creation_turn(0, 1, false, &mut cooldown));
    assert!(!claim_webview_creation_turn(0, 0, true, &mut cooldown));
    cooldown = true;
    assert!(!claim_webview_creation_turn(0, 0, true, &mut cooldown));
    assert!(cooldown, "pending commands must not consume the safe tick");
    assert!(!claim_webview_creation_turn(0, 0, false, &mut cooldown));
    assert!(claim_webview_creation_turn(0, 0, false, &mut cooldown));
}

fn vanilla_asset(path: &str) -> Option<String> {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../mods/vanilla_ui/assets/webui")
            .join(path),
    )
    .ok()
}

#[test]
fn connecting_page_closes_itself_when_the_connection_has_already_completed() {
    let Some(connecting) = vanilla_asset("connecting/index.html") else {
        return;
    };

    assert!(connecting.contains("const connectionStatus=s.status?.status"));
    assert!(connecting.contains("connectionStatus==='connected'"));
    assert!(connecting.contains("command:'ui.back'"));
    assert!(connecting.contains("<main hidden>"));
    assert!(connecting.contains("setInterval(status,1000)"));
    assert!(!connecting.contains("setInterval(status,250)"));
    assert!(!connecting.contains("roundo.hud"));
    assert!(!connecting.contains("command:'ui.open'"));
}

#[test]
fn server_selection_initializes_once_without_passive_polling() {
    let Some(selection) = vanilla_asset("server-selection/index.html") else {
        return;
    };

    assert!(selection.contains("if (loading) return"));
    assert!(selection.contains("finally { loading = false; }"));
    assert!(selection.contains("if (renderedServers === nextServers) return"));
    assert!(selection.contains("address: addressInput.value"));
    assert!(selection.ends_with("load();\n</script>\n</body>\n</html>\n"));
    assert!(!selection.contains("refresh();\nload();"));
    assert!(!selection.contains("setInterval(load"));
    let select_handler = selection
        .split("function select(index)")
        .nth(1)
        .unwrap()
        .split("async function refresh")
        .next()
        .unwrap();
    assert!(select_handler.contains("render();"));
    assert!(!select_handler.contains("load();"));
    assert!(!selection.contains("quic_addr"));
    assert!(!selection.contains("https"));
    assert!(!selection.contains("setInterval(load,250)"));
}

#[test]
fn main_menu_does_not_retain_a_command_lock_while_an_exclusive_child_is_open() {
    let Some(main_menu) = vanilla_asset("main-menu/index.html") else {
        return;
    };

    assert!(!main_menu.contains("commandPending"));
    assert!(!main_menu.contains("button.disabled"));
}

#[test]
fn vanilla_settings_asset_consumes_command_metadata_and_keeps_edits_atomic() {
    let Some(settings) = vanilla_asset("settings/index.html") else {
        return;
    };

    assert!(settings.contains("command('bindings.list',{})"));
    assert!(settings.contains("bindingModel.supported_keys"));
    assert!(settings.contains("bindingModel.supported_actions"));
    assert!(!settings.contains("const actions=['"));
    assert!(!settings.contains("const keys=['"));
    assert!(settings.contains("command('bindings.replace'"));
    assert!(settings.contains("old_binding:bindingDraft.old_binding"));
    assert!(settings.contains("bindingDraft.old_binding?'edit':'add'"));
    assert!(settings.contains("cancel.addEventListener('click'"));
    assert!(settings.contains("command('bindings.unbind'"));
    assert!(settings.contains("error.code||'command_error'"));
    assert!(settings.contains("slider.min=def.min"));
    assert!(settings.contains("number.step=def.step"));
    assert!(settings.contains("saveSetting(def,def.default)"));
}

#[test]
fn registry_resolves_initial_and_refuses_project_escape() {
    let root = fixture_root();
    let mods = LoadedMods::discover(&root).unwrap();
    let registry = UiRegistry::load(&mods).unwrap();
    assert_eq!(
        registry.slot(DISCONNECTED_ROOT_SLOT).unwrap().name,
        "vanilla.vanilla_ui.main"
    );
    assert_eq!(
        registry
            .read_asset("vanilla.vanilla_ui.main", "index.html")
            .unwrap()
            .0,
        b"ok"
    );
    assert!(
        registry
            .read_asset("vanilla.vanilla_ui.main", "../manifest.toml")
            .is_err()
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn manager_creates_distinct_children_enforces_cap_and_back_is_source_bound() {
    let root = fixture_root();
    let registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
    let mut manager = UiLifecycleManager::new(registry);
    let root_ui = manager.open_configured_root().unwrap().unwrap();
    let target = manager
        .resolve_resource_path("vanilla.vanilla_ui.main", Some("about.html"))
        .unwrap();
    let child = manager
        .open(UiCommandSource::WebView(root_ui), target)
        .unwrap();
    assert_ne!(root_ui, child);
    assert_eq!(manager.parent_instance(child), Some(root_ui));
    assert_eq!(manager.parent_instance(root_ui), None);
    assert_eq!(manager.live_count("vanilla.vanilla_ui.main"), 2);
    assert_eq!(manager.adapter_count(), 2);
    let target = manager
        .resolve_resource_path("vanilla.vanilla_ui.main", None)
        .unwrap();
    assert_eq!(
        manager.open(UiCommandSource::Host, target),
        Err(UiLifecycleError::UiInstanceLimit)
    );
    assert_eq!(manager.back(child).unwrap(), vec![child]);
    assert!(manager.instance(root_ui).is_some());
    assert_eq!(manager.live_count("vanilla.vanilla_ui.main"), 1);
    assert_eq!(manager.adapter_count(), 1);
    assert_eq!(manager.back(child), Err(UiLifecycleError::StaleUiInstance));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn duplicate_pending_open_is_rejected_but_back_allows_a_fresh_instance() {
    let root = fixture_root();
    let mut registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
    let mut settings = registry
        .resource("vanilla.vanilla_ui.main")
        .unwrap()
        .clone();
    settings.name = "vanilla.vanilla_ui.settings".into();
    settings.max_instances = 1;
    registry.resources.insert(settings.name.clone(), settings);
    let mut manager = UiLifecycleManager::new(registry);
    let root_ui = manager.open_configured_root().unwrap().unwrap();
    let target = manager
        .resolve_resource_path("vanilla.vanilla_ui.settings", None)
        .unwrap();

    let pending = manager
        .begin_open(UiCommandSource::WebView(root_ui), target.clone())
        .unwrap();
    assert_eq!(
        manager.begin_open(UiCommandSource::WebView(root_ui), target.clone()),
        Err(UiLifecycleError::DuplicatePendingOpen)
    );
    let first = manager.commit_open(pending).unwrap();
    assert_eq!(manager.live_count("vanilla.vanilla_ui.settings"), 1);

    assert_eq!(manager.back(first).unwrap(), vec![first]);
    assert_eq!(manager.live_count("vanilla.vanilla_ui.settings"), 0);
    let second = manager
        .open(UiCommandSource::WebView(root_ui), target)
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(manager.live_count("vanilla.vanilla_ui.settings"), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn presentation_hides_exclusive_branches_without_unloading_and_restores_focus() {
    let root = fixture_root();
    let mut registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
    let base = registry
        .resource("vanilla.vanilla_ui.main")
        .unwrap()
        .clone();
    let mut exclusive = base.clone();
    exclusive.name = "vanilla.vanilla_ui.exclusive".into();
    exclusive.max_instances = 4;
    exclusive.presentation = PresentationMode::Exclusive;
    let mut concurrent = base;
    concurrent.name = "vanilla.vanilla_ui.concurrent".into();
    concurrent.max_instances = 4;
    concurrent.presentation = PresentationMode::Concurrent;
    registry.resources.insert(exclusive.name.clone(), exclusive);
    registry
        .resources
        .insert(concurrent.name.clone(), concurrent);
    let mut manager = UiLifecycleManager::new(registry);
    let parent = manager.open_configured_root().unwrap().unwrap();
    let first = manager
        .open_resource(
            UiCommandSource::WebView(parent),
            "vanilla.vanilla_ui.exclusive",
            None,
        )
        .unwrap();
    let second = manager
        .open_resource(
            UiCommandSource::WebView(parent),
            "vanilla.vanilla_ui.exclusive",
            None,
        )
        .unwrap();
    assert!(!manager.instance(parent).unwrap().visible);
    assert!(!manager.instance(first).unwrap().visible);
    assert!(manager.instance(second).unwrap().visible);
    assert!(manager.instance(first).unwrap().loaded);
    assert!(manager.command_source_is_live(first));
    manager.back(second).unwrap();
    assert!(manager.instance(first).unwrap().visible);
    assert!(manager.instance(first).unwrap().loaded);
    manager.back(first).unwrap();
    let sibling = manager
        .open_resource(
            UiCommandSource::WebView(parent),
            "vanilla.vanilla_ui.concurrent",
            None,
        )
        .unwrap();
    let sibling_two = manager
        .open_resource(
            UiCommandSource::WebView(parent),
            "vanilla.vanilla_ui.concurrent",
            None,
        )
        .unwrap();
    assert!(manager.instance(sibling).unwrap().visible);
    assert!(manager.instance(sibling_two).unwrap().visible);
    assert_eq!(
        manager.interactive_instance_at(10.0, 10.0),
        Some(sibling_two)
    );
    manager.focus(parent).unwrap();
    assert_eq!(
        manager.stacking_order(),
        vec![parent, sibling, sibling_two],
        "a focused ancestor must remain physically behind concurrent descendants"
    );
    assert_eq!(
        manager.interactive_instance_at(10.0, 10.0),
        Some(sibling_two)
    );
    let nested = manager
        .open_resource(
            UiCommandSource::WebView(sibling),
            "vanilla.vanilla_ui.concurrent",
            None,
        )
        .unwrap();
    assert_eq!(
        manager.stacking_order(),
        vec![parent, sibling_two, sibling, nested],
        "the branch containing the newest focused descendant must rise as a unit"
    );
    assert_eq!(manager.interactive_instance_at(10.0, 10.0), Some(nested));
    manager.focus(sibling_two).unwrap();
    assert_eq!(
        manager.stacking_order(),
        vec![parent, sibling, nested, sibling_two]
    );
    assert_eq!(
        manager.interactive_instance_at(10.0, 10.0),
        Some(sibling_two)
    );
    assert_eq!(manager.interactive_instance_at(5000.0, 5000.0), None);
    manager.clear_focus();
    assert_eq!(manager.focused_instance(), None);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn windowed_survivor_preserves_geometry_and_adapter_identity_after_back() {
    let root = fixture_root();
    let mut registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
    let mut window = registry
        .resource("vanilla.vanilla_ui.main")
        .unwrap()
        .clone();
    window.name = "vanilla.vanilla_ui.window".into();
    window.max_instances = 4;
    window.presentation = PresentationMode::Concurrent;
    window.lifecycle_independent = true;
    window.layout = UiLayout::Windowed {
        initial_width: 480,
        initial_height: 320,
    };
    registry.resources.insert(window.name.clone(), window);
    let mut manager = UiLifecycleManager::new(registry);
    manager.set_client_bounds(1000, 800);
    let parent = manager.open_configured_root().unwrap().unwrap();
    let child = manager
        .open_resource(
            UiCommandSource::WebView(parent),
            "vanilla.vanilla_ui.window",
            None,
        )
        .unwrap();
    let bounds = UiBounds {
        x: 11,
        y: 22,
        width: 333,
        height: 444,
    };
    manager.set_bounds(child, bounds).unwrap();
    let adapter = manager.adapter_identity(child).unwrap();
    manager.back(parent).unwrap();
    assert_eq!(manager.parent_instance(child), None);
    assert_eq!(manager.instance(child).unwrap().bounds, bounds);
    assert_eq!(manager.adapter_identity(child), Some(adapter));
    assert!(manager.instance(child).unwrap().visible);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn definition_unload_destroys_targets_and_reparents_retained_independent_descendants() {
    let root = fixture_root();
    let mut registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
    let mut retained = registry
        .resource("vanilla.vanilla_ui.main")
        .unwrap()
        .clone();
    retained.name = "vanilla.vanilla_ui.retained".into();
    retained.lifecycle_independent = true;
    retained.presentation = PresentationMode::Concurrent;
    registry.resources.insert(retained.name.clone(), retained);
    let mut manager = UiLifecycleManager::new(registry);
    let parent = manager.open_configured_root().unwrap().unwrap();
    let survivor = manager
        .open_resource(
            UiCommandSource::WebView(parent),
            "vanilla.vanilla_ui.retained",
            None,
        )
        .unwrap();
    let pending_target = manager
        .resolve_resource_path("vanilla.vanilla_ui.main", None)
        .unwrap();
    let pending = manager
        .begin_open(UiCommandSource::WebView(survivor), pending_target)
        .unwrap();

    let destroyed = manager
        .unload_definitions(["vanilla.vanilla_ui.main".to_string()])
        .unwrap();
    assert_eq!(destroyed, vec![parent]);
    assert_eq!(manager.parent_instance(survivor), None);
    assert!(manager.instance(survivor).is_some());
    assert!(
        manager
            .registry
            .resource("vanilla.vanilla_ui.main")
            .is_none()
    );
    assert!(
        manager
            .registry
            .resource("vanilla.vanilla_ui.retained")
            .is_some()
    );
    assert_eq!(
        manager.commit_open(pending),
        Err(UiLifecycleError::StaleUiInstance)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn root_load_failure_activates_recovery_without_committing_or_counting() {
    let root = fixture_root();
    let registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
    let mut manager = UiLifecycleManager::new(registry);
    let pending = manager.begin_configured_root().unwrap().unwrap();
    manager.fail_open(pending, "bridge failed").unwrap();
    let recovery = manager.recovery_surface().unwrap();
    assert_eq!(recovery.lifecycle, UiLifecycleState::Disconnected);
    assert_eq!(recovery.failed_resource, "vanilla.vanilla_ui.main");
    assert_eq!(manager.live_count("vanilla.vanilla_ui.main"), 0);
    assert_eq!(manager.instances().count(), 0);
    let retry = manager.retry_recovery().unwrap();
    assert!(manager.pending_descriptor(retry).is_some());
    assert!(manager.recovery_surface().is_none());
    manager.replace_root(UiLifecycleState::Connected).unwrap();
    assert!(manager.recovery_surface().is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn prepared_candidate_survives_root_replacement_until_claimed() {
    let root = fixture_root();
    let registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
    let mut manager = UiLifecycleManager::new(registry);
    let target = manager
        .resolve_resource_path("vanilla.vanilla_ui.main", Some("about.html"))
        .unwrap();
    let prepared = manager.begin_prefetch(target.clone()).unwrap();

    manager.replace_root(UiLifecycleState::Connected).unwrap();

    assert!(manager.pending_descriptor(prepared).unwrap().prefetched);
    assert_eq!(
        manager
            .claim_prefetch(UiCommandSource::Host, &target)
            .unwrap(),
        Some(prepared)
    );
    assert!(manager.commit_open(prepared).is_ok());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pending_open_is_cancelled_by_root_replacement_and_optional_root_is_valid() {
    let root = fixture_root();
    let mut registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
    registry.slots.remove(CONNECTED_ROOT_SLOT);
    let mut manager = UiLifecycleManager::new(registry);
    let target = manager
        .resolve_resource_path("vanilla.vanilla_ui.main", None)
        .unwrap();
    let pending = manager.begin_open(UiCommandSource::Host, target).unwrap();
    manager.replace_root(UiLifecycleState::Connected).unwrap();
    assert_eq!(
        manager.commit_open(pending),
        Err(UiLifecycleError::StaleUiInstance)
    );
    assert_eq!(manager.open_configured_root().unwrap(), None);
    fs::remove_dir_all(root).unwrap();
}
