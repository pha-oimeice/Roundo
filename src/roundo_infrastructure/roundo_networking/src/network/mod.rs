//! World-owned synchronization state and its implementation modules.

mod batching;
mod peer;
mod receive;
mod statistics;
mod submit;
mod worker;

pub use peer::Peer;
pub use receive::{ComponentUpdate, NetworkEvent, ResourceUpdate};
pub use statistics::LengthStatistics;

use self::{peer::PeerState, receive::Received, worker::WorkerEvent};
use crate::{BatchPolicy, NetworkTypeId, UpdateKind};
use bevy::prelude::Resource;
use crossbeam::channel::{Receiver, Sender, unbounded};
use std::collections::{HashMap, VecDeque};
use tokio::runtime::Handle;

type ReceiveKey = (NetworkTypeId, UpdateKind);

/// Application-layer synchronization owned by one Bevy World.
///
/// `sync_*` encodes immediately. Accepted buffers never borrow ECS storage;
/// all destinations share the same immutable Arc-backed payload. There are no
/// capacity budgets, implicit retries, component monitoring, lifecycle messages,
/// or automatic writes to the receiving World.
#[derive(Resource)]
pub struct Network {
    runtime: Handle,
    pub(crate) policy: BatchPolicy,
    peers: HashMap<Peer, PeerState>,
    incoming: Receiver<WorkerEvent>,
    worker_events: Sender<WorkerEvent>,
    received: HashMap<ReceiveKey, VecDeque<Received>>,
    events: VecDeque<NetworkEvent>,
    statistics: HashMap<NetworkTypeId, LengthStatistics>,
    tick: u64,
    sequence: u64,
}

impl Default for Network {
    /// Initializes empty queues using the current host-owned Tokio runtime.
    ///
    /// # Panics
    /// Panics when called outside an entered Tokio runtime context.
    fn default() -> Self {
        let (worker_events, incoming) = unbounded();
        Self {
            runtime: Handle::current(),
            policy: Default::default(),
            peers: Default::default(),
            incoming,
            worker_events,
            received: Default::default(),
            events: Default::default(),
            statistics: Default::default(),
            tick: Default::default(),
            sequence: Default::default(),
        }
    }
}

impl Network {
    /// Number of Bevy fixed cycles observed, not a separate network clock.
    pub fn fixed_tick(&self) -> u64 {
        self.tick
    }

    pub(crate) fn advance_fixed_tick(&mut self) {
        self.tick = self
            .tick
            .checked_add(1)
            .expect("network FixedUpdate tick exhausted");
    }
}
