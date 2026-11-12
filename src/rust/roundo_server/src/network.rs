//! Binary-owned configuration and persistent session provider for the shared server bridge.
use log::debug;
use roundo_contracts::{SessionId, UserId, UserSession};
use roundo_game_network_bridge::ServerSessionProvider;
use roundo_networking::{HookFuture, PublicSession, ServerNetworkConfig};
use std::path::PathBuf;

pub use roundo_game_network_bridge::ServerNetworkRuntime;

pub fn network_config() -> ServerNetworkConfig {
    let network = &crate::config::SERVER_CONFIG.network;
    ServerNetworkConfig {
        quic_address: network.endpoint.get_quic_addr(),
        certificate_directory: PathBuf::from(&network.certificate_path).join("certs"),
        server_alternative_names: network.server_alternative_names.clone(),
        generate_self_signed_certificate: network.generate_self_signed_certificate,
        outbound: roundo_networking::OutboundQueuePolicy::default(),
    }
}

pub struct DatabaseSessionProvider;

impl ServerSessionProvider for DatabaseSessionProvider {
    fn initialize(&self) -> HookFuture<()> {
        Box::pin(async {
            debug!("Initializing database before accepting network connections");
            let result = crate::my_db::initialize_database()
                .await
                .map_err(|error| error.to_string());
            if result.is_ok() {
                debug!("Database initialization completed");
            }
            result
        })
    }

    fn public_session(&self) -> HookFuture<PublicSession> {
        Box::pin(async {
            let session = crate::my_db::game_session::public_session()
                .await
                .map_err(|error| error.to_string())?;
            Ok(PublicSession {
                user_session: UserSession {
                    user_id: UserId(session.owner_user_id),
                    session_id: SessionId(session.id),
                },
                session_name: session.name,
            })
        })
    }

    fn session_is_open(&self, session_id: SessionId) -> HookFuture<bool> {
        Box::pin(async move {
            crate::my_db::game_session::is_session_open(session_id.0)
                .await
                .map_err(|error| error.to_string())
        })
    }
}
