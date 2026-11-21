//! Connection routes, attachment and worker ownership.

use super::{
    Network,
    batching::Pending,
    worker::{spawn_receiver, spawn_sender},
};
use crate::{SyncConnection, wire::SyncBatch};
use std::{
    collections::BinaryHeap,
    hash::{Hash, Hasher},
    net::SocketAddr,
    sync::Arc,
};
use tokio::{sync::mpsc, task::JoinHandle};

/// An opaque route to one attached physical connection, not a player identity
/// or an authorization grant. Reattaching creates a different route, including
/// when the remote socket address is unchanged. Cloning retains only the route.
#[derive(Clone, Debug)]
pub struct Peer(Arc<PeerIdentity>);

#[derive(Debug)]
struct PeerIdentity {
    initial_address: SocketAddr,
}

impl Peer {
    /// Diagnostic address captured at attachment; not used as identity.
    pub fn initial_address(&self) -> SocketAddr {
        self.0.initial_address
    }
}
impl PartialEq for Peer {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for Peer {}
impl Hash for Peer {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}

pub(super) struct PeerState {
    pub(super) limit: usize,
    pub(super) pending: BinaryHeap<Pending>,
    pub(super) payload_bytes: usize,
    pub(super) outgoing: mpsc::UnboundedSender<Vec<SyncBatch>>,
    send_task: JoinHandle<()>,
    receive_task: JoinHandle<()>,
}
impl Drop for PeerState {
    fn drop(&mut self) {
        // Request asynchronous cancellation when the World drops/detaches this
        // route. Workers own their buffers and never retain ECS references.
        // A partial send keeps the existing close-on-interrupted-write behavior.
        self.send_task.abort();
        self.receive_task.abort();
    }
}

impl Network {
    /// Hands an already connected, schema-compatible transport to the Plugin.
    /// The host retains responsibility for runtime, TLS and endpoint setup.
    /// Do not concurrently consume the connection through external clones.
    pub fn attach(&mut self, connection: SyncConnection) -> Peer {
        let peer = Peer(Arc::new(PeerIdentity {
            initial_address: connection.peer_addr(),
        }));
        let limit = connection.message_limit().bytes();
        let (outgoing, batches) = mpsc::unbounded_channel::<Vec<SyncBatch>>();
        let send_task = spawn_sender(
            &self.runtime,
            connection.clone(),
            peer.clone(),
            self.worker_events.clone(),
            batches,
        );
        let receive_task = spawn_receiver(
            &self.runtime,
            connection,
            peer.clone(),
            self.worker_events.clone(),
        );
        self.peers.insert(
            peer.clone(),
            PeerState {
                limit,
                pending: BinaryHeap::new(),
                payload_bytes: 0,
                outgoing,
                send_task,
                receive_task,
            },
        );
        peer
    }

    /// Drops this route's pending send queue and requests worker cancellation.
    /// Already received data/outcomes remain readable with their original Peer;
    /// that retired route cannot address a subsequently attached connection.
    /// This is not an entity/shared-identity notification.
    pub fn detach(&mut self, peer: &Peer) -> bool {
        self.peers.remove(peer).is_some()
    }

    pub fn peers(&self) -> impl Iterator<Item = &Peer> {
        self.peers.keys()
    }
}
