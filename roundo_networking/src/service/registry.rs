use super::ticket::TicketAuthority;
use super::*;

#[derive(Clone, Default)]
pub(super) struct ConnectionRegistry {
    connections: Arc<RwLock<HashMap<ConnectionId, ConnectionSenders>>>,
    sessions: Arc<RwLock<HashMap<UserSession, HashSet<ConnectionId>>>>,
    tickets: TicketAuthority,
}

struct ConnectionSenders {
    stream0: mpsc::UnboundedSender<ServerMessage>,
    stream1: mpsc::UnboundedSender<ServerMessage>,
}

impl ConnectionRegistry {
    pub(super) fn register(
        &self,
        connection_id: ConnectionId,
        user_session: UserSession,
        stream0: mpsc::UnboundedSender<ServerMessage>,
        stream1: mpsc::UnboundedSender<ServerMessage>,
    ) {
        self.connections
            .write()
            .expect("connection sender registry lock poisoned")
            .insert(connection_id, ConnectionSenders { stream0, stream1 });
        self.sessions
            .write()
            .expect("connection session registry lock poisoned")
            .entry(user_session)
            .or_default()
            .insert(connection_id);
    }

    pub(super) fn unregister(
        &self,
        connection_id: ConnectionId,
        user_session: UserSession,
    ) -> bool {
        let removed = self
            .connections
            .write()
            .expect("connection sender registry lock poisoned")
            .remove(&connection_id)
            .is_some();
        let mut sessions = self
            .sessions
            .write()
            .expect("connection session registry lock poisoned");
        if let Some(connection_ids) = sessions.get_mut(&user_session) {
            connection_ids.remove(&connection_id);
            if connection_ids.is_empty() {
                sessions.remove(&user_session);
            }
        }
        removed
    }

    pub(super) fn send_to_connection(
        &self,
        connection_id: ConnectionId,
        stream: StreamId,
        message: ServerMessage,
    ) -> bool {
        self.connections
            .read()
            .expect("connection sender registry lock poisoned")
            .get(&connection_id)
            .is_some_and(|senders| senders.sender(stream).send(message).is_ok())
    }

    pub(super) fn send_to_session(
        &self,
        user_session: UserSession,
        stream: StreamId,
        message: ServerMessage,
    ) -> (usize, usize) {
        let connection_ids = self
            .sessions
            .read()
            .expect("connection session registry lock poisoned")
            .get(&user_session)
            .cloned()
            .unwrap_or_default();
        let target_count = connection_ids.len();
        let connections = self
            .connections
            .read()
            .expect("connection sender registry lock poisoned");
        let queued_count = connection_ids
            .into_iter()
            .filter_map(|connection_id| connections.get(&connection_id))
            .filter(|senders| senders.sender(stream).send(message.clone()).is_ok())
            .count();
        (target_count, queued_count)
    }

    pub(super) fn send_to_all(&self, stream: StreamId, message: ServerMessage) -> (usize, usize) {
        let connections = self
            .connections
            .read()
            .expect("connection sender registry lock poisoned");
        let target_count = connections.len();
        let queued_count = connections
            .values()
            .filter(|senders| senders.sender(stream).send(message.clone()).is_ok())
            .count();
        (target_count, queued_count)
    }

    pub(super) fn tickets(&self) -> TicketAuthority {
        self.tickets.clone()
    }
}

impl ConnectionSenders {
    fn sender(&self, stream: StreamId) -> &mpsc::UnboundedSender<ServerMessage> {
        match stream {
            StreamId::Stream0 => &self.stream0,
            StreamId::Stream1 => &self.stream1,
        }
    }
}
