use crate::frame::MAX_FRAME_SIZE;
use crate::protocol::ProtocolErrorCode;
use std::fmt::{Display, Formatter};

/// A recoverable transport or protocol failure.
#[derive(Debug)]
pub enum ProtocolError {
    Io(std::io::Error),
    Encode(postcard::Error),
    Decode(postcard::Error),
    FrameTooLarge {
        length: usize,
        maximum: usize,
    },
    UnexpectedMessage {
        expected: &'static str,
        received: &'static str,
    },
    Rejected(ProtocolErrorCode),
    UnsupportedProtocolVersion {
        expected: u16,
        received: u16,
    },
    ConnectionClosed,
}

impl ProtocolError {
    pub fn unexpected(expected: &'static str, received: &'static str) -> Self {
        Self::UnexpectedMessage { expected, received }
    }

    pub fn frame_too_large(length: usize) -> Self {
        Self::FrameTooLarge {
            length,
            maximum: MAX_FRAME_SIZE,
        }
    }
}

impl Display for ProtocolError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "network I/O error: {error}"),
            Self::Encode(error) => write!(formatter, "postcard encode error: {error}"),
            Self::Decode(error) => write!(formatter, "postcard decode error: {error}"),
            Self::FrameTooLarge { length, maximum } => {
                write!(formatter, "frame length {length} exceeds maximum {maximum}")
            }
            Self::UnexpectedMessage { expected, received } => {
                write!(
                    formatter,
                    "unexpected message {received}; expected {expected}"
                )
            }
            Self::Rejected(code) => {
                write!(formatter, "peer rejected the protocol request: {code:?}")
            }
            Self::UnsupportedProtocolVersion { expected, received } => {
                write!(
                    formatter,
                    "unsupported protocol version {received}; expected {expected}"
                )
            }
            Self::ConnectionClosed => write!(formatter, "connection is closed"),
        }
    }
}

impl std::error::Error for ProtocolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Encode(error) | Self::Decode(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ProtocolError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
