//! Authentication and game-session ordering for typed connections.

use crate::ProtocolError;
use crate::connection::{ClientConnection, ServerConnection};
use crate::protocol::{
    AuthenticationInfo, ClientMessage, ConnectionToken, GAME_PROTOCOL_VERSION, ProtocolErrorCode,
    ServerMessage,
};
use tokio::io::{AsyncRead, AsyncWrite};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionState {
    Unauthenticated,
    Authenticating,
    Authenticated,
    InGame,
    Closed,
}

/// Client-side session before server-confirmed authentication.
pub struct ClientSession<Stream> {
    connection: ClientConnection<Stream>,
    state: SessionState,
}

pub struct ClientAuthenticatedSession<Stream> {
    connection: ClientConnection<Stream>,
    info: AuthenticationInfo,
}

pub struct ClientGameSession<Stream> {
    connection: ClientConnection<Stream>,
}

impl<Stream> ClientSession<Stream> {
    pub fn new(stream: Stream) -> Self {
        Self {
            connection: ClientConnection::new(stream),
            state: SessionState::Unauthenticated,
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
    /// Send credentials and wait for the server's authoritative confirmation.
    pub async fn authenticate(
        mut self,
        connection_token: ConnectionToken,
    ) -> Result<ClientAuthenticatedSession<Stream>, ProtocolError> {
        self.state = SessionState::Authenticating;
        self.connection
            .send(&ClientMessage::Authenticate { connection_token })
            .await?;
        match self.connection.receive().await? {
            ServerMessage::Authenticated { info } => Ok(ClientAuthenticatedSession {
                connection: self.connection,
                info,
            }),
            ServerMessage::Error { code } => Err(ProtocolError::Rejected(code)),
            message => Err(ProtocolError::unexpected(
                "Authenticated or Error",
                server_name(&message),
            )),
        }
    }
}

impl<Stream> ClientAuthenticatedSession<Stream> {
    pub fn state(&self) -> SessionState {
        SessionState::Authenticated
    }

    pub fn authentication_info(&self) -> AuthenticationInfo {
        self.info
    }
}

impl<Stream> ClientAuthenticatedSession<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    pub async fn enter_game(mut self) -> Result<ClientGameSession<Stream>, ProtocolError> {
        self.connection
            .send(&ClientMessage::Ready {
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

/// Server-side session waiting for the first client authentication message.
pub struct ServerSession<Stream> {
    connection: ServerConnection<Stream>,
    state: SessionState,
}

pub struct ServerAuthenticationPending<Stream> {
    connection: ServerConnection<Stream>,
}

pub struct ServerAuthenticatedSession<Stream> {
    connection: ServerConnection<Stream>,
}

pub struct ServerGameSession<Stream> {
    connection: ServerConnection<Stream>,
}

impl<Stream> ServerSession<Stream> {
    pub fn new(stream: Stream) -> Self {
        Self {
            connection: ServerConnection::new(stream),
            state: SessionState::Unauthenticated,
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
    pub async fn receive_authentication(
        mut self,
    ) -> Result<(ServerAuthenticationPending<Stream>, ConnectionToken), ProtocolError> {
        match self.connection.receive().await? {
            ClientMessage::Authenticate { connection_token } => Ok((
                ServerAuthenticationPending {
                    connection: self.connection,
                },
                connection_token,
            )),
            message => {
                self.state = SessionState::Closed;
                Err(ProtocolError::unexpected(
                    "Authenticate",
                    client_name(&message),
                ))
            }
        }
    }
}

impl<Stream> ServerAuthenticationPending<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    /// Send the authoritative authentication confirmation before allowing
    /// gameplay traffic.
    pub async fn confirm(
        mut self,
        info: AuthenticationInfo,
    ) -> Result<ServerAuthenticatedSession<Stream>, ProtocolError> {
        self.connection
            .send(&ServerMessage::Authenticated { info })
            .await?;
        Ok(ServerAuthenticatedSession {
            connection: self.connection,
        })
    }

    pub async fn reject(mut self, code: ProtocolErrorCode) -> Result<(), ProtocolError> {
        self.connection.send(&ServerMessage::Error { code }).await?;
        self.connection.close().await
    }
}

impl<Stream> ServerAuthenticatedSession<Stream> {
    pub const fn state(&self) -> SessionState {
        SessionState::Authenticated
    }
}

impl<Stream> ServerAuthenticatedSession<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    /// Accept the client's version advertisement, then enable game messages.
    pub async fn enter_game(mut self) -> Result<ServerGameSession<Stream>, ProtocolError> {
        match self.connection.receive().await? {
            ClientMessage::Ready { protocol_version }
                if protocol_version == GAME_PROTOCOL_VERSION =>
            {
                Ok(ServerGameSession {
                    connection: self.connection,
                })
            }
            ClientMessage::Ready { protocol_version } => {
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

impl<Stream> ServerGameSession<Stream> {
    pub const fn state(&self) -> SessionState {
        SessionState::InGame
    }

    pub fn into_connection(self) -> ServerConnection<Stream> {
        self.connection
    }
}

fn client_name(message: &ClientMessage) -> &'static str {
    match message {
        ClientMessage::Authenticate { .. } => "Authenticate",
        ClientMessage::Ready { .. } => "Ready",
        ClientMessage::Game(_) => "Game",
    }
}

fn server_name(message: &ServerMessage) -> &'static str {
    match message {
        ServerMessage::Authenticated { .. } => "Authenticated",
        ServerMessage::Error { .. } => "Error",
        ServerMessage::Game(_) => "Game",
    }
}
