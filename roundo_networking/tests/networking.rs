use roundo_networking::ProtocolError;
use roundo_networking::connection::{
    ClientConnection, ConnectionLifecycle, ServerConnection, run_writer,
};
use roundo_networking::frame::{self, MAX_FRAME_SIZE};
use roundo_networking::protocol::{
    AuthenticationInfo, CharacterId, ClientGameMessage, ClientMessage, ConnectionToken,
    ControllerAccessPolicy, ControllerCameraState, ControllerDescriptor, ControllerId,
    ControllerInput, ControllerKind, ControllerScope, JoinableWorldId, LocomotionInput,
    NearbyJoinableWorld, NearbyPlayer, PlayerId, PresenceSnapshot, ProtocolErrorCode,
    ServerGameMessage, ServerMessage, SessionId, UserId, UserSession, ViewInput,
};
use roundo_networking::session::{ClientSession, ServerSession, SessionState};
use tokio::io::{AsyncWriteExt, DuplexStream};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Duration, timeout};

const TEST_TIMEOUT: Duration = Duration::from_secs(1);

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
        protocol_version: 1,
    };
    let second = ClientMessage::Game(ClientGameMessage::RequestController {
        controller_id: ControllerId(7),
    });

    timeout(TEST_TIMEOUT, async {
        client.send(&first).await?;
        client.send(&second).await?;
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
    let message = ClientMessage::Game(ClientGameMessage::ControllerInput {
        controller_id: ControllerId(3),
        input: ControllerInput::View(ViewInput {
            sequence: 9,
            translation: [1.0, -2.0, 3.0],
            yaw_delta: 0.5,
            pitch_delta: -0.25,
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
async fn unauthenticated_game_intent_is_rejected() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let mut client: ClientConnection<_> = ClientConnection::new(client_stream);
    timeout(
        TEST_TIMEOUT,
        client.send(&ClientMessage::Game(ClientGameMessage::ReleaseController {
            controller_id: ControllerId(11),
        })),
    )
    .await
    .expect("malicious client write must finish")
    .expect("test write must succeed");

    let result = timeout(
        TEST_TIMEOUT,
        ServerSession::new(server_stream).receive_authentication(),
    )
    .await
    .expect("server must inspect the first message");
    let error = match result {
        Ok(_) => panic!("game intent before authentication must be rejected"),
        Err(error) => error,
    };
    assert!(matches!(error, ProtocolError::UnexpectedMessage { .. }));
}

#[tokio::test]
async fn authentication_is_server_confirmed_and_client_state_changes_after_reply() {
    let (client_stream, server_stream) = tokio::io::duplex(1024);
    let (client_done, mut client_result) = oneshot::channel();
    let client_task = tokio::spawn(async move {
        let result = ClientSession::new(client_stream)
            .authenticate(ConnectionToken::new("valid-token"))
            .await;
        let _ = client_done
            .send(result.map(|session| (session.state(), session.authentication_info())));
    });

    let (pending, token) = timeout(
        TEST_TIMEOUT,
        ServerSession::new(server_stream).receive_authentication(),
    )
    .await
    .expect("server must receive authentication")
    .expect("authentication message must be valid");
    assert_eq!(token, ConnectionToken::new("valid-token"));
    assert!(matches!(
        client_result.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));

    let info = authentication_info();
    let authenticated = timeout(TEST_TIMEOUT, pending.confirm(info))
        .await
        .expect("server confirmation must not deadlock")
        .expect("server confirmation must succeed");
    assert_eq!(authenticated.state(), SessionState::Authenticated);

    let (state, received_info) = timeout(TEST_TIMEOUT, client_result)
        .await
        .expect("client must receive confirmation")
        .expect("client task must keep its result")
        .expect("client must accept the authenticated reply");
    assert_eq!(state, SessionState::Authenticated);
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
    outbound
        .send(ClientMessage::Ready {
            protocol_version: 1,
        })
        .expect("test sender should be open");
    drop(outbound);
    let abrupt_result = timeout(TEST_TIMEOUT, tokio::spawn(run_writer(writer, receiver)))
        .await
        .expect("abrupt peer loss must wake the writer")
        .expect("abrupt writer task must not panic");
    assert!(matches!(abrupt_result, Err(ProtocolError::Io(_))));
}

fn authentication_info() -> AuthenticationInfo {
    AuthenticationInfo {
        user_session: UserSession {
            user_id: UserId(42),
            session_id: SessionId(7),
        },
    }
}

fn client_messages() -> Vec<ClientMessage> {
    vec![
        ClientMessage::Authenticate {
            connection_token: ConnectionToken::new("connection-token"),
        },
        ClientMessage::Ready {
            protocol_version: 1,
        },
        ClientMessage::Game(ClientGameMessage::RequestController {
            controller_id: ControllerId(1),
        }),
        ClientMessage::Game(ClientGameMessage::UpdatePlayerPosition {
            translation: [8.0, 4.0, -2.0],
        }),
        ClientMessage::Game(ClientGameMessage::ReleaseController {
            controller_id: ControllerId(2),
        }),
        ClientMessage::Game(ClientGameMessage::ControllerInput {
            controller_id: ControllerId(3),
            input: ControllerInput::Locomotion(LocomotionInput {
                sequence: 4,
                world_direction: [1.0, 0.0, -1.0],
                jump: true,
                sprint: false,
            }),
        }),
        ClientMessage::Game(ClientGameMessage::ControllerInput {
            controller_id: ControllerId(5),
            input: ControllerInput::View(ViewInput {
                sequence: 6,
                translation: [0.0, 1.0, 2.0],
                yaw_delta: 0.25,
                pitch_delta: -0.5,
            }),
        }),
    ]
}

fn server_messages() -> Vec<ServerMessage> {
    let descriptor = ControllerDescriptor {
        controller_id: ControllerId(12),
        kind: ControllerKind::Locomotion,
        access_policy: ControllerAccessPolicy::Exclusive,
        scope: ControllerScope::CharacterLocomotion {
            character_id: CharacterId(13),
        },
    };
    vec![
        ServerMessage::Authenticated {
            info: authentication_info(),
        },
        ServerMessage::Error {
            code: ProtocolErrorCode::AuthenticationRejected,
        },
        ServerMessage::Error {
            code: ProtocolErrorCode::UnexpectedMessage,
        },
        ServerMessage::Error {
            code: ProtocolErrorCode::UnsupportedProtocolVersion,
        },
        ServerMessage::Game(ServerGameMessage::ControllerGranted {
            controller: descriptor,
        }),
        ServerMessage::Game(ServerGameMessage::PresenceSnapshot {
            snapshot: PresenceSnapshot {
                own_player_id: PlayerId(16),
                players: vec![NearbyPlayer {
                    player_id: PlayerId(17),
                    translation: [1.0, 2.0, 3.0],
                }],
                joinable_worlds: vec![NearbyJoinableWorld {
                    world_id: JoinableWorldId(1),
                    name: "Arda".to_string(),
                    translation: [0.0, 0.0, 0.0],
                }],
            },
        }),
        ServerMessage::Game(ServerGameMessage::ControllerRevoked {
            controller_id: ControllerId(14),
        }),
        ServerMessage::Game(ServerGameMessage::ViewCameraState {
            controller_id: ControllerId(15),
            state: ControllerCameraState {
                translation: [2.0, 3.0, 4.0],
                yaw: 0.75,
                pitch: -0.5,
            },
        }),
    ]
}
