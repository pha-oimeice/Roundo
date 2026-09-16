use roundo_networking::ProtocolError;
use roundo_networking::connection::{
    ClientConnection, ConnectionLifecycle, ServerConnection, run_writer,
};
use roundo_networking::frame::{self, MAX_FRAME_SIZE};
use roundo_networking::protocol::{
    ChunkVersion, ClientGameMessage, ClientMessage, ClientResourceMessage, ControllerCommand,
    DestroyBlockControllerAction, JoinableWorldId, LocalCoordinateId, Movement3DAction,
    NearbyJoinableWorld, NearbyPlayer, PlaceBlockControllerAction, PlayerControllerCommand,
    PlayerId, PlayerState, PresenceSnapshot, ProtocolErrorCode, ResourceCatalogFingerprint,
    RotationSync, SceneId, SerializedPayload, ServerGameMessage, ServerMessage,
    ServerResourceMessage, SessionId, SessionInfo, StreamId, UserId, UserSession,
};
use roundo_networking::session::{ClientSession, ServerSession, SessionState};
use roundo_toolbox::UpdateVersion;
use tokio::io::{AsyncWriteExt, DuplexStream};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Duration, timeout};

const TEST_TIMEOUT: Duration = Duration::from_secs(1);

#[test]
fn serialized_payload_keeps_domain_serialization_at_the_host_seam() {
    let source = [3_u32, 5, 8, 13];
    let payload = SerializedPayload::encode(&source).expect("domain payload should encode");
    let decoded: [u32; 4] = payload.decode().expect("domain payload should decode");

    assert_eq!(decoded, source);
}

#[test]
fn every_client_and_server_message_round_trips_through_postcard() {
    for message in client_messages() {
        let payload = frame::encode(&message).expect("client message should encode");
        let decoded: ClientMessage = frame::decode(&payload).expect("client message should decode");
        assert_eq!(decoded, message);
    }

    for message in server_messages() {
        let payload = frame::encode(&message).expect("server message should encode");
        let decoded: ServerMessage = frame::decode(&payload).expect("server message should decode");
        assert_eq!(decoded, message);
    }
}

#[tokio::test]
async fn consecutive_frames_preserve_boundaries_and_order() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let mut client: ClientConnection<_> = ClientConnection::new(client_stream);
    let mut server: ServerConnection<_> = ServerConnection::new(server_stream);
    let first = ClientMessage::Ready {
        stream: StreamId::Stream0,
        protocol_version: 1,
    };
    let second = ClientMessage::Game(ClientGameMessage::UsePlayerController {
        command: PlayerControllerCommand::Movement3D(ControllerCommand {
            sequence: 7,
            action: Movement3DAction {
                direction: [1.0, 0.0, 0.0],
            },
        }),
    });

    timeout(TEST_TIMEOUT, async {
        client.transmit(&first).await?;
        client.transmit(&second).await?;
        let received_first = server.receive().await?;
        let received_second = server.receive().await?;
        assert_eq!(received_first, first);
        assert_eq!(received_second, second);
        Ok::<(), ProtocolError>(())
    })
    .await
    .expect("framed exchange must not deadlock")
    .expect("framed exchange must succeed");
}

#[tokio::test]
async fn arbitrarily_fragmented_frame_decodes_once_complete() {
    let (mut writer, reader) = tokio::io::duplex(1024);
    let message = ClientMessage::Game(ClientGameMessage::UsePlayerController {
        command: PlayerControllerCommand::SyncRotation(RotationSync {
            rotation: [0.0, 0.25, -0.5, 1.0],
        }),
    });
    let payload = frame::encode(&message).expect("message should encode");
    let mut bytes = (payload.len() as u32).to_be_bytes().to_vec();
    bytes.extend(payload);

    let writer_task = tokio::spawn(async move {
        let mut offset = 0;
        for chunk_length in [1_usize, 3, 2, 5, 1, 8, 13] {
            if offset == bytes.len() {
                break;
            }
            let next = (offset + chunk_length).min(bytes.len());
            writer.write_all(&bytes[offset..next]).await?;
            offset = next;
            tokio::task::yield_now().await;
        }
        if offset < bytes.len() {
            writer.write_all(&bytes[offset..]).await?;
        }
        Ok::<(), std::io::Error>(())
    });
    let mut server: ServerConnection<_> = ServerConnection::new(reader);

    let received = timeout(TEST_TIMEOUT, server.receive())
        .await
        .expect("fragmented frame must not deadlock")
        .expect("fragmented frame must decode");
    assert_eq!(received, message);
    timeout(TEST_TIMEOUT, writer_task)
        .await
        .expect("fragment writer must finish")
        .expect("fragment writer task must not panic")
        .expect("fragment writer must succeed");
}

