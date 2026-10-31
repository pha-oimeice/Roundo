//! Change-driven publication of client-owned subscribed data.

use super::*;
use std::collections::BTreeMap;

/// Per-resource accepted snapshots and source-specific publication progress.
#[derive(Resource, Default)]
pub(super) struct ClientDataSyncState {
    accepted_snapshots: BTreeMap<&'static str, AcceptedClientData>,
    server_list_revision: Option<u64>,
    hud_elapsed_secs: f32,
}

struct AcceptedClientData {
    snapshot: serde_json::Value,
    subscription_generation: u64,
}

enum PublishOutcome {
    Accepted,
    Unchanged,
    NotAccepted,
}

impl ClientDataSyncState {
    fn publish_snapshot(
        &mut self,
        sink: &mut impl ClientDataSink,
        resource: &'static str,
        snapshot: serde_json::Value,
    ) -> PublishOutcome {
        if !sink.has_subscribers(resource) {
            return PublishOutcome::NotAccepted;
        }
        let subscription_generation = sink.subscription_generation();
        if self
            .accepted_snapshots
            .get(resource)
            .is_some_and(|accepted| {
                accepted.subscription_generation == subscription_generation
                    && accepted.snapshot == snapshot
            })
        {
            return PublishOutcome::Unchanged;
        }
        if sink.publish(resource, &snapshot) == 0 {
            return PublishOutcome::NotAccepted;
        }
        self.accepted_snapshots.insert(
            resource,
            AcceptedClientData {
                snapshot,
                subscription_generation,
            },
        );
        PublishOutcome::Accepted
    }

    fn needs_subscription_snapshot(
        &self,
        sink: &impl ClientDataSink,
        resource: &'static str,
    ) -> bool {
        let generation = sink.subscription_generation();
        self.accepted_snapshots
            .get(resource)
            .is_none_or(|accepted| accepted.subscription_generation != generation)
    }
}

/// Host snapshot delivery through instance-scoped subscriptions.
pub(super) trait ClientDataSink {
    fn subscription_generation(&self) -> u64;
    fn has_subscribers(&self, resource: &str) -> bool;
    /// Counts accepted submissions, not completed native callbacks or page receipts.
    fn publish(&mut self, resource: &str, snapshot: &serde_json::Value) -> usize;
}

impl ClientDataSink for roundo_webui::UiNavigationExecutor {
    fn subscription_generation(&self) -> u64 {
        self.data_subscription_generation()
    }

    fn has_subscribers(&self, resource: &str) -> bool {
        self.has_data_subscribers(resource)
    }

    fn publish(&mut self, resource: &str, snapshot: &serde_json::Value) -> usize {
        self.publish_data(resource, snapshot)
    }
}

/// Processes only the exact source revision whose snapshot is accepted or unchanged.
/// An unaccepted subscription snapshot remains eligible on the next host update.
pub(super) fn sync_server_list_data(
    config: &ClientConfigStore,
    sync: &mut ClientDataSyncState,
    sink: &mut impl ClientDataSink,
) {
    if !sink.has_subscribers(SERVER_LIST_DATA) {
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
    match sync.publish_snapshot(sink, SERVER_LIST_DATA, data) {
        PublishOutcome::Accepted | PublishOutcome::Unchanged => {
            sync.server_list_revision = Some(snapshot.revision);
        }
        PublishOutcome::NotAccepted => {}
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

/// Client Data is produced only on the client side. WebViews merely maintain
/// subscriptions; this system chooses event-driven or periodic synchronization
/// independently for each resource and never receives read requests over IPC.
pub(super) fn sync_subscribed_client_data(
    time: Res<Time>,
    config: Res<ClientConfigStore>,
    network: Res<crate::network::ClientNetworkManager>,
    hud: Res<HudCache>,
    access: Res<roundo_marionette::ClientPlayerControllerAccess>,
    input_registry: Option<Res<InputRegistry>>,
    mut sync: ResMut<ClientDataSyncState>,
    mut navigation: Option<bevy::ecs::system::NonSendMut<roundo_webui::UiNavigationExecutor>>,
) {
    let Some(navigation) = navigation.as_deref_mut() else {
        return;
    };
    let subscribed = navigation.has_data_subscribers("client.inventory");
    let inventory = network.inventory_view(
        subscribed
            .then(|| access.active_movement_binding())
            .flatten(),
    );
    if subscribed {
        let snapshot = serde_json::to_value(inventory).expect("inventory snapshot serializes");
        sync.publish_snapshot(navigation, "client.inventory", snapshot);
    }
    sync_client_data_update(
        time.delta_secs(),
        &config,
        &network,
        &hud,
        input_registry.as_deref(),
        &mut sync,
        navigation,
    );
}

fn sync_client_data_update(
    delta_secs: f32,
    config: &ClientConfigStore,
    network: &crate::network::ClientNetworkManager,
    hud: &HudCache,
    input_registry: Option<&InputRegistry>,
    sync: &mut ClientDataSyncState,
    sink: &mut impl ClientDataSink,
) {
    sync_server_list_data(config, sync, sink);
    if sink.has_subscribers(SERVER_STATUS_DATA) {
        let snapshot = serde_json::to_value(server_status_output(network.status()))
            .expect("server status snapshot serializes");
        sync.publish_snapshot(sink, SERVER_STATUS_DATA, snapshot);
    }
    if sink.has_subscribers("client.debug") {
        let snapshot =
            serde_json::to_value(network.debug_status()).expect("diagnostic status serializes");
        sync.publish_snapshot(sink, "client.debug", snapshot);
    }
    if sink.has_subscribers(CONTROL_STATUS_DATA) {
        let snapshot =
            serde_json::to_value(control_status_output(network.control_acquire_statuses()))
                .expect("control status snapshot serializes");
        sync.publish_snapshot(sink, CONTROL_STATUS_DATA, snapshot);
    }
    if sink.has_subscribers(SETTINGS_DATA) {
        let snapshot = serde_json::to_value(settings_show_output(config))
            .expect("settings snapshot serializes");
        sync.publish_snapshot(sink, SETTINGS_DATA, snapshot);
    }
    if sink.has_subscribers(BINDINGS_DATA) {
        let snapshot = serde_json::to_value(bindings_list_output(config, input_registry))
            .expect("bindings snapshot serializes");
        sync.publish_snapshot(sink, BINDINGS_DATA, snapshot);
    }

    sync.hud_elapsed_secs += delta_secs;
    if sink.has_subscribers(HUD_DATA)
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
