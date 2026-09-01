use super::*;

pub(super) async fn process_server_connection(
    incoming: quinn::Incoming,
    hooks: Arc<dyn ServerHooks>,
    registry: ConnectionRegistry,
) {
    let peer_address = incoming.remote_address();
    let connection = match incoming.await {
        Ok(connection) => connection,
        Err(error) => {
            log::debug!("QUIC handshake failed: peer={peer_address}, error={error}");
            return;
        }
    };
    let streams = match quic::establish_streams(&connection).await {
        Ok(streams) => streams,
        Err(error) => {
            log::warn!("failed to establish QUIC streams: peer={peer_address}, error={error}");
            connection.close(quinn::VarInt::from_u32(1), b"invalid stream setup");
            return;
        }
    };
    let pending_session = match ServerSession::new(streams.stream0)
        .receive_public_session_request()
        .await
    {
        Ok(result) => result,
        Err(error) => {
            log::warn!(
                "QUIC peer failed before session establishment: peer={peer_address}, error={error}"
            );
            connection.close(
                quinn::VarInt::from_u32(2),
                b"public session request required",
            );
            return;
        }
    };
    let public_session = match hooks.public_session().await {
        Ok(session) => session,
        Err(error) => {
            log::error!("failed to resolve public session: peer={peer_address}, error={error}");
            let _ = pending_session
                .reject(crate::protocol::ProtocolErrorCode::SessionRejected)
                .await;
            connection.close(quinn::VarInt::from_u32(2), b"public session unavailable");
            return;
        }
    };
    let user_session = public_session.user_session;
    let connection_id = registry.next_connection_id();
    let is_open = match hooks.session_is_open(user_session.session_id).await {
        Ok(is_open) => is_open,
        Err(error) => {
            log::error!(
                "failed to validate QUIC session: peer={peer_address}, user_id={}, session_id={}, error={error}",
                user_session.user_id.0,
                user_session.session_id.0
            );
            let _ = pending_session
                .reject(crate::protocol::ProtocolErrorCode::SessionRejected)
                .await;
            connection.close(quinn::VarInt::from_u32(2), b"session validation failed");
            return;
        }
    };
    if !is_open {
        let _ = pending_session
            .reject(crate::protocol::ProtocolErrorCode::SessionRejected)
            .await;
        connection.close(quinn::VarInt::from_u32(2), b"session closed");
        return;
    }
    let established = match pending_session
        .confirm(crate::protocol::SessionInfo { user_session })
        .await
    {
        Ok(session) => session,
        Err(error) => {
            log::warn!(
                "failed to confirm QUIC public session: peer={peer_address}, connection_id={}, error={error}",
                connection_id.0
            );
            connection.close(quinn::VarInt::from_u32(2), b"session establishment failed");
            return;
        }
    };
    let stream0 = match established.enter_game().await {
        Ok(session) => session,
        Err(error) => {
            log::warn!("stream0 protocol setup failed: peer={peer_address}, error={error}");
            connection.close(quinn::VarInt::from_u32(3), b"invalid stream0 protocol");
            return;
        }
    };
    let stream1 = match ServerResourceSession::accept(streams.stream1).await {
        Ok(session) => session,
        Err(error) => {
            log::warn!("stream1 protocol setup failed: peer={peer_address}, error={error}");
            connection.close(quinn::VarInt::from_u32(3), b"invalid stream1 protocol");
            return;
        }
    };

    let mut stream0 = ConnectionIo::spawn(stream0.into_connection());
    let mut stream1 = ConnectionIo::spawn(stream1.into_connection());
    registry.register(
        connection_id,
        user_session,
        stream0.sender(),
        stream1.sender(),
    );
    log::info!(
        "QUIC connection established: peer={peer_address}, connection_id={}, user_id={}, session_id={}",
        connection_id.0,
        user_session.user_id.0,
        user_session.session_id.0
    );
    hooks.on_session_connected(connection_id, user_session);

    loop {
        tokio::select! {
            event = stream0.receive() => match event {
                Some(ConnectionIoEvent::Message(ClientMessage::Game(message))) => {
                    hooks.on_client_game_message(connection_id, user_session, message);
                }
                Some(ConnectionIoEvent::Message(message)) => {
                    log::warn!(
                        "unexpected client stream0 message: peer={peer_address}, connection_id={}, message={}",
                        connection_id.0,
                        client_message_kind(&message)
                    );
                    break;
                }
                Some(ConnectionIoEvent::Stopped { side, result: Err(error) }) if error.is_peer_disconnect() => {
                    log::info!(
                        "QUIC peer disconnected: peer={peer_address}, connection_id={}, stream=stream0, side={}",
                        connection_id.0,
                        side.name()
                    );
                    break;
                }
                Some(ConnectionIoEvent::Stopped { side, result: Err(error) }) => {
                    log::warn!(
                        "stream0 I/O failed: peer={peer_address}, connection_id={}, side={}, error={error}",
                        connection_id.0,
                        side.name()
                    );
                    break;
                }
                Some(ConnectionIoEvent::Stopped { .. }) | None => break,
            },
            event = stream1.receive() => match event {
                Some(ConnectionIoEvent::Message(ClientMessage::Resource(message))) => {
                    hooks.on_client_resource_message(connection_id, user_session, message);
                }
                Some(ConnectionIoEvent::Message(message)) => {
                    log::warn!(
                        "unexpected client stream1 message: peer={peer_address}, connection_id={}, message={}",
                        connection_id.0,
                        client_message_kind(&message)
                    );
                    break;
                }
                Some(ConnectionIoEvent::Stopped { side, result: Err(error) }) if error.is_peer_disconnect() => {
                    log::info!(
                        "QUIC peer disconnected: peer={peer_address}, connection_id={}, stream=stream1, side={}",
                        connection_id.0,
                        side.name()
                    );
                    break;
                }
                Some(ConnectionIoEvent::Stopped { side, result: Err(error) }) => {
                    log::warn!(
                        "stream1 I/O failed: peer={peer_address}, connection_id={}, side={}, error={error}",
                        connection_id.0,
                        side.name()
                    );
                    break;
                }
                Some(ConnectionIoEvent::Stopped { .. }) | None => break,
            },
        }
    }

    stream0.shutdown();
    stream1.shutdown();
    connection.close(quinn::VarInt::from_u32(0), b"session closed");
    if registry.unregister(connection_id, user_session) {
        hooks.on_session_disconnected(connection_id, user_session);
    }
    log::info!(
        "QUIC connection closed: peer={peer_address}, connection_id={}, user_id={}, session_id={}",
        connection_id.0,
        user_session.user_id.0,
        user_session.session_id.0
    );
}

fn client_message_kind(message: &ClientMessage) -> &'static str {
    match message {
        ClientMessage::JoinPublicSession => "JoinPublicSession",
        ClientMessage::Ready { .. } => "Ready",
        ClientMessage::Game(message) => message.kind(),
        ClientMessage::Resource(message) => message.kind(),
    }
}