#[tokio::test]
async fn oversized_frame_is_rejected_before_payload_allocation() {
    let (mut attacker, stream) = tokio::io::duplex(64);
    let mut server: ServerConnection<_> = ServerConnection::new(stream);
    attacker
        .write_all(&((MAX_FRAME_SIZE as u32 + 1).to_be_bytes()))
        .await
        .expect("test stream should accept a frame header");

    let error = timeout(TEST_TIMEOUT, server.receive())
        .await
        .expect("oversized frame must fail immediately")
        .expect_err("oversized frame must be rejected");
    assert!(matches!(
        error,
        ProtocolError::FrameTooLarge {
            length,
            maximum: MAX_FRAME_SIZE
        } if length == MAX_FRAME_SIZE + 1
    ));
}

#[tokio::test]
async fn game_intent_before_session_establishment_is_rejected() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let mut client: ClientConnection<_> = ClientConnection::new(client_stream);
    timeout(
        TEST_TIMEOUT,
        client.transmit(&ClientMessage::Game(
            ClientGameMessage::UsePlayerController {
                command: PlayerControllerCommand::Movement3D(ControllerCommand {
                    sequence: 11,
                    action: Movement3DAction {
                        direction: [0.0, 0.0, 1.0],
                    },
                }),
            },
        )),
    )
    .await
    .expect("malicious client write must finish")
    .expect("test write must succeed");

    let result = timeout(
        TEST_TIMEOUT,
        ServerSession::new(server_stream).receive_public_session_request(),
    )
    .await
    .expect("server must inspect the first message");
    let error = match result {
        Ok(_) => panic!("game intent before session establishment must be rejected"),
        Err(error) => error,
    };
    assert!(matches!(error, ProtocolError::UnexpectedMessage { .. }));
}

#[tokio::test]
async fn public_session_is_server_confirmed_and_client_state_changes_after_reply() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_done, mut client_result) = oneshot::channel();
    let client_task = tokio::spawn(async move {
        let result = ClientSession::new(client_stream)
            .join_public_session(ResourceCatalogFingerprint::default())
            .await;
        let client_summary = result.map(|session| (session.state(), session.session_info()));
        let notification = client_done.send(client_summary);
        assert!(
            notification.is_ok(),
            "client result test receiver was dropped"
        );
    });

    let pending = timeout(
        TEST_TIMEOUT,
        ServerSession::new(server_stream).receive_public_session_request(),
    )
    .await
    .expect("server must receive public session request")
    .expect("public session request must be valid");
    assert_eq!(
        pending.resource_fingerprint(),
        ResourceCatalogFingerprint::default()
    );
    assert!(matches!(
        client_result.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));

    let info = session_info();
    let established = timeout(TEST_TIMEOUT, pending.confirm(info))
        .await
        .expect("server confirmation must not deadlock")
        .expect("server confirmation must succeed");
    assert_eq!(established.state(), SessionState::Established);

    let (state, received_info) = timeout(TEST_TIMEOUT, client_result)
        .await
        .expect("client must receive confirmation")
        .expect("client task must keep its result")
        .expect("client must accept the established session reply");
    assert_eq!(state, SessionState::Established);
    assert_eq!(received_info, info);
    timeout(TEST_TIMEOUT, client_task)
        .await
        .expect("client task must exit")
        .expect("client task must not panic");
}

#[tokio::test]
async fn normal_and_abnormal_disconnects_end_connection_tasks() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let client: ClientConnection<_> = ClientConnection::new(client_stream);
    let (reader, writer) = client.into_split();
    let (outbound, receiver) = mpsc::unbounded_channel::<ClientMessage>();
    drop(outbound);
    let writer_task = tokio::spawn(run_writer(writer, receiver));
    let mut server: ServerConnection<_> = ServerConnection::new(server_stream);

    timeout(TEST_TIMEOUT, writer_task)
        .await
        .expect("normal writer shutdown must finish")
        .expect("normal writer task must not panic")
        .expect("normal writer shutdown must succeed");
    let normal_error = timeout(TEST_TIMEOUT, server.receive())
        .await
        .expect("peer close must wake a reader")
        .expect_err("reader must observe normal peer close");
    assert!(matches!(normal_error, ProtocolError::Io(_)));
    assert_eq!(reader.lifecycle(), ConnectionLifecycle::Closed);
    assert_eq!(server.lifecycle(), ConnectionLifecycle::Closed);

    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let client: ClientConnection<_> = ClientConnection::new(client_stream);
    let (_, writer) = client.into_split();
    drop(ServerConnection::<DuplexStream>::new(server_stream));
    let (outbound, receiver) = mpsc::unbounded_channel();
    let notification = outbound.send(ClientMessage::Ready {
        stream: StreamId::Stream0,
        protocol_version: 1,
    });
    notification.expect("test sender should be open");
    drop(outbound);
    let abrupt_result = timeout(TEST_TIMEOUT, tokio::spawn(run_writer(writer, receiver)))
        .await
        .expect("abrupt peer loss must wake the writer")
        .expect("abrupt writer task must not panic");
    assert!(matches!(abrupt_result, Err(ProtocolError::Io(_))));
}

