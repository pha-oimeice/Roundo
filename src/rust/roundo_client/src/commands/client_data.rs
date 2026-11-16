//! Change-driven publication of client-owned subscribed data.

use super::*;
use std::collections::BTreeMap;

/// Per-resource accepted snapshots and source-specific publication progress.
#[derive(Resource, Default)]
pub(super) struct ClientDataSyncState {
    published_generations: BTreeMap<&'static str, u64>,
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
    hud: Res<HudCache>,
    input_registry: Res<InputFunctionRegistry>,
    mut sync: ResMut<ClientDataSyncState>,
    mut data: Option<ResMut<roundo_ui::UiData>>,
) {
    let Some(data) = data.as_deref_mut() else {
        return;
    };
    sync_client_data_update(
        time.delta_secs(),
        &config,
        &hud,
        &input_registry,
        &mut sync,
        data,
    );
}

fn sync_client_data_update(
    delta_secs: f32,
    config: &ClientConfigStore,
    hud: &HudCache,
    input_registry: &InputFunctionRegistry,
    sync: &mut ClientDataSyncState,
    sink: &mut roundo_ui::UiData,
) {
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
