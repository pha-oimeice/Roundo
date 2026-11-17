//! Bevy fixed-update integration for explicit publications.
//!
//! Both endpoints use the same compiled data definitions and derive their wire
//! IDs from TypeId. No cross-build identity compatibility is maintained.
//! Component interfaces require Component + DataTransferObject; resource
//! interfaces require Resource + DataTransferObject. Typed reception additionally requires owned
//! deserialization and decodes the queued bytes as the requested T.
//! There is no type registry, receive allowlist, duplicate DTO, change
//! monitoring, entity lifecycle protocol, or automatic World insertion.
//!
//! Install SharedEntities separately on the authoritative World with an explicit
//! first ID. Attach an already handshaken SyncConnection to Network; endpoint,
//! TLS, runtime and transport message-limit configuration remain host-owned.
//!
//! Plugin systems participate in FixedUpdate without forcing an order between
//! reception, business systems and flushing. NetworkSystems labels and public
//! system functions let the host express that order later. FixedFirst only
//! counts the enclosing Bevy fixed cycles; it is not another network clock.
//! sync_* encodes before returning and never waits for network I/O. Each call
//! supplies Option<Deadline>: None omits the time-based trigger, while Some
//! uses a positive fixed-tick interval. Deadline::default() is one fixed tick.
//! Flush triggers are optional, opt-in application batching policy,
//! not hard capacity limits or changes to QUIC flow control. Background outcomes
//! are available through Network::pop_event.

use crate::{BatchPolicy, Network};
use bevy::prelude::{App, FixedFirst, FixedUpdate, IntoScheduleConfigs, Plugin, ResMut, SystemSet};
use tokio::runtime::Handle;

/// Ordering labels only: no before/after relationship is imposed by the Plugin.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum NetworkSystems {
    Receive,
    Flush,
}

/// May be added only once per App. Bevy checks plugin identity before build;
/// the presence of a Network resource is not a duplicate-plugin condition.
/// Default uses the current host-owned runtime handle and the
/// default batching policy (byte/count triggers disabled).
///
/// # Panics
/// Default construction panics outside an entered Tokio runtime context.
pub struct NetworkPlugin {
    runtime: Handle,
    policy: BatchPolicy,
}

impl NetworkPlugin {
    pub fn new(runtime: Handle, policy: BatchPolicy) -> Self {
        Self { runtime, policy }
    }
}

impl Default for NetworkPlugin {
    fn default() -> Self {
        Self {
            runtime: Handle::current(),
            policy: Default::default(),
        }
    }
}

impl Plugin for NetworkPlugin {
    fn build(&self, app: &mut App) {
        // Default reads the current handle; enter the configured runtime so
        // explicit Plugin construction still works outside a Tokio context.
        let _runtime = self.runtime.enter();
        let mut network = Network::default();
        network.policy = self.policy;
        app.insert_resource(network)
            .add_systems(FixedFirst, advance_network_fixed_tick)
            .add_systems(
                FixedUpdate,
                (
                    receive_network_updates.in_set(NetworkSystems::Receive),
                    flush_network.in_set(NetworkSystems::Flush),
                ),
            );
    }

    fn is_unique(&self) -> bool {
        true
    }
}

fn advance_network_fixed_tick(mut network: ResMut<Network>) {
    network.advance_fixed_tick();
}

/// Nonblocking ingestion of background results into encoded receive queues.
pub fn receive_network_updates(mut network: ResMut<Network>) {
    network.poll();
}

/// Initiates due application batches without awaiting network I/O.
pub fn flush_network(mut network: ResMut<Network>) {
    network.flush_due();
}
