//! Authentication and stream setup ordering for typed connections.

use crate::ProtocolError;
use crate::connection::{ClientConnection, ServerConnection};
use crate::protocol::{
    ClientMessage, GAME_PROTOCOL_VERSION, ProtocolErrorCode, RESOURCE_PROTOCOL_VERSION,
    ResourceCatalogFingerprint, ServerMessage, SessionInfo, StreamId,
};
use tokio::io::{AsyncRead, AsyncWrite};

/// Semantic phase reported by the protocol's linear typestate wrappers.
///
/// This is not a live transport-health indicator. Most transitions consume the
/// old wrapper, and established game/resource wrappers report a fixed phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionState {
    /// A typed transport exists but session negotiation has not started.
    TransportConnected,
    /// Public-session negotiation is being performed.
    Establishing,
    /// The server accepted the public session but no application stream is ready.
    Established,
    /// Stream 0 is typed for game messages.
    InGame,
    /// Stream 1 is typed for resource messages.
    InResource,
    /// Negotiation failed or the wrapper was explicitly closed.
    Closed,
}

/// Client-side session before the server establishes a public session.
pub struct ClientSession<Stream> {
    connection: ClientConnection<Stream>,
    state: SessionState,
}

/// Client typestate after receiving server-confirmed session identity.
pub struct ClientEstablishedSession<Stream> {
    connection: ClientConnection<Stream>,
    info: SessionInfo,
}

/// Client typestate whose stream 0 is ready for game messages.
pub struct ClientGameSession<Stream> {
    connection: ClientConnection<Stream>,
}

/// Client typestate whose stream 1 is ready for resource messages.
pub struct ClientResourceSession<Stream> {
    connection: ClientConnection<Stream>,
}

impl<Stream> ClientSession<Stream> {
    /// Wraps a connected stream 0 without performing session I/O.
    pub fn new(stream: Stream) -> Self {
        Self {
            connection: ClientConnection::new(stream),
            state: SessionState::TransportConnected,
        }
    }

    /// Returns this wrapper's current negotiation phase.
    pub fn state(&self) -> SessionState {
        self.state
    }
}

impl<Stream> ClientSession<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    /// Requests the server's public session over stream 0.
    ///
    /// This consumes the pre-session wrapper. The resource fingerprint must
    /// match the server catalog; success returns server-issued identity. Any
    /// failure drops the connection with no retry through this wrapper.
    ///
    /// # Errors
    ///
    /// Returns framing/transport errors, a typed server rejection, or an
    /// unexpected-message error.
    pub async fn join_public_session(
        mut self,
        resource_fingerprint: ResourceCatalogFingerprint,
    ) -> Result<ClientEstablishedSession<Stream>, ProtocolError> {
        self.state = SessionState::Establishing;
        let request = ClientMessage::JoinPublicSession {
            resource_fingerprint,
        };
        let send_result = self.connection.transmit(&request).await;
        send_result?;
        match self.connection.receive().await? {
            ServerMessage::SessionEstablished { info } => Ok(ClientEstablishedSession {
                connection: self.connection,
                info,
            }),
            ServerMessage::Error { code } => Err(ProtocolError::Rejected(code)),
            message => Err(ProtocolError::unexpected(
                "SessionEstablished or Error",
                server_name(&message),
            )),
        }
    }
}

impl<Stream> ClientEstablishedSession<Stream> {
    /// Returns [`SessionState::Established`].
    pub fn state(&self) -> SessionState {
        SessionState::Established
    }

    /// Returns the copyable identity confirmed by the server.
    pub fn session_info(&self) -> SessionInfo {
        self.info
    }
}

impl<Stream> ClientEstablishedSession<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    /// Advertises the stream-0 game protocol version and enters game typestate.
    ///
    /// Success means the readiness frame was written; the client does not wait
    /// for a separate server acknowledgment.
    pub async fn enter_game(mut self) -> Result<ClientGameSession<Stream>, ProtocolError> {
        let ready = ClientMessage::Ready {
            stream: StreamId::Stream0,
            protocol_version: GAME_PROTOCOL_VERSION,
        };
        let send_result = self.connection.transmit(&ready).await;
        send_result?;
        Ok(ClientGameSession {
            connection: self.connection,
        })
    }
}

impl<Stream> ClientGameSession<Stream> {
    /// Returns [`SessionState::InGame`].
    pub const fn state(&self) -> SessionState {
        SessionState::InGame
    }

