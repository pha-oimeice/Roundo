//! Typed transport and application synchronization errors.

#[derive(Debug, thiserror::Error)]
pub enum TransportationLayerError {
    #[error(transparent)]
    Transport(#[from] roundo_quinnet::Error),
    #[error("message codec failed: {0}")]
    Codec(#[from] postcard::Error),
    #[error("received message contains trailing bytes")]
    TrailingBytes,
}

#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    #[error("deadline must be a positive number of FixedUpdate ticks")]
    InvalidDeadline,
    #[error("no explicit destination was supplied")]
    NoTargets,
    #[error("destination is not attached to this Network")]
    UnknownPeer,
    #[error("destination occurs more than once")]
    DuplicateTarget,
    #[error("shared identity range exhausted")]
    IdentityExhausted,
    #[error("fixed tick or submission sequence exhausted")]
    TickOverflow,
    #[error("encoded size/statistics overflow")]
    SizeOverflow,
    #[error("one update exceeds a destination's existing transport message limit")]
    MessageTooLarge,
    #[error("connection send worker is no longer running")]
    WorkerStopped,
    #[error("received update does not match its component/resource kind")]
    TypeMismatch,
    #[error("serialized update contains trailing bytes")]
    TrailingBytes,
    #[error(transparent)]
    Codec(#[from] postcard::Error),
    #[error(transparent)]
    Transport(#[from] TransportationLayerError),
}
