use super::*;

#[derive(Clone, Default)]
pub(super) struct TicketAuthority {
    tickets: Arc<RwLock<HashMap<String, IssuedTicket>>>,
    next_connection_id: Arc<AtomicU64>,
}

impl TicketAuthority {
    pub(super) fn issue(&self, user_session: UserSession) -> ConnectionTokenResponse {
        let expires_at = unix_timestamp() + CONNECTION_TOKEN_TTL.as_secs();
        let token = format!("{:032x}", rand::random::<u128>());
        let connection_id = ConnectionId(self.next_connection_id.fetch_add(1, Ordering::Relaxed));
        self.tickets
            .write()
            .expect("connection ticket registry lock poisoned")
            .insert(
                token.clone(),
                IssuedTicket {
                    connection_id,
                    user_session,
                    expires_at,
                },
            );
        ConnectionTokenResponse {
            connection_token: token,
            expires_at,
        }
    }

    pub(super) fn claim(&self, token: &str) -> Option<(ConnectionId, UserSession)> {
        let now = unix_timestamp();
        let mut tickets = self
            .tickets
            .write()
            .expect("connection ticket registry lock poisoned");
        tickets.retain(|_, ticket| ticket.expires_at > now);
        let ticket = tickets.remove(token)?;
        let claim = (ticket.connection_id, ticket.user_session);
        Some(claim)
    }
}

#[derive(Clone)]
struct IssuedTicket {
    connection_id: ConnectionId,
    user_session: UserSession,
    expires_at: u64,
}

#[derive(Deserialize)]
pub(super) struct ConnectionTokenResponse {
    pub(super) connection_token: String,
    pub(super) expires_at: u64,
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before UNIX_EPOCH")
        .as_secs()
}