    /// Exposes the established game stream. The caller is already past all
    /// linear phases and may run independent read/write tasks.
    pub fn into_connection(self) -> ClientConnection<Stream> {
        self.connection
    }
}

impl<Stream> ClientResourceSession<Stream> {
    /// Returns [`SessionState::InResource`].
    pub const fn state(&self) -> SessionState {
        SessionState::InResource
    }

    /// Consumes the typestate wrapper and returns its typed framed connection.
    pub fn into_connection(self) -> ClientConnection<Stream> {
        self.connection
    }
}

impl<Stream> ClientResourceSession<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    /// Initializes stream 1 after stream 0 established the public session.
    ///
    /// Success means the resource-version readiness frame was written; no
    /// separate server acknowledgment is awaited.
    pub async fn initialize(stream: Stream) -> Result<Self, ProtocolError> {
        let mut connection = ClientConnection::new(stream);
        let ready = ClientMessage::Ready {
            stream: StreamId::Stream1,
            protocol_version: RESOURCE_PROTOCOL_VERSION,
        };
        let send_result = connection.transmit(&ready).await;
        send_result?;
        Ok(Self { connection })
    }
}

/// Server-side session waiting for a public-session request.
pub struct ServerSession<Stream> {
    connection: ServerConnection<Stream>,
    state: SessionState,
}

/// Server typestate holding an unconfirmed resource fingerprint.
pub struct ServerSessionEstablishmentPending<Stream> {
    connection: ServerConnection<Stream>,
    resource_fingerprint: ResourceCatalogFingerprint,
}

/// Server typestate after sending confirmed public-session identity.
pub struct ServerEstablishedSession<Stream> {
    connection: ServerConnection<Stream>,
}

/// Server typestate whose stream 0 accepted the game protocol version.
pub struct ServerGameSession<Stream> {
    connection: ServerConnection<Stream>,
}

/// Server typestate whose stream 1 accepted the resource protocol version.
pub struct ServerResourceSession<Stream> {
    connection: ServerConnection<Stream>,
}

impl<Stream> ServerSession<Stream> {
    /// Wraps an accepted stream 0 without reading negotiation input.
    pub fn new(stream: Stream) -> Self {
        Self {
            connection: ServerConnection::new(stream),
            state: SessionState::TransportConnected,
        }
    }

    /// Returns this wrapper's current negotiation phase.
    pub fn state(&self) -> SessionState {
        self.state
    }
}

impl<Stream> ServerSession<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    /// Consumes the initial client frame and requires a public-session request.
    ///
    /// Success transfers the claimed resource fingerprint into a pending
    /// typestate; policy validation remains the caller's responsibility.
    pub async fn receive_public_session_request(
        mut self,
    ) -> Result<ServerSessionEstablishmentPending<Stream>, ProtocolError> {
        match self.connection.receive().await? {
            ClientMessage::JoinPublicSession {
                resource_fingerprint,
            } => Ok(ServerSessionEstablishmentPending {
                connection: self.connection,
                resource_fingerprint,
            }),
            message => {
                self.state = SessionState::Closed;
                Err(ProtocolError::unexpected(
                    "JoinPublicSession",
                    client_name(&message),
                ))
            }
        }
    }
}

impl<Stream> ServerSessionEstablishmentPending<Stream> {
    /// Returns the client-claimed immutable resource-catalog fingerprint.
    pub const fn resource_fingerprint(&self) -> ResourceCatalogFingerprint {
        self.resource_fingerprint
    }
}

impl<Stream> ServerSessionEstablishmentPending<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    /// Sends server-issued identity before allowing gameplay traffic.
    ///
    /// This method does not accept a game protocol version; callers must next
    /// use [`ServerEstablishedSession::enter_game`].
    pub async fn confirm(
        mut self,
        info: SessionInfo,
    ) -> Result<ServerEstablishedSession<Stream>, ProtocolError> {
        let established = ServerMessage::SessionEstablished { info };
        let send_result = self.connection.transmit(&established).await;
        send_result?;
        Ok(ServerEstablishedSession {
            connection: self.connection,
        })
    }

    /// Sends a typed rejection and then shuts down the write side.
    ///
    /// The operation is not transactional: a shutdown error can occur after the
    /// rejection frame was written.
    pub async fn reject(mut self, code: ProtocolErrorCode) -> Result<(), ProtocolError> {
        let send_result = self
            .connection
            .transmit(&ServerMessage::Error { code })
            .await;
        send_result?;
        return self.connection.finish().await;
    }
}

