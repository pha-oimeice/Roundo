use roundo_networking::protocol::{
    ClientGameMessage, ConnectionId, ControllerId, ServerGameMessage, SessionId, UserId,
    UserSession,
};
use roundo_networking::{
    CertificatePolicy, ClientHooks, ClientNetwork, ClientNetworkConfig, HookFuture, PublicSession,
    ServerHooks, ServerNetwork, ServerNetworkConfig,
};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TEST_TIMEOUT: Duration = Duration::from_secs(3);

#[tokio::test]
async fn client_and_server_exchange_game_messages_over_tls() {
    let (connected_sender, connected_receiver) = channel();
    let (client_message_sender, client_message_receiver) = channel();
    let certificate_directory = test_certificate_directory();
    let server = ServerNetwork::start(
        server_config(certificate_directory.clone()),
        Arc::new(TestServerHooks {
            connected_sender,
            client_message_sender,
        }),
    )
    .expect("network server should start");
    let addresses = server.addresses();
    let (server_message_sender, server_message_receiver) = channel();
    let (connection_error_sender, connection_error_receiver) = channel();
    let client = ClientNetwork::start(
        ClientNetworkConfig {
            game_address: addresses.game_address,
            public_address: addresses.public_address,
            server_name: "localhost".to_string(),
            certificate_policy: CertificatePolicy::TrustOnFirstUse,
            reconnect_delay: Duration::from_millis(10),
        },
        Arc::new(TestClientHooks {
            server_message_sender,
            connection_error_sender,
        }),
    )
    .expect("network client should start");

    let (connection_id, user_session) =
        receive_or_connection_error(connected_receiver, connection_error_receiver).await;
    assert_eq!(user_session, test_session());

    let request = ClientGameMessage::RequestController {
        controller_id: ControllerId(7),
    };
    client
        .send(request.clone())
        .expect("connected client should send game message");
    let (received_connection_id, received_session, received_message) =
        receive(client_message_receiver).await;
    assert_eq!(received_connection_id, connection_id);
    assert_eq!(received_session, test_session());
    assert_eq!(received_message, request);

    let outbound = ServerGameMessage::ControllerRevoked {
        controller_id: ControllerId(7),
    };
    assert!(server.send_to_connection(connection_id, outbound.clone()));
    assert_eq!(receive(server_message_receiver).await, outbound);

    client.shutdown();
    server.shutdown();
    let _ = std::fs::remove_dir_all(certificate_directory);
}

struct TestServerHooks {
    connected_sender: Sender<(ConnectionId, UserSession)>,
    client_message_sender: Sender<(ConnectionId, UserSession, ClientGameMessage)>,
}

impl ServerHooks for TestServerHooks {
    fn initialize(&self) -> HookFuture<()> {
        Box::pin(async { Ok(()) })
    }

    fn public_session(&self) -> HookFuture<PublicSession> {
        Box::pin(async {
            Ok(PublicSession {
                user_session: test_session(),
                session_name: Some("integration-test".to_string()),
            })
        })
    }

    fn session_is_open(&self, session_id: SessionId) -> HookFuture<bool> {
        Box::pin(async move { Ok(session_id == test_session().session_id) })
    }

    fn on_session_connected(&self, connection_id: ConnectionId, user_session: UserSession) {
        let _ = self.connected_sender.send((connection_id, user_session));
    }

    fn on_client_game_message(
        &self,
        connection_id: ConnectionId,
        user_session: UserSession,
        message: ClientGameMessage,
    ) {
        let _ = self
            .client_message_sender
            .send((connection_id, user_session, message));
    }
}

struct TestClientHooks {
    server_message_sender: Sender<ServerGameMessage>,
    connection_error_sender: Sender<String>,
}

impl ClientHooks for TestClientHooks {
    fn on_server_game_message(&self, message: ServerGameMessage) {
        let _ = self.server_message_sender.send(message);
    }

    fn on_connection_error(&self, error: &roundo_networking::NetworkError) {
        let _ = self.connection_error_sender.send(error.to_string());
    }
}

async fn receive<T: Send + 'static>(receiver: Receiver<T>) -> T {
    tokio::task::spawn_blocking(move || {
        receiver
            .recv_timeout(TEST_TIMEOUT)
            .expect("network event should arrive before the test timeout")
    })
    .await
    .expect("network event receiver should not panic")
}

async fn receive_or_connection_error(
    connected_receiver: Receiver<(ConnectionId, UserSession)>,
    error_receiver: Receiver<String>,
) -> (ConnectionId, UserSession) {
    tokio::task::spawn_blocking(move || {
        for _ in 0..3 {
            if let Ok(connection) = connected_receiver.recv_timeout(Duration::from_secs(1)) {
                return connection;
            }
            if let Ok(error) = error_receiver.try_recv() {
                panic!("network client failed to connect: {error}");
            }
        }
        panic!("network client did not complete its TLS session")
    })
    .await
    .expect("connection wait task should not panic")
}

fn server_config(certificate_directory: PathBuf) -> ServerNetworkConfig {
    ServerNetworkConfig {
        game_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        public_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        certificate_directory,
        server_alternative_names: vec!["localhost".to_string()],
        generate_self_signed_certificate: true,
    }
}

fn test_session() -> UserSession {
    UserSession {
        user_id: UserId(42),
        session_id: SessionId(7),
    }
}

fn test_certificate_directory() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after UNIX_EPOCH")
        .as_nanos();
    std::env::temp_dir().join(format!("roundo-networking-{nanos}"))
}