fn session_info() -> SessionInfo {
    SessionInfo {
        user_session: UserSession {
            user_id: UserId(42),
            session_id: SessionId(7),
        },
    }
}

fn client_messages() -> Vec<ClientMessage> {
    vec![
        ClientMessage::JoinPublicSession {
            resource_fingerprint: ResourceCatalogFingerprint::default(),
        },
        ClientMessage::Ready {
            stream: StreamId::Stream0,
            protocol_version: 1,
        },
        ClientMessage::Game(ClientGameMessage::UsePlayerController {
            command: PlayerControllerCommand::Movement3D(ControllerCommand {
                sequence: 4,
                action: Movement3DAction {
                    direction: [0.0, 0.0, 0.0],
                },
            }),
        }),
        ClientMessage::Game(ClientGameMessage::UsePlayerController {
            command: PlayerControllerCommand::SyncRotation(RotationSync {
                rotation: [0.0, 0.125, -0.25, 1.0],
            }),
        }),
        ClientMessage::Game(ClientGameMessage::UsePlayerController {
            command: PlayerControllerCommand::DestroyBlock(ControllerCommand {
                sequence: 2,
                action: DestroyBlockControllerAction,
            }),
        }),
        ClientMessage::Game(ClientGameMessage::UsePlayerController {
            command: PlayerControllerCommand::PlaceBlock(ControllerCommand {
                sequence: 3,
                action: PlaceBlockControllerAction { voxel_id: 1 },
            }),
        }),
        ClientMessage::Resource(ClientResourceMessage::RequestLocalCoordinateChunks {
            chunks: vec![test_chunk_version().id()],
        }),
    ]
}

fn server_messages() -> Vec<ServerMessage> {
    vec![
        ServerMessage::SessionEstablished {
            info: session_info(),
        },
        ServerMessage::Error {
            code: ProtocolErrorCode::SessionRejected,
        },
        ServerMessage::Error {
            code: ProtocolErrorCode::UnexpectedMessage,
        },
        ServerMessage::Error {
            code: ProtocolErrorCode::UnsupportedProtocolVersion,
        },
        ServerMessage::Error {
            code: ProtocolErrorCode::IncompatibleResources,
        },
        ServerMessage::Game(ServerGameMessage::PlayerState {
            state: PlayerState {
                player_id: PlayerId(13),
                scene_id: SceneId::S1,
                translation: [0.0, 2.0, 0.0],
                rotation: [-0.707, 0.0, 0.0, 0.707],
            },
        }),
        ServerMessage::Game(ServerGameMessage::PresenceSnapshot {
            snapshot: PresenceSnapshot {
                own_player_id: PlayerId(16),
                players: vec![NearbyPlayer {
                    player_id: PlayerId(17),
                    translation: [1.0, 2.0, 3.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                }],
                joinable_worlds: vec![NearbyJoinableWorld {
                    world_id: JoinableWorldId(1),
                    name: "Arda".to_string(),
                    translation: [0.0, 0.0, 0.0],
                }],
            },
        }),
        ServerMessage::Resource(ServerResourceMessage::LocalCoordinateChunkVersions {
            chunks: vec![test_chunk_version()],
        }),
        ServerMessage::Resource(ServerResourceMessage::LocalCoordinateChunk {
            chunk: test_chunk_version(),
            edge_length: 16,
            svo: test_chunk_svo(),
        }),
    ]
}

fn test_chunk_version() -> ChunkVersion {
    ChunkVersion {
        local_coordinate_id: LocalCoordinateId(1),
        coordinate: [2, -1, 4],
        version: UpdateVersion::new(9),
    }
}

fn test_chunk_svo() -> SerializedPayload {
    let encoded = SerializedPayload::encode(&[0_u32, 7, 11]);
    encoded.unwrap()
}
