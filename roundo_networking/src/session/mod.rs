//! Authentication and stream setup ordering for typed connections.

use crate::ProtocolError;
use crate::connection::{ClientConnection, ServerConnection};
use crate::protocol::{
    ClientMessage, GAME_PROTOCOL_VERSION, ProtocolErrorCode, RESOURCE_PROTOCOL_VERSION,
    ServerMessage, SessionInfo, StreamId,
};
use tokio::io::{AsyncRead, AsyncWrite};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionState {
    TransportConnected,
    Establishing,
    Established,
    InGame,
    InResource,
    Closed,
}

/// Client-side session before the server establishes a public session.
pub struct ClientSession<Stream> {
    connection: ClientConnection<Stream>,
    state: SessionState,
}

pub struct ClientEstablishedSession<Stream> {
    connection: ClientConnection<Stream>,
    info: SessionInfo,
}

pub struct ClientGameSession<Stream> {
    connection: ClientConnection<Stream>,
}

pub struct ClientResourceSession<Stream> {
    connection: ClientConnection<Stream>,
}

impl<Stream> ClientSession<Stream> {
    pub fn new(stream: Stream) -> Self {
        Self {
            connection: ClientConnection::new(stream),
            state: SessionState::TransportConnected,
        }
    }

    pub fn state(&self) -> SessionState {
        self.state
    }
}

impl<Stream> ClientSession<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    /// Request the server's public session over the trusted QUIC connection.
    pub async fn join_public_session(
        mut self,
    ) -> Result<ClientEstablishedSession<Stream>, ProtocolError> {
        self.state = SessionState::Establishing;
        self.connection
            .send(&ClientMessage::JoinPublicSession)
            .await?;
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
    pub fn state(&self) -> SessionState {
        SessionState::Established
    }

    pub fn session_info(&self) -> SessionInfo {
        self.info
    }
}

impl<Stream> ClientEstablishedSession<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    pub async fn enter_game(mut self) -> Result<ClientGameSession<Stream>, ProtocolError> {
        self.connection
            .send(&ClientMessage::Ready {
                stream: StreamId::Stream0,
                protocol_version: GAME_PROTOCOL_VERSION,
            })
            .await?;
        Ok(ClientGameSession {
            connection: self.connection,
        })
    }
}

impl<Stream> ClientGameSession<Stream> {
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
    pub const fn state(&self) -> SessionState {
        SessionState::InResource
    }

    pub fn into_connection(self) -> ClientConnection<Stream> {
        self.connection
    }
}

impl<Stream> ClientResourceSession<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    /// Initialize `stream1` after `stream0` established the public session.
    pub async fn open(stream: Stream) -> Result<Self, ProtocolError> {
        let mut connection = ClientConnection::new(stream);
        connection
            .send(&ClientMessage::Ready {
                stream: StreamId::Stream1,
                protocol_version: RESOURCE_PROTOCOL_VERSION,
            })
            .await?;
        Ok(Self { connection })
    }
}

/// Server-side session waiting for a public-session request.
pub struct ServerSession<Stream> {
    connection: ServerConnection<Stream>,
    state: SessionState,
}

pub struct ServerSessionEstablishmentPending<Stream> {
    connection: ServerConnection<Stream>,
}

pub struct ServerEstablishedSession<Stream> {
    connection: ServerConnection<Stream>,
}

pub struct ServerGameSession<Stream> {
    connection: ServerConnection<Stream>,
}

pub struct ServerResourceSession<Stream> {
    connection: ServerConnection<Stream>,
}

impl<Stream> ServerSession<Stream> {
    pub fn new(stream: Stream) -> Self {
        Self {
            connection: ServerConnection::new(stream),
            state: SessionState::TransportConnected,
        }
    }

    pub fn state(&self) -> SessionState {
        self.state
    }
}

impl<Stream> ServerSession<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    pub async fn receive_public_session_request(
        mut self,
    ) -> Result<ServerSessionEstablishmentPending<Stream>, ProtocolError> {
        match self.connection.receive().await? {
            ClientMessage::JoinPublicSession => Ok(ServerSessionEstablishmentPending {
                connection: self.connection,
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

impl<Stream> ServerSessionEstablishmentPending<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    /// Bind the public session before allowing gameplay traffic.
    pub async fn confirm(
        mut self,
        info: SessionInfo,
    ) -> Result<ServerEstablishedSession<Stream>, ProtocolError> {
        self.connection
            .send(&ServerMessage::SessionEstablished { info })
            .await?;
        Ok(ServerEstablishedSession {
            connection: self.connection,
        })
    }

    pub async fn reject(mut self, code: ProtocolErrorCode) -> Result<(), ProtocolError> {
        self.connection.send(&ServerMessage::Error { code }).await?;
        self.connection.close().await
    }
}

impl<Stream> ServerEstablishedSession<Stream> {
    pub const fn state(&self) -> SessionState {
        SessionState::Established
    }
}

impl<Stream> ServerEstablishedSession<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    /// Accept the client's version advertisement, then enable game messages.
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
                let _ = self
                    .connection
                    .send(&ServerMessage::Error {
                        code: ProtocolErrorCode::UnsupportedProtocolVersion,
                    })
                    .await;
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
    /// Accept `stream1` after `stream0` established the public session.
    pub async fn accept(stream: Stream) -> Result<Self, ProtocolError> {
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
                let _ = connection
                    .send(&ServerMessage::Error {
                        code: ProtocolErrorCode::UnsupportedProtocolVersion,
                    })
                    .await;
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
    pub const fn state(&self) -> SessionState {
        SessionState::InGame
    }

    pub fn into_connection(self) -> ServerConnection<Stream> {
        self.connection
    }
}

impl<Stream> ServerResourceSession<Stream> {
    pub const fn state(&self) -> SessionState {
        SessionState::InResource
    }

    pub fn into_connection(self) -> ServerConnection<Stream> {
        self.connection
    }
}

fn client_name(message: &ClientMessage) -> &'static str {
    match message {
        ClientMessage::JoinPublicSession => "JoinPublicSession",
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
