use std::collections::TryReserveError;

/// Transport and framing errors retain their original causes.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Connect(#[from] quinn::ConnectError),
    #[error(transparent)]
    Connection(#[from] quinn::ConnectionError),
    #[error(transparent)]
    Read(#[from] quinn::ReadError),
    #[error(transparent)]
    Write(#[from] quinn::WriteError),
    #[error("message limit must be between 1 and u32::MAX bytes")]
    InvalidMessageLimit,
    #[error("message has {size} bytes, exceeding the configured limit {limit}")]
    MessageTooLarge { size: usize, limit: usize },
    #[error("peer finished its message stream")]
    StreamFinished,
    #[error("peer finished its stream inside a message")]
    IncompleteMessage,
    #[error("cannot allocate an incoming message: {0}")]
    Allocation(#[from] TryReserveError),
}
