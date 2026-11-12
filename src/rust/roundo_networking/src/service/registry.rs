//! Process-unique physical identities and business-queue routing, not byte flow control.
use super::*;

#[derive(Clone, Default)]
pub(super) struct ConnectionRegistry {
    connections: Arc<RwLock<HashMap<ConnectionId, [mpsc::Sender<ServerMessage>; 2]>>>,
    sessions: Arc<RwLock<HashMap<UserSession, HashSet<ConnectionId>>>>,
}
impl ConnectionRegistry {
    pub(super) fn next_connection_id(&self) -> Option<ConnectionId> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .ok()
            .map(ConnectionId)
    }
    pub(super) fn register(
        &self,
        id: ConnectionId,
        session: UserSession,
        senders: [mpsc::Sender<ServerMessage>; 2],
    ) {
        self.connections_write().insert(id, senders);
        self.sessions_write().entry(session).or_default().insert(id);
    }
    pub(super) fn unregister(&self, id: ConnectionId, session: UserSession) -> bool {
        let removed = self.connections_write().remove(&id).is_some();
        let mut sessions = self.sessions_write();
        if let Some(ids) = sessions.get_mut(&session) {
            if ids.remove(&id) != removed {
                log::error!("connection routing indexes diverged: {}", id.0);
            }
            if ids.is_empty() {
                sessions.remove(&session);
            }
        } else if removed {
            log::error!("connection session index missing: {}", id.0);
        }
        removed
    }
    pub(super) fn try_send_to_connection(
        &self,
        id: ConnectionId,
        class: TrafficClass,
        message: ServerMessage,
    ) -> Result<(), mpsc::error::TrySendError<ServerMessage>> {
        let connections = self.connections_read();
        let Some(senders) = connections.get(&id) else {
            return Err(mpsc::error::TrySendError::Closed(message));
        };
        senders[class as usize].try_send(message)
    }
    pub(super) fn send_to_session(
        &self,
        session: UserSession,
        class: TrafficClass,
        message: ServerMessage,
    ) -> (usize, usize) {
        let ids = self
            .sessions_read()
            .get(&session)
            .cloned()
            .unwrap_or_default();
        let targets = ids.len();
        let connections = self.connections_read();
        let queued = ids
            .iter()
            .filter_map(|id| connections.get(id))
            .filter(|senders| {
                admit(&senders[class as usize], message.clone()) == AdmissionResult::Queued
            })
            .count();
        (targets, queued)
    }
    pub(super) fn send_to_all(
        &self,
        class: TrafficClass,
        message: ServerMessage,
    ) -> (usize, usize) {
        let connections = self.connections_read();
        let queued = connections
            .values()
            .filter(|senders| {
                admit(&senders[class as usize], message.clone()) == AdmissionResult::Queued
            })
            .count();
        (connections.len(), queued)
    }
    fn connections_read(
        &self,
    ) -> RwLockReadGuard<'_, HashMap<ConnectionId, [mpsc::Sender<ServerMessage>; 2]>> {
        self.connections.read().unwrap_or_else(|p| {
            log::error!("poisoned connection registry");
            p.into_inner()
        })
    }
    fn connections_write(
        &self,
    ) -> RwLockWriteGuard<'_, HashMap<ConnectionId, [mpsc::Sender<ServerMessage>; 2]>> {
        self.connections.write().unwrap_or_else(|p| {
            log::error!("poisoned connection registry");
            p.into_inner()
        })
    }
    fn sessions_read(&self) -> RwLockReadGuard<'_, HashMap<UserSession, HashSet<ConnectionId>>> {
        self.sessions.read().unwrap_or_else(|p| {
            log::error!("poisoned session registry");
            p.into_inner()
        })
    }
    fn sessions_write(&self) -> RwLockWriteGuard<'_, HashMap<UserSession, HashSet<ConnectionId>>> {
        self.sessions.write().unwrap_or_else(|p| {
            log::error!("poisoned session registry");
            p.into_inner()
        })
    }
}
