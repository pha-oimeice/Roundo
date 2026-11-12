//! Change-driven publication of client-owned subscribed data.

use super::*;
use std::collections::BTreeMap;

/// Per-resource accepted snapshots and source-specific publication progress.
#[derive(Resource, Default)]
pub(super) struct ClientDataSyncState {
    published_generations: BTreeMap<&'static str, u64>,
    server_list_revision: Option<u64>,
    hud_elapsed_secs: f32,
}

impl ClientDataSyncState {
    fn publish_snapshot(
        &mut self,
        sink: &mut roundo_ui::UiData,
        resource: &'static str,
        snapshot: serde_json::Value,
    ) -> bool {
        if sink.publish_data(resource, &snapshot) == 0 {
            return false;
        }
        self.published_generations
            .insert(resource, sink.data_subscription_generation());
        true
    }

    fn needs_subscription_snapshot(
        &self,
        sink: &roundo_ui::UiData,
        resource: &'static str,
    ) -> bool {
        let generation = sink.data_subscription_generation();
        self.published_generations.get(resource) != Some(&generation)
    }
}

/// Processes only the exact source revision whose snapshot is accepted or unchanged.
/// An unaccepted subscription snapshot remains eligible on the next host update.
pub(super) fn sync_server_list_data(
    config: &ClientConfigStore,
    sync: &mut ClientDataSyncState,
    sink: &mut roundo_ui::UiData,
) {
    if !sink.has_data_subscribers(SERVER_LIST_DATA) {
        return;
    }
    let since = if sync.needs_subscription_snapshot(sink, SERVER_LIST_DATA) {
        None
    } else {
        sync.server_list_revision
    };
    let Some(snapshot) = config.server_list_snapshot(since) else {
        return;
    };
    let data = serde_json::to_value(snapshot.data).expect("server list snapshot serializes");
    if sync.publish_snapshot(sink, SERVER_LIST_DATA, data) {
        sync.server_list_revision = Some(snapshot.revision);
    }
}

pub(super) fn update_hud_cache(
    time: Res<Time>,
    controller: Res<ClientPlayerController>,
    cameras: Query<&Transform>,
    mut hud: ResMut<HudCache>,
) {
    hud.record_frame(time.delta_secs());
    hud.position = controller
        .camera()
        .and_then(|entity| {
            let camera = cameras.get(entity);
            camera.ok()
        })
        .map(|transform| {
            let translation = transform.translation;
            HudPosition {
                absolute: [translation.x, translation.y, translation.z],
                chunk_relative: [
                    translation.x.floor() as i32,
                    translation.y.floor() as i32,
                    translation.z.floor() as i32,
                ],
            }
        });
}

/// Client-owned producers publish directly into the native ECS snapshot store.
/// Live view declarations determine subscriptions; no UI data IPC remains.
pub(super) fn sync_subscribed_client_data(
    time: Res<Time>,
    config: Res<ClientConfigStore>,
    network: Res<crate::network::ClientNetworkManager>,
    hud: Res<HudCache>,
    access: Res<roundo_marionette::ClientPlayerControllerAccess>,
    input_registry: Res<InputFunctionRegistry>,
    mut sync: ResMut<ClientDataSyncState>,
    mut data: Option<ResMut<roundo_ui::UiData>>,
) {
    let Some(data) = data.as_deref_mut() else {
        return;
    };
    let subscribed = data.has_data_subscribers("client.inventory");
    let inventory = network.inventory_view(
        subscribed
            .then(|| access.active_movement_binding())
            .flatten(),
    );
    if subscribed {
        let snapshot = serde_json::to_value(inventory).expect("inventory snapshot serializes");
        sync.publish_snapshot(data, "client.inventory", snapshot);
    }
    sync_client_data_update(
        time.delta_secs(),
        &config,
        &network,
        &hud,
        &input_registry,
        &mut sync,
        data,
    );
}

fn sync_client_data_update(
    delta_secs: f32,
    config: &ClientConfigStore,
    network: &crate::network::ClientNetworkManager,
    hud: &HudCache,
    input_registry: &InputFunctionRegistry,
    sync: &mut ClientDataSyncState,
    sink: &mut roundo_ui::UiData,
) {
    sync_server_list_data(config, sync, sink);
    if sink.has_data_subscribers(SERVER_STATUS_DATA) {
        let snapshot = serde_json::to_value(server_status_output(network.status()))
            .expect("server status snapshot serializes");
        sync.publish_snapshot(sink, SERVER_STATUS_DATA, snapshot);
    }
    if sink.has_data_subscribers("client.debug") {
        let snapshot =
            serde_json::to_value(network.debug_status()).expect("diagnostic status serializes");
        sync.publish_snapshot(sink, "client.debug", snapshot);
    }
    if sink.has_data_subscribers(CONTROL_STATUS_DATA) {
        let snapshot =
            serde_json::to_value(control_status_output(network.control_acquire_statuses()))
                .expect("control status snapshot serializes");
        sync.publish_snapshot(sink, CONTROL_STATUS_DATA, snapshot);
    }
    if sink.has_data_subscribers(SETTINGS_DATA) {
        let snapshot = serde_json::to_value(settings_show_output(config))
            .expect("settings snapshot serializes");
        sync.publish_snapshot(sink, SETTINGS_DATA, snapshot);
    }
    if sink.has_data_subscribers(BINDINGS_DATA) {
        let snapshot = serde_json::to_value(bindings_list_output(config, input_registry))
            .expect("bindings snapshot serializes");
        sync.publish_snapshot(sink, BINDINGS_DATA, snapshot);
    }

    sync.hud_elapsed_secs += delta_secs;
    if sink.has_data_subscribers(HUD_DATA)
        && (sync.needs_subscription_snapshot(sink, HUD_DATA)
            || sync.hud_elapsed_secs >= HUD_SYNC_INTERVAL_SECS)
    {
        sync.hud_elapsed_secs = 0.0;
        match hud_show_output(hud).and_then(serde_json::to_value) {
            Ok(snapshot) => {
                sync.publish_snapshot(sink, HUD_DATA, snapshot);
            }
            Err(error) => log::error!("cannot serialize HUD Client Data: {error}"),
        }
    }
}
