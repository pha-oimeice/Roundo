//! Typed, post-TLS connections and their shared I/O lifecycle.

use crate::ProtocolError;
use crate::frame;
use crate::protocol::{ClientMessage, ServerMessage};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::mpsc::UnboundedReceiver;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionLifecycle {
    /// TCP and TLS have completed; no application I/O has happened yet.
    TlsEstablished,
    Open,
    Closing,
    Closed,
}

impl ConnectionLifecycle {
    fn as_u8(self) -> u8 {
        match self {
            Self::TlsEstablished => 0,
            Self::Open => 1,
            Self::Closing => 2,
            Self::Closed => 3,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::TlsEstablished,
            1 => Self::Open,
            2 => Self::Closing,
            _ => Self::Closed,
        }
    }
}

type LifecycleHandle = Arc<AtomicU8>;

/// A post-TLS full-duplex connection.
///
/// `Incoming` and `Outgoing` are intentionally separate. A client connection
/// is therefore unable to read a `ClientMessage`, and a server connection is
/// unable to send one.
pub struct Connection<Stream, Incoming, Outgoing> {
    stream: Stream,
    lifecycle: LifecycleHandle,
    marker: PhantomData<fn(Incoming) -> Outgoing>,
}

pub type ClientConnection<Stream> = Connection<Stream, ServerMessage, ClientMessage>;
pub type ServerConnection<Stream> = Connection<Stream, ClientMessage, ServerMessage>;

impl<Stream, Incoming, Outgoing> Connection<Stream, Incoming, Outgoing> {
    pub fn new(stream: Stream) -> Self {
        Self {
            stream,
            lifecycle: Arc::new(AtomicU8::new(ConnectionLifecycle::TlsEstablished.as_u8())),
            marker: PhantomData,
        }
    }

    pub fn lifecycle(&self) -> ConnectionLifecycle {
        lifecycle(&self.lifecycle)
    }

    pub fn into_inner(self) -> Stream {
        self.stream
    }
}

impl<Stream, Incoming, Outgoing> Connection<Stream, Incoming, Outgoing>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
    Incoming: DeserializeOwned,
    Outgoing: Serialize,
{
    pub async fn receive(&mut self) -> Result<Incoming, ProtocolError> {
        ensure_open(&self.lifecycle)?;
        let result = frame::read(&mut self.stream).await;
        finish_io(&self.lifecycle, &result);
        result
    }

    pub async fn send(&mut self, message: &Outgoing) -> Result<(), ProtocolError> {
        ensure_open(&self.lifecycle)?;
        let result = frame::write(&mut self.stream, message).await;
        finish_io(&self.lifecycle, &result);
        result
    }

    pub async fn close(&mut self) -> Result<(), ProtocolError> {
        close_stream(&mut self.stream, &self.lifecycle).await
    }

    pub fn into_split(
        self,
    ) -> (
        ConnectionReader<ReadHalf<Stream>, Incoming>,
        ConnectionWriter<WriteHalf<Stream>, Outgoing>,
    ) {
        let (reader, writer) = tokio::io::split(self.stream);
        (
            ConnectionReader::new(reader, Arc::clone(&self.lifecycle)),
            ConnectionWriter::new(writer, self.lifecycle),
        )
    }
}

pub struct ConnectionReader<Reader, Incoming> {
    reader: Reader,
    lifecycle: LifecycleHandle,
    marker: PhantomData<Incoming>,
}

impl<Reader, Incoming> ConnectionReader<Reader, Incoming> {
    fn new(reader: Reader, lifecycle: LifecycleHandle) -> Self {
        Self {
            reader,
            lifecycle,
            marker: PhantomData,
        }
    }

    pub fn lifecycle(&self) -> ConnectionLifecycle {
        lifecycle(&self.lifecycle)
    }
}

impl<Reader, Incoming> ConnectionReader<Reader, Incoming>
where
    Reader: AsyncRead + Unpin,
    Incoming: DeserializeOwned,
{
    pub async fn receive(&mut self) -> Result<Incoming, ProtocolError> {
        ensure_open(&self.lifecycle)?;
        let result = frame::read(&mut self.reader).await;
        finish_io(&self.lifecycle, &result);
        result
    }
}

pub struct ConnectionWriter<Writer, Outgoing> {
    writer: Writer,
    lifecycle: LifecycleHandle,
    marker: PhantomData<Outgoing>,
}

impl<Writer, Outgoing> ConnectionWriter<Writer, Outgoing> {
    fn new(writer: Writer, lifecycle: LifecycleHandle) -> Self {
        Self {
            writer,
            lifecycle,
            marker: PhantomData,
        }
    }

    pub fn lifecycle(&self) -> ConnectionLifecycle {
        lifecycle(&self.lifecycle)
    }
}

impl<Writer, Outgoing> ConnectionWriter<Writer, Outgoing>
where
    Writer: AsyncWrite + Unpin,
    Outgoing: Serialize,
{
    pub async fn send(&mut self, message: &Outgoing) -> Result<(), ProtocolError> {
        ensure_open(&self.lifecycle)?;
        let result = frame::write(&mut self.writer, message).await;
        finish_io(&self.lifecycle, &result);
        result
    }

    pub async fn close(&mut self) -> Result<(), ProtocolError> {
        close_stream(&mut self.writer, &self.lifecycle).await
    }
}

/// Drain one outbound channel through the shared framed writer.
pub async fn run_writer<Writer, Outgoing>(
    mut writer: ConnectionWriter<Writer, Outgoing>,
    mut outbound: UnboundedReceiver<Outgoing>,
) -> Result<(), ProtocolError>
where
    Writer: AsyncWrite + Unpin,
    Outgoing: Serialize,
{
    while let Some(message) = outbound.recv().await {
        writer.send(&message).await?;
    }
    writer.close().await
}

fn lifecycle(handle: &LifecycleHandle) -> ConnectionLifecycle {
    ConnectionLifecycle::from_u8(handle.load(Ordering::Acquire))
}

fn ensure_open(handle: &LifecycleHandle) -> Result<(), ProtocolError> {
    match lifecycle(handle) {
        ConnectionLifecycle::TlsEstablished | ConnectionLifecycle::Open => {
            handle.store(ConnectionLifecycle::Open.as_u8(), Ordering::Release);
            Ok(())
        }
        ConnectionLifecycle::Closing | ConnectionLifecycle::Closed => {
            Err(ProtocolError::ConnectionClosed)
        }
    }
}

fn finish_io<T>(handle: &LifecycleHandle, result: &Result<T, ProtocolError>) {
    if result.is_err() {
        handle.store(ConnectionLifecycle::Closed.as_u8(), Ordering::Release);
    }
}

async fn close_stream<Stream>(
    stream: &mut Stream,
    lifecycle_handle: &LifecycleHandle,
) -> Result<(), ProtocolError>
where
    Stream: AsyncWrite + Unpin,
{
    match lifecycle(lifecycle_handle) {
        ConnectionLifecycle::Closed => return Ok(()),
        ConnectionLifecycle::Closing => return Err(ProtocolError::ConnectionClosed),
        ConnectionLifecycle::TlsEstablished | ConnectionLifecycle::Open => {}
    }
    lifecycle_handle.store(ConnectionLifecycle::Closing.as_u8(), Ordering::Release);
    let result = stream.shutdown().await.map_err(ProtocolError::Io);
    lifecycle_handle.store(ConnectionLifecycle::Closed.as_u8(), Ordering::Release);
    result
}
