use crate::Error;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::Mutex;

/// Explicit maximum serialized payload size, excluding the four-byte header.
/// Zero-length messages are valid; there is deliberately no default limit.
#[derive(Clone, Copy, Debug)]
pub struct MessageLimit(usize);

impl MessageLimit {
    pub fn new(bytes: usize) -> Result<Self, Error> {
        if bytes == 0 || u32::try_from(bytes).is_err() {
            return Err(Error::InvalidMessageLimit);
        }
        Ok(Self(bytes))
    }

    pub const fn bytes(self) -> usize {
        self.0
    }
}

/// Clones share a physical connection and its ordered send/receive streams.
/// Dropping the last owner closes the connection; the endpoint stays alive as
/// long as a connection owner exists. Neither a peer address nor this handle
/// grants application-level authorization.
#[derive(Clone)]
pub struct Connection(Arc<Inner>);

struct Inner {
    endpoint: quinn::Endpoint,
    connection: quinn::Connection,
    limit: MessageLimit,
    send: Mutex<Option<quinn::SendStream>>,
    receive: Mutex<Reader>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.connection
            .close(0u32.into(), b"connection owner dropped");
    }
}

// Partial receive progress lives with the connection, not a cancellable future.
#[derive(Default)]
struct Reader {
    stream: Option<quinn::RecvStream>,
    header: [u8; 4],
    header_read: usize,
    payload: Vec<u8>,
    payload_read: usize,
    length: Option<usize>,
}

// Quinn's write_all is not cancellation-safe. Never reuse a partially written
// stream after cancellation or error, since its next frame would be ambiguous.
struct WriteTransaction<'a> {
    connection: &'a quinn::Connection,
    complete: bool,
}

impl Drop for WriteTransaction<'_> {
    fn drop(&mut self) {
        if !self.complete {
            self.connection
                .close(1u32.into(), b"message write interrupted");
        }
    }
}

impl Connection {
    pub(crate) fn new(
        endpoint: quinn::Endpoint,
        connection: quinn::Connection,
        limit: MessageLimit,
    ) -> Self {
        Self(Arc::new(Inner {
            endpoint,
            connection,
            limit,
            send: Mutex::new(None),
            receive: Mutex::new(Reader::default()),
        }))
    }

    /// Current remote address. QUIC migration can change it; it is not durable.
    pub fn peer_addr(&self) -> SocketAddr {
        self.0.connection.remote_address()
    }

    pub fn local_addr(&self) -> Result<SocketAddr, Error> {
        Ok(self.0.endpoint.local_addr()?)
    }

    pub fn message_limit(&self) -> MessageLimit {
        self.0.limit
    }

    /// Backpressured, ordered submission to QUIC, not remote application commit.
    /// Oversized messages are rejected before touching the stream. Cancelling
    /// after writing starts closes the connection to prevent frame corruption.
    pub async fn send(&self, payload: &[u8]) -> Result<(), Error> {
        if payload.len() > self.0.limit.bytes() {
            return Err(Error::MessageTooLarge {
                size: payload.len(),
                limit: self.0.limit.bytes(),
            });
        }
        let mut stream = self.0.send.lock().await;
        if stream.is_none() {
            *stream = Some(self.0.connection.open_uni().await?);
        }
        let mut transaction = WriteTransaction {
            connection: &self.0.connection,
            complete: false,
        };
        let stream = stream.as_mut().expect("opened send stream");
        stream
            .write_all(&(payload.len() as u32).to_be_bytes())
            .await?;
        stream.write_all(payload).await?;
        transaction.complete = true;
        Ok(())
    }

    /// Receives exactly one message. Cancelling this future retains partial
    /// framing progress for the next call. Cloned receivers consume one shared
    /// FIFO, not independent copies. Invalid/unfinished frames close the peer.
    pub async fn receive(&self) -> Result<Vec<u8>, Error> {
        let mut reader = self.0.receive.lock().await;
        let result = reader.read(&self.0.connection, self.0.limit).await;
        if result.is_err() {
            self.0
                .connection
                .close(1u32.into(), b"message receive failed");
        }
        result
    }

    pub fn close(&self, code: u32, reason: &[u8]) {
        self.0.connection.close(code.into(), reason);
    }

    pub async fn closed(&self) -> quinn::ConnectionError {
        self.0.connection.closed().await
    }
}

impl Reader {
    async fn read(
        &mut self,
        connection: &quinn::Connection,
        limit: MessageLimit,
    ) -> Result<Vec<u8>, Error> {
        if self.stream.is_none() {
            self.stream = Some(connection.accept_uni().await?);
        }
        let stream = self.stream.as_mut().expect("accepted receive stream");
        while self.header_read < self.header.len() {
            match stream.read(&mut self.header[self.header_read..]).await? {
                Some(count) => self.header_read += count,
                None if self.header_read == 0 => return Err(Error::StreamFinished),
                None => return Err(Error::IncompleteMessage),
            }
        }
        let length = match self.length {
            Some(length) => length,
            None => {
                let length = u32::from_be_bytes(self.header) as usize;
                if length > limit.bytes() {
                    return Err(Error::MessageTooLarge {
                        size: length,
                        limit: limit.bytes(),
                    });
                }
                self.payload.try_reserve_exact(length)?;
                self.payload.resize(length, 0);
                self.length = Some(length);
                length
            }
        };
        while self.payload_read < length {
            match stream.read(&mut self.payload[self.payload_read..]).await? {
                Some(count) => self.payload_read += count,
                None => return Err(Error::IncompleteMessage),
            }
        }
        self.header_read = 0;
        self.payload_read = 0;
        self.length = None;
        Ok(std::mem::take(&mut self.payload))
    }
}
