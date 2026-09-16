use roundo_networking::protocol::{
    ClientGameMessage, ClientResourceMessage, ConnectionId, ControllerCommand, LocalCoordinateId,
    Movement3DAction, PlayerControllerCommand, PlayerId, PlayerState, SceneId, ServerGameMessage,
    ServerResourceMessage, SessionId, UserId, UserSession,
};
use roundo_networking::{
    CertificatePolicy, ClientHooks, ClientNetwork, ClientNetworkConfig, HookFuture, PublicSession,
    ResourceCatalogFingerprint, ServerHooks, ServerNetwork, ServerNetworkConfig, StreamId,
    TransportAdmissionPolicy,
};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TEST_TIMEOUT: Duration = Duration::from_secs(3);

#[tokio::test]
async fn client_and_server_exchange_messages_over_prioritized_quic_streams() {
    let (connected_sender, connected_receiver) = channel();
    let (client_message_sender, client_message_receiver) = channel();
    let (client_resource_sender, client_resource_receiver) = channel();
    let certificate_directory = test_certificate_directory();
    let server = ServerNetwork::start(
        server_config(certificate_directory.clone()),
        Arc::new(TestServerHooks {
            resource_fingerprint: ResourceCatalogFingerprint([7; 32]),
            connected_sender,
            client_message_sender,
            client_resource_sender,
        }),
    )
    .expect("network server should start");
    let addresses = server.addresses();
    let (server_message_sender, server_message_receiver) = channel();
    let (server_resource_sender, server_resource_receiver) = channel();
    let (connection_error_sender, connection_error_receiver) = channel();
    let client = ClientNetwork::start(
        ClientNetworkConfig {
            quic_address: addresses.quic_address,
            server_name: "localhost".to_string(),
            certificate_policy: CertificatePolicy::TrustOnFirstUse,
            reconnect_delay: Duration::from_millis(10),
            admission: TransportAdmissionPolicy::default(),
        },
        Arc::new(TestClientHooks {
            resource_fingerprint: ResourceCatalogFingerprint([7; 32]),
            server_message_sender,
            server_resource_sender,
            connection_error_sender,
        }),
    )
    .expect("network client should start");

    let (connection_id, user_session) =
        receive_or_connection_error(connected_receiver, connection_error_receiver).await;
    assert_eq!(user_session, test_session());

    let request = ClientGameMessage::UsePlayerController {
        command: PlayerControllerCommand::Movement3D(ControllerCommand {
            sequence: 7,
            action: Movement3DAction {
                direction: [1.0, 0.0, 0.0],
            },
        }),
    };
    let wrong_stream = client.send(StreamId::Stream1, request.clone());
    assert!(wrong_stream.is_err());
    let send_result = client.send(StreamId::Stream0, request.clone());
    send_result.expect("connected client should send game message");
    let (received_connection_id, received_session, received_message) =
        receive(client_message_receiver).await;
    assert_eq!(received_connection_id, connection_id);
    assert_eq!(received_session, test_session());
    assert_eq!(received_message, request);

    let outbound = ServerGameMessage::PlayerState {
        state: PlayerState {
            player_id: PlayerId(1),
            scene_id: SceneId::S1,
            translation: [0.25, 2.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
        },
    };
    assert!(!server.send_to_connection(connection_id, StreamId::Stream1, outbound.clone()));
    assert!(server.send_to_connection(connection_id, StreamId::Stream0, outbound.clone()));
    assert_eq!(receive(server_message_receiver).await, outbound);

    let resource_request = ClientResourceMessage::RequestLocalCoordinateChunks { chunks: vec![] };
    let send_result = client.send(StreamId::Stream1, resource_request.clone());
    send_result.expect("connected client should send resource message");
    let (received_connection_id, received_session, received_message) =
        receive(client_resource_receiver).await;
    assert_eq!(received_connection_id, connection_id);
    assert_eq!(received_session, test_session());
    assert_eq!(received_message, resource_request);

    let resource_outbound = ServerResourceMessage::LocalCoordinateSpawned {
        local_coordinate_id: LocalCoordinateId(1),
    };
    assert!(server.send_to_connection(connection_id, StreamId::Stream1, resource_outbound.clone()));
    assert_eq!(receive(server_resource_receiver).await, resource_outbound);

    client.shutdown();
    server.shutdown();
    let _ = std::fs::remove_dir_all(certificate_directory);
}

#[tokio::test]
async fn incompatible_resource_catalog_is_rejected_before_session_connection() {
    let (connected_sender, connected_receiver) = channel();
    let (client_message_sender, _) = channel();
    let (client_resource_sender, _) = channel();
    let certificate_directory = test_certificate_directory();
    let server = ServerNetwork::start(
        server_config(certificate_directory.clone()),
        Arc::new(TestServerHooks {
            resource_fingerprint: ResourceCatalogFingerprint([1; 32]),
            connected_sender,
            client_message_sender,
            client_resource_sender,
        }),
    )
    .expect("network server should start");
    let (server_message_sender, _) = channel();
    let (server_resource_sender, _) = channel();
    let (connection_error_sender, connection_error_receiver) = channel();
    let client = ClientNetwork::start(
        ClientNetworkConfig {
            quic_address: server.addresses().quic_address,
            server_name: "localhost".to_string(),
            certificate_policy: CertificatePolicy::TrustOnFirstUse,
            reconnect_delay: Duration::from_secs(60),
            admission: TransportAdmissionPolicy::default(),
        },
        Arc::new(TestClientHooks {
            resource_fingerprint: ResourceCatalogFingerprint([2; 32]),
            server_message_sender,
            server_resource_sender,
            connection_error_sender,
        }),
    )
    .expect("network client should start");

    let error = receive(connection_error_receiver).await;
    assert!(!error.is_empty());
    assert!(connected_receiver.try_recv().is_err());

    client.shutdown();
    server.shutdown();
    let _ = std::fs::remove_dir_all(certificate_directory);
}

struct TestServerHooks {
    resource_fingerprint: ResourceCatalogFingerprint,
    connected_sender: Sender<(ConnectionId, UserSession)>,
    client_message_sender: Sender<(ConnectionId, UserSession, ClientGameMessage)>,
    client_resource_sender: Sender<(ConnectionId, UserSession, ClientResourceMessage)>,
}

impl ServerHooks for TestServerHooks {
    fn initialize(&self) -> HookFuture<()> {
        Box::pin(async { Ok(()) })
    }

    fn resource_catalog_fingerprint(&self) -> ResourceCatalogFingerprint {
        self.resource_fingerprint
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
        let notification = self.connected_sender.send((connection_id, user_session));
        assert!(
            notification.is_ok(),
            "connected-event test receiver was dropped"
        );
    }

    fn on_session_disconnected(&self, _: ConnectionId, _: UserSession) {}

    fn on_client_game_message(
        &self,
        connection_id: ConnectionId,
        user_session: UserSession,
        message: ClientGameMessage,
    ) {
        let sender = &self.client_message_sender;
        let notification = sender.send((connection_id, user_session, message));
        assert!(
            notification.is_ok(),
            "game-message test receiver was dropped"
        );
    }

    fn on_client_resource_message(
        &self,
        connection_id: ConnectionId,
        user_session: UserSession,
        message: ClientResourceMessage,
    ) {
        let sender = &self.client_resource_sender;
        let notification = sender.send((connection_id, user_session, message));
        assert!(
            notification.is_ok(),
            "resource-message test receiver was dropped"
        );
    }
}

struct TestClientHooks {
    resource_fingerprint: ResourceCatalogFingerprint,
    server_message_sender: Sender<ServerGameMessage>,
    server_resource_sender: Sender<ServerResourceMessage>,
    connection_error_sender: Sender<String>,
}

impl ClientHooks for TestClientHooks {
    fn resource_catalog_fingerprint(&self) -> ResourceCatalogFingerprint {
        self.resource_fingerprint
    }

    fn on_server_game_message(&self, message: ServerGameMessage) {
        let notification = self.server_message_sender.send(message);
        assert!(
            notification.is_ok(),
            "server-message test receiver was dropped"
        );
    }

    fn on_server_resource_message(&self, message: ServerResourceMessage) {
        let notification = self.server_resource_sender.send(message);
        assert!(
            notification.is_ok(),
            "server-resource test receiver was dropped"
        );
    }

    fn on_connection_error(&self, error: &roundo_networking::NetworkError) {
        let notification = self.connection_error_sender.send(error.to_string());
        assert!(
            notification.is_ok(),
            "connection-error test receiver was dropped"
        );
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
        quic_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        certificate_directory,
        server_alternative_names: vec!["localhost".to_string()],
        generate_self_signed_certificate: true,
        admission: TransportAdmissionPolicy::default(),
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
