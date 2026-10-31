//! Length-prefixed postcard frames.

use crate::ProtocolError;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Maximum postcard payload size, excluding the four-byte length prefix.
pub const MAX_FRAME_SIZE: usize = 64 * 1024;

/// Serializes one postcard payload without its length prefix.
///
/// # Errors
///
/// Returns [`ProtocolError::Encode`] when serialization fails or
/// [`ProtocolError::FrameTooLarge`] when the payload exceeds [`MAX_FRAME_SIZE`].
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

/// Deserializes one postcard payload supplied without a length prefix.
///
/// This function does not enforce [`MAX_FRAME_SIZE`]; callers decoding untrusted
/// standalone buffers must apply their own admission limit.
///
/// # Errors
///
/// Returns [`ProtocolError::Decode`] when `payload` is not a valid `Message`.
pub fn decode<Message>(payload: &[u8]) -> Result<Message, ProtocolError>
where
    Message: DeserializeOwned,
{
    postcard::from_bytes(payload).map_err(ProtocolError::Decode)
}

/// Reads a four-byte big-endian length followed by one postcard payload.
///
/// The function waits for the entire declared frame. Any error may occur after
/// bytes have been consumed, so the stream must not be assumed to remain at a
/// frame boundary.
///
/// # Errors
///
/// Returns an I/O error for short/failed reads, [`ProtocolError::FrameTooLarge`]
/// for an oversized declared length, or [`ProtocolError::Decode`] for an invalid
/// payload.
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
    return decode(&payload);
}

/// Writes and flushes a four-byte big-endian length plus postcard payload.
///
/// The operation is not transactional: an I/O error may leave a partial prefix
/// or payload in the stream, after which callers must not assume frame alignment.
///
/// # Errors
///
/// Returns serialization/frame-size errors before writing, or an I/O error from
/// writing or flushing the frame.
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
