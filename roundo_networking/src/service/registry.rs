//! Concurrent indexes for connection routing and session fan-out.

use super::*;

#[derive(Clone, Default)]
/// Maps connection and session identities to bounded stream senders.
pub(super) struct ConnectionRegistry {
    connections: Arc<RwLock<HashMap<ConnectionId, ConnectionSenders>>>,
    sessions: Arc<RwLock<HashMap<UserSession, HashSet<ConnectionId>>>>,
    next_connection_id: Arc<AtomicU64>,
}

/// Per-connection admission channels ordered by stream priority.
struct ConnectionSenders {
    stream0: mpsc::Sender<ServerMessage>,
    stream1: mpsc::Sender<ServerMessage>,
}

impl ConnectionRegistry {
    /// Publishes a connection to the sender index and then the session index.
    ///
    /// The two separately locked maps are not updated atomically: concurrent
    /// direct routing may observe the sender before session fan-out can find it.
    pub(super) fn register(
        &self,
        connection_id: ConnectionId,
        user_session: UserSession,
        stream0: mpsc::Sender<ServerMessage>,
        stream1: mpsc::Sender<ServerMessage>,
    ) {
        self.connections_write()
            .insert(connection_id, ConnectionSenders { stream0, stream1 });
        self.sessions_write()
            .entry(user_session)
            .or_default()
            .insert(connection_id);
    }

    /// Removes a connection from both indexes.
    ///
    /// Returns whether the sender entry existed. Removal is not atomic across
    /// the separately locked maps; an index mismatch is logged.
    pub(super) fn unregister(
        &self,
        connection_id: ConnectionId,
        user_session: UserSession,
    ) -> bool {
        let removed = self.connections_write().remove(&connection_id).is_some();
        let mut sessions = self.sessions_write();
        if let Some(connection_ids) = sessions.get_mut(&user_session) {
            let session_connection_removed = connection_ids.remove(&connection_id);
            if removed != session_connection_removed {
                log::warn!(
                    "connection registry indexes diverged during removal: connection_id={}, user_id={}, session_id={}, sender_removed={removed}, session_removed={session_connection_removed}",
                    connection_id.0,
                    user_session.user_id.0,
                    user_session.session_id.0
                );
            }
            if connection_ids.is_empty() {
                let removed_session = sessions.remove(&user_session);
                debug_assert!(removed_session.is_some());
            }
        } else if removed {
            log::warn!(
                "connection registry session index was missing during removal: connection_id={}, user_id={}, session_id={}",
                connection_id.0,
                user_session.user_id.0,
                user_session.session_id.0
            );
        }
        removed
    }

    /// Attempts bounded, non-blocking admission for one connection.
    ///
    /// A missing connection and a closed writer both produce
    /// [`AdmissionResult::Closed`]. Queue success does not imply peer delivery.
    pub(super) fn send_to_connection(
        &self,
        connection_id: ConnectionId,
        stream: StreamId,
        message: ServerMessage,
    ) -> AdmissionResult {
        let connections = self.connections_read();
        let Some(senders) = connections.get(&connection_id) else {
            return AdmissionResult::Closed;
        };
        admit(senders.sender(stream), message)
    }

    /// Independently attempts non-blocking admission to each connection in a session.
    ///
    /// The returned pair is `(targets_found, messages_queued)`. Fan-out is not
    /// atomic, iteration order is unspecified, and saturated/closed recipients
    /// do not roll back successful admissions.
    pub(super) fn send_to_session(
        &self,
        user_session: UserSession,
        stream: StreamId,
        message: ServerMessage,
    ) -> (usize, usize) {
        let sessions = self.sessions_read();
        let connection_ids = sessions.get(&user_session).cloned().unwrap_or_default();
        let target_count = connection_ids.len();
        let connections = self.connections_read();
        let queued_count = connection_ids
            .into_iter()
            .filter_map(|connection_id| {
                let senders = connections.get(&connection_id);
                senders
            })
            .filter(|senders| {
                admit(senders.sender(stream), message.clone()) == AdmissionResult::Queued
            })
            .count();
        (target_count, queued_count)
    }

    /// Independently attempts non-blocking admission to every registered connection.
    ///
    /// The returned pair is `(targets_found, messages_queued)`. Fan-out is not
    /// atomic, iteration order is unspecified, and partial success is retained.
    pub(super) fn send_to_all(&self, stream: StreamId, message: ServerMessage) -> (usize, usize) {
        let connections = self.connections_read();
        let target_count = connections.len();
        let queued_count = connections
            .values()
            .filter(|senders| {
                admit(senders.sender(stream), message.clone()) == AdmissionResult::Queued
            })
            .count();
        (target_count, queued_count)
    }

    /// Allocates from a process-local wrapping counter.
    ///
    /// IDs are unique until the `u64` counter wraps; restart does not preserve
    /// identity or continue the prior sequence.
    pub(super) fn next_connection_id(&self) -> ConnectionId {
        ConnectionId(self.next_connection_id.fetch_add(1, Ordering::Relaxed))
    }

    // Poison recovery preserves service availability and records the invariant breach.
    fn connections_read(&self) -> RwLockReadGuard<'_, HashMap<ConnectionId, ConnectionSenders>> {
        let guard = self.connections.read();
        guard.unwrap_or_else(|poisoned| {
            log::error!("recovering poisoned connection sender registry read lock");
            poisoned.into_inner()
        })
    }

    fn connections_write(&self) -> RwLockWriteGuard<'_, HashMap<ConnectionId, ConnectionSenders>> {
        let guard = self.connections.write();
        guard.unwrap_or_else(|poisoned| {
            log::error!("recovering poisoned connection sender registry write lock");
            poisoned.into_inner()
        })
    }

    // Session locks use the same explicit poison-recovery policy.
    fn sessions_read(&self) -> RwLockReadGuard<'_, HashMap<UserSession, HashSet<ConnectionId>>> {
        let guard = self.sessions.read();
        guard.unwrap_or_else(|poisoned| {
            log::error!("recovering poisoned connection session registry read lock");
            poisoned.into_inner()
        })
    }

    fn sessions_write(&self) -> RwLockWriteGuard<'_, HashMap<UserSession, HashSet<ConnectionId>>> {
        let guard = self.sessions.write();
        guard.unwrap_or_else(|poisoned| {
            log::error!("recovering poisoned connection session registry write lock");
            poisoned.into_inner()
        })
    }
}

impl ConnectionSenders {
    /// Selects the bounded channel associated with a protocol stream.
    fn sender(&self, stream: StreamId) -> &mpsc::Sender<ServerMessage> {
        match stream {
            StreamId::Stream0 => &self.stream0,
            StreamId::Stream1 => &self.stream1,
        }
    }
}
