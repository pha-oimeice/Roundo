use std::{io, sync::Arc};

use super::channels::ChannelId;

/// Operation in which a connection's terminal transport failure was observed.
#[derive(Debug, Clone, Copy)]
pub enum TransportOperation {
    /// The QUIC handshake could not establish a connection.
    Handshake,
    /// The underlying QUIC connection ended.
    ConnectionClosed,
    /// A reliable channel could not open its sending stream.
    OpenStream,
    /// A frame could not be sent.
    SendFrame,
    /// A reliable stream could not be flushed.
    FlushStream,
    /// A reliable stream could not be finished.
    FinishStream,
    /// The connection's close notification was lost.
    ReceiveCloseSignal,
}

/// Original transport error; QUIC errors retain their native classification.
#[derive(thiserror::Error, Debug, Clone)]
pub enum TransportError {
    /// The underlying connection ended, including its application close code.
    #[error(transparent)]
    Connection(#[from] quinn::ConnectionError),
    /// Stream or framing failure, retaining its original source chain.
    #[error(transparent)]
    Io(Arc<io::Error>),
    /// A close notification was overwritten before it could be consumed.
    #[error(transparent)]
    CloseSignal(tokio::sync::broadcast::error::RecvError),
}

/// Terminal failure with its original cause and transport-local context.
#[derive(thiserror::Error, Debug, Clone)]
#[error("{operation:?} (channel={channel_id:?}): {source}")]
pub struct TransportFailure {
    /// Operation which observed the failure.
    pub operation: TransportOperation,
    /// Application channel, absent for connection-wide or protocol operations.
    pub channel_id: Option<ChannelId>,
    /// Original error, not an interpretation of an application error code.
    #[source]
    pub source: TransportError,
}
impl TransportFailure {
    pub(crate) fn connection(error: quinn::ConnectionError) -> Self {
        Self::quic(TransportOperation::ConnectionClosed, None, error)
    }

    pub(crate) fn quic(
        operation: TransportOperation,
        channel_id: Option<ChannelId>,
        error: quinn::ConnectionError,
    ) -> Self {
        Self {
            operation,
            channel_id,
            source: TransportError::Connection(error),
        }
    }

    pub(crate) fn io(
        operation: TransportOperation,
        channel_id: Option<ChannelId>,
        error: io::Error,
    ) -> Self {
        // AsyncWrite wraps Quinn's typed errors in io::Error. Recover a native
        // connection error instead of flattening it to "connection lost".
        if let Some(quinn::WriteError::ConnectionLost(error)) =
            error.get_ref().and_then(|source| source.downcast_ref::<quinn::WriteError>())
        {
            return Self::quic(operation, channel_id, error.clone());
        }
        if let Some(error) = error
            .get_ref()
            .and_then(|source| source.downcast_ref::<quinn::ConnectionError>())
        {
            return Self::quic(operation, channel_id, error.clone());
        }
        Self {
            operation,
            channel_id,
            source: TransportError::Io(Arc::new(error)),
        }
    }

    pub(crate) fn close_signal(
        channel_id: ChannelId,
        error: tokio::sync::broadcast::error::RecvError,
    ) -> Self {
        Self {
            operation: TransportOperation::ReceiveCloseSignal,
            channel_id: Some(channel_id),
            source: TransportError::CloseSignal(error),
        }
    }

    /// Whether the underlying QUIC connection was closed locally.
    pub fn is_local_close(&self) -> bool {
        matches!(
            self.source,
            TransportError::Connection(quinn::ConnectionError::LocallyClosed)
        )
    }

    /// Whether the peer sent an application close, irrespective of its code.
    /// This is not a statement that the application-level operation succeeded.
    pub fn is_peer_application_close(&self) -> bool {
        matches!(
            self.source,
            TransportError::Connection(quinn::ConnectionError::ApplicationClosed(_))
        )
    }
}

/// Quinnet internal error in async<->sync communications
#[derive(thiserror::Error, Debug)]
pub enum AsyncChannelError {
    /// The data could not be sent on the channel because the channel is currently full and sending would require blocking
    #[error("The data could not be sent on the channel because the channel is currently full and sending would require blocking")]
    FullQueue,
    /// The receiving half of an internal channel was explicitly closed or has been dropped
    #[error(
        "The receiving half of the internal channel was explicitly closed or has been dropped"
    )]
    InternalChannelClosed,
}

/// Error while closing a channel
#[derive(thiserror::Error, Debug)]
pub enum ChannelCloseError {
    /// A channel is closed
    #[error("Channel is closed already")]
    ChannelAlreadyClosed,
    /// A channel id is invalid
    #[error("Channel with id `{0}` is invalid")]
    InvalidChannelId(ChannelId),
}

/// Errro while creating a channel
#[derive(thiserror::Error, Debug)]
pub enum ChannelCreationError {
    /// The maximum number of simultaneously opened channels has been reached
    #[error("The maximum number of simultaneously opened channels has been reached")]
    MaxChannelsCountReached,
    /// Quinnet async channel error
    #[error("Quinnet async channel error")]
    AsyncChannelError(#[from] AsyncChannelError),
}

/// Error while configuring channels
#[derive(thiserror::Error, Debug)]
pub enum ChannelConfigError {
    /// The maximum number of configured channels has been reached
    #[error("The maximum number of configured channels has been reached")]
    MaxChannelsCountReached,
}

/// Error when sending data from the server
#[derive(thiserror::Error, Debug)]
pub enum ConnectionSendError {
    /// A terminal transport failure has already ended this connection.
    #[error("Connection terminated: {0}")]
    ConnectionTerminated(#[source] TransportFailure),
    /// A channel id is invalid
    #[error("Channel with id `{0}` is invalid")]
    InvalidChannelId(ChannelId),
    /// A channel is closed
    #[error("Channel is closed")]
    ChannelClosed,
    /// Quinnet async channel error
    #[error("Quinnet async channel error")]
    ChannelSendError(#[from] AsyncChannelError),
}

/// Connection is already closed
#[derive(thiserror::Error, Debug)]
#[error("Connection is already closed")]
pub struct ConnectionAlreadyClosed;

#[cfg(feature = "recv_channels")]
/// Error while receiving payloads on a recv channel
#[derive(thiserror::Error, Debug, Clone)]
#[error("Error while receiving payload on a recv channel")]
pub enum RecvChannelError {
    /// The receiving channel queue is full, a payload has been dropped
    #[error("Channel queue with id {0} is full, a payload has been dropped")]
    RecvChannelFull(ChannelId),
    /// The maximum number of opened receive channels has been reached, a payload has been dropped
    #[error(
        "The maximum number of opened receive channels has been reached, triggered by channel id {0}. A payload has been dropped"
    )]
    MaxRecvChannelCountReached(ChannelId),
}

#[cfg(feature = "recv_channels")]
/// Event raised when there is an error while receiving data from the server
#[derive(bevy::ecs::message::Message, Debug, Clone)]
pub struct RecvChannelErrorEvent<T> {
    /// Local id of the connection
    pub id: T,
    /// Error raised during the reception
    pub error: RecvChannelError,
}