impl<Stream> ServerEstablishedSession<Stream> {
    /// Returns [`SessionState::Established`].
    pub const fn state(&self) -> SessionState {
        SessionState::Established
    }
}

impl<Stream> ServerEstablishedSession<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    /// Accepts the client's stream-0 version advertisement.
    ///
    /// A mismatched game version is answered with a best-effort typed rejection
    /// and returned as [`ProtocolError::UnsupportedProtocolVersion`]. Any other
    /// message variant returns [`ProtocolError::UnexpectedMessage`].
    pub async fn enter_game(mut self) -> Result<ServerGameSession<Stream>, ProtocolError> {
        match self.connection.receive().await? {
            ClientMessage::Ready {
                stream: StreamId::Stream0,
                protocol_version,
            } if protocol_version == GAME_PROTOCOL_VERSION => Ok(ServerGameSession {
                connection: self.connection,
            }),
            ClientMessage::Ready {
                stream: StreamId::Stream0,
                protocol_version,
            } => {
                let response = ServerMessage::Error {
                    code: ProtocolErrorCode::UnsupportedProtocolVersion,
                };
                let rejection = self.connection.transmit(&response).await;
                if let Err(error) = rejection {
                    log::debug!(
                        "failed to deliver unsupported game protocol response: expected={GAME_PROTOCOL_VERSION}, received={protocol_version}, error={error}"
                    );
                }
                Err(ProtocolError::UnsupportedProtocolVersion {
                    expected: GAME_PROTOCOL_VERSION,
                    received: protocol_version,
                })
            }
            message => Err(ProtocolError::unexpected("Ready", client_name(&message))),
        }
    }
}

impl<Stream> ServerResourceSession<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    /// Accepts stream 1 after stream 0 established the public session.
    ///
    /// The first frame must advertise the matching resource protocol version.
    /// Version mismatch receives a best-effort typed rejection before returning
    /// [`ProtocolError::UnsupportedProtocolVersion`].
    pub async fn initialize(stream: Stream) -> Result<Self, ProtocolError> {
        let mut connection = ServerConnection::new(stream);
        match connection.receive().await? {
            ClientMessage::Ready {
                stream: StreamId::Stream1,
                protocol_version,
            } if protocol_version == RESOURCE_PROTOCOL_VERSION => {
                Ok(ServerResourceSession { connection })
            }
            ClientMessage::Ready {
                stream: StreamId::Stream1,
                protocol_version,
            } => {
                let response = ServerMessage::Error {
                    code: ProtocolErrorCode::UnsupportedProtocolVersion,
                };
                let rejection = connection.transmit(&response).await;
                if let Err(error) = rejection {
                    log::debug!(
                        "failed to deliver unsupported resource protocol response: expected={RESOURCE_PROTOCOL_VERSION}, received={protocol_version}, error={error}"
                    );
                }
                Err(ProtocolError::UnsupportedProtocolVersion {
                    expected: RESOURCE_PROTOCOL_VERSION,
                    received: protocol_version,
                })
            }
            message => Err(ProtocolError::unexpected("Ready", client_name(&message))),
        }
    }
}

impl<Stream> ServerGameSession<Stream> {
    /// Returns [`SessionState::InGame`].
    pub const fn state(&self) -> SessionState {
        SessionState::InGame
    }

    /// Consumes the typestate wrapper and returns its typed framed connection.
    pub fn into_connection(self) -> ServerConnection<Stream> {
        self.connection
    }
}

impl<Stream> ServerResourceSession<Stream> {
    /// Returns [`SessionState::InResource`].
    pub const fn state(&self) -> SessionState {
        SessionState::InResource
    }

    /// Consumes the typestate wrapper and returns its typed framed connection.
    pub fn into_connection(self) -> ServerConnection<Stream> {
        self.connection
    }
}

fn client_name(message: &ClientMessage) -> &'static str {
    match message {
        ClientMessage::JoinPublicSession { .. } => "JoinPublicSession",
        ClientMessage::Ready { .. } => "Ready",
        ClientMessage::Game(_) => "Game",
        ClientMessage::Resource(_) => "Resource",
    }
}

fn server_name(message: &ServerMessage) -> &'static str {
    match message {
        ServerMessage::SessionEstablished { .. } => "SessionEstablished",
        ServerMessage::Error { .. } => "Error",
        ServerMessage::Game(_) => "Game",
        ServerMessage::Resource(_) => "Resource",
    }
}
