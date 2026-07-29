//! Length-prefixed postcard frames.

use crate::ProtocolError;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Maximum postcard payload size, excluding the four-byte length prefix.
pub const MAX_FRAME_SIZE: usize = 64 * 1024;

pub fn encode<Message>(message: &Message) -> Result<Vec<u8>, ProtocolError>
where
    Message: Serialize,
{
    let payload = postcard::to_allocvec(message).map_err(ProtocolError::Encode)?;
    if payload.len() > MAX_FRAME_SIZE {
        return Err(ProtocolError::frame_too_large(payload.len()));
    }
    Ok(payload)
}

pub fn decode<Message>(payload: &[u8]) -> Result<Message, ProtocolError>
where
    Message: DeserializeOwned,
{
    postcard::from_bytes(payload).map_err(ProtocolError::Decode)
}

pub async fn read<Message, Stream>(stream: &mut Stream) -> Result<Message, ProtocolError>
where
    Message: DeserializeOwned,
    Stream: AsyncRead + Unpin,
{
    let mut length_bytes = [0_u8; 4];
    stream
        .read_exact(&mut length_bytes)
        .await
        .map_err(ProtocolError::Io)?;
    let length = u32::from_be_bytes(length_bytes) as usize;
    if length > MAX_FRAME_SIZE {
        return Err(ProtocolError::frame_too_large(length));
    }

    let mut payload = vec![0_u8; length];
    stream
        .read_exact(&mut payload)
        .await
        .map_err(ProtocolError::Io)?;
    decode(&payload)
}

pub async fn write<Message, Stream>(
    stream: &mut Stream,
    message: &Message,
) -> Result<(), ProtocolError>
where
    Message: Serialize,
    Stream: AsyncWrite + Unpin,
{
    let payload = encode(message)?;
    stream
        .write_all(&(payload.len() as u32).to_be_bytes())
        .await
        .map_err(ProtocolError::Io)?;
    stream
        .write_all(&payload)
        .await
        .map_err(ProtocolError::Io)?;
    stream.flush().await.map_err(ProtocolError::Io)
}
