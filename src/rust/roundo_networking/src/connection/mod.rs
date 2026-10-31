//! Typed, framed streams and their shared I/O lifecycle.

use crate::ProtocolError;
use crate::frame;
use crate::protocol::{ClientMessage, ServerMessage};
use crate::service::{AdmissionResult, admit};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{mpsc, watch};

/// Shared high-level state of a typed framed connection.
///
/// This is an atomic observation for diagnostics and admission checks, not proof
/// that peer I/O has completed. Split reader and writer halves share the state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionLifecycle {
    /// The transport stream exists; no application I/O has started yet.
    Established,
    /// At least one framed read or write has started successfully.
    Open,
    /// Local asynchronous-write shutdown is in progress.
    Closing,
    /// Local shutdown completed or a framed I/O operation failed.
    Closed,
}

impl ConnectionLifecycle {
    fn as_u8(self) -> u8 {
        match self {
            Self::Established => 0,
            Self::Open => 1,
            Self::Closing => 2,
            Self::Closed => 3,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Established,
            1 => Self::Open,
            2 => Self::Closing,
            _ => Self::Closed,
        }
    }
}

type LifecycleHandle = Arc<AtomicU8>;

/// A typed framed connection over a paired reader and writer.
///
/// `Incoming` and `Outgoing` are intentionally separate. A client connection
/// is therefore unable to read a `ClientMessage`, and a server connection is
/// unable to send one.
pub struct Connection<Stream, Incoming, Outgoing> {
    stream: Stream,
    lifecycle: LifecycleHandle,
    marker: PhantomData<fn(Incoming) -> Outgoing>,
}

/// Client-side connection that receives server messages and sends client messages.
pub type ClientConnection<Stream> = Connection<Stream, ServerMessage, ClientMessage>;
/// Server-side connection that receives client messages and sends server messages.
pub type ServerConnection<Stream> = Connection<Stream, ClientMessage, ServerMessage>;

impl<Stream, Incoming, Outgoing> Connection<Stream, Incoming, Outgoing> {
    /// Wraps an established transport without performing I/O.
    pub fn new(stream: Stream) -> Self {
        Self {
            stream,
            lifecycle: Arc::new(AtomicU8::new(ConnectionLifecycle::Established.as_u8())),
            marker: PhantomData,
        }
    }

    /// Returns a momentary observation of the shared lifecycle.
    pub fn lifecycle(&self) -> ConnectionLifecycle {
        lifecycle(&self.lifecycle)
    }

    /// Removes the typed framing wrapper without shutting down the transport.
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
    /// Reads and decodes exactly one length-prefixed message.
    ///
    /// Any framing, decode, or I/O error marks the shared lifecycle closed.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::ConnectionClosed`] after closing starts, or the
    /// framing/transport error from reading the next message.
    pub async fn receive(&mut self) -> Result<Incoming, ProtocolError> {
        ensure_open(&self.lifecycle)?;
        let result = frame::read(&mut self.stream).await;
        finish_io(&self.lifecycle, &result);
        result
    }

    /// Encodes and writes exactly one length-prefixed message.
    ///
    /// Failure may leave partial frame bytes in the transport and marks the
    /// shared lifecycle closed; callers must not retry on the same connection.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::ConnectionClosed`] after closing starts, or an
    /// encode/frame-size/transport error.
    pub async fn transmit(&mut self, message: &Outgoing) -> Result<(), ProtocolError> {
        ensure_open(&self.lifecycle)?;
        let result = frame::write(&mut self.stream, message).await;
        finish_io(&self.lifecycle, &result);
        result
    }

    /// Shuts down the asynchronous write side and marks the wrapper closed.
    ///
    /// Calling this after closure succeeds without touching the transport.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::ConnectionClosed`] if another finish is already
    /// in progress, or [`ProtocolError::Io`] if transport shutdown fails.
    pub async fn finish(&mut self) -> Result<(), ProtocolError> {
        close_stream(&mut self.stream, &self.lifecycle).await
    }

    /// Splits transport ownership into independently usable typed halves.
    ///
    /// Both halves share one lifecycle: an I/O failure or writer finish prevents
    /// subsequent operations through either half.
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

/// Owned read half of a typed connection with shared lifecycle state.
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

    /// Returns a momentary observation shared with the writer half.
    pub fn lifecycle(&self) -> ConnectionLifecycle {
        lifecycle(&self.lifecycle)
    }
}

impl<Reader, Incoming> ConnectionReader<Reader, Incoming>
where
    Reader: AsyncRead + Unpin,
    Incoming: DeserializeOwned,
{
    /// Reads and decodes one frame, closing shared lifecycle state on failure.
    pub async fn receive(&mut self) -> Result<Incoming, ProtocolError> {
        ensure_open(&self.lifecycle)?;
        let result = frame::read(&mut self.reader).await;
        finish_io(&self.lifecycle, &result);
        result
    }
}

/// Owned write half of a typed connection with shared lifecycle state.
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

    /// Returns a momentary observation shared with the reader half.
    pub fn lifecycle(&self) -> ConnectionLifecycle {
        lifecycle(&self.lifecycle)
    }
}

impl<Writer, Outgoing> ConnectionWriter<Writer, Outgoing>
where
    Writer: AsyncWrite + Unpin,
    Outgoing: Serialize,
{
    /// Encodes and writes one frame, closing shared lifecycle state on failure.
    ///
    /// A failed write may have emitted a partial frame and must not be retried on
    /// this connection.
    pub async fn transmit(&mut self, message: &Outgoing) -> Result<(), ProtocolError> {
        ensure_open(&self.lifecycle)?;
        let result = frame::write(&mut self.writer, message).await;
        finish_io(&self.lifecycle, &result);
        result
    }

    /// Shuts down the write half and closes lifecycle state shared with the reader.
    pub async fn finish(&mut self) -> Result<(), ProtocolError> {
        close_stream(&mut self.writer, &self.lifecycle).await
    }
}

/// Drains an unbounded outbound channel through a framed writer.
///
/// Messages are transmitted in channel receive order. When all senders drop,
/// the write side is shut down. The first transmission or shutdown error stops
/// draining; unsent queued messages are dropped with the receiver.
pub async fn run_writer<Writer, Outgoing>(
    mut writer: ConnectionWriter<Writer, Outgoing>,
    mut outbound: mpsc::UnboundedReceiver<Outgoing>,
) -> Result<(), ProtocolError>
where
    Writer: AsyncWrite + Unpin,
    Outgoing: Serialize,
{
    while let Some(message) = outbound.recv().await {
        writer.transmit(&message).await?;
    }
    writer.finish().await
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConnectionIoSide {
    Reader,
    Writer,
}

impl ConnectionIoSide {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Reader => "reader",
            Self::Writer => "writer",
        }
    }
}

pub(crate) enum ConnectionIoEvent<Incoming> {
    Message(Incoming),
    Stopped {
        side: ConnectionIoSide,
        result: Result<(), ProtocolError>,
    },
}

pub(crate) struct ConnectionIo<Incoming, Outgoing> {
    outbound: mpsc::Sender<Outgoing>,
    events: mpsc::UnboundedReceiver<ConnectionIoEvent<Incoming>>,
    shutdown: watch::Sender<bool>,
}

impl<Incoming, Outgoing> ConnectionIo<Incoming, Outgoing>
where
    Incoming: DeserializeOwned + Send + 'static,
    Outgoing: Serialize + Send + Sync + 'static,
{
    pub(crate) fn spawn<Stream>(
        connection: Connection<Stream, Incoming, Outgoing>,
        outbound_capacity: usize,
    ) -> Self
    where
        Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (reader, writer) = connection.into_split();
        let (outbound, outbound_receiver) = mpsc::channel(outbound_capacity.max(1));
        let (events, event_receiver) = mpsc::unbounded_channel();
        let (shutdown, shutdown_receiver) = watch::channel(false);

        let reader_events = events.clone();
        let reader_shutdown = shutdown.clone();
        let reader_shutdown_receiver = shutdown_receiver.clone();
        tokio::spawn(async move {
            let result = run_reader(reader, reader_events.clone(), reader_shutdown_receiver).await;
            let event_result = reader_events.send(ConnectionIoEvent::Stopped {
                side: ConnectionIoSide::Reader,
                result,
            });
            if event_result.is_err() {
                log::trace!("reader stopped after its Connection I/O owner was dropped");
            }
            let shutdown_result = reader_shutdown.send(true);
            if shutdown_result.is_err() {
                log::trace!("reader stopped after its writer shutdown receiver was dropped");
            }
        });

        let writer_events = events;
        let writer_shutdown = shutdown.clone();
        tokio::spawn(async move {
            let result = run_managed_writer(writer, outbound_receiver, shutdown_receiver).await;
            let event_result = writer_events.send(ConnectionIoEvent::Stopped {
                side: ConnectionIoSide::Writer,
                result,
            });
            if event_result.is_err() {
                log::trace!("writer stopped after its Connection I/O owner was dropped");
            }
            let shutdown_result = writer_shutdown.send(true);
            if shutdown_result.is_err() {
                log::trace!("writer stopped after its reader shutdown receiver was dropped");
            }
        });

        Self {
            outbound,
            events: event_receiver,
            shutdown,
        }
    }

    pub(crate) fn sender(&self) -> mpsc::Sender<Outgoing> {
        self.outbound.clone()
    }

    pub(crate) fn admit_message(&self, message: Outgoing) -> AdmissionResult {
        admit(&self.outbound, message)
    }

    pub(crate) async fn receive(&mut self) -> Option<ConnectionIoEvent<Incoming>> {
        self.events.recv().await
    }

    pub(crate) fn shutdown(&self) {
        let shutdown_result = self.shutdown.send(true);
        if shutdown_result.is_err() {
            log::trace!("Connection I/O shutdown requested after both workers stopped");
        }
    }
}

impl<Incoming, Outgoing> Drop for ConnectionIo<Incoming, Outgoing> {
    fn drop(&mut self) {
        let shutdown_result = self.shutdown.send(true);
        if shutdown_result.is_err() {
            log::trace!("Connection I/O dropped after both workers stopped");
        }
    }
}

async fn run_reader<Reader, Incoming>(
    mut reader: ConnectionReader<Reader, Incoming>,
    events: mpsc::UnboundedSender<ConnectionIoEvent<Incoming>>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), ProtocolError>
where
    Reader: AsyncRead + Unpin,
    Incoming: DeserializeOwned,
{
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
            message = reader.receive() => {
                let message = message?;
                let event_result = events.send(ConnectionIoEvent::Message(message));
                if event_result.is_err() {
                    return Ok(());
                }
            }
        }
    }
}

async fn run_managed_writer<Writer, Outgoing>(
    mut writer: ConnectionWriter<Writer, Outgoing>,
    mut outbound: mpsc::Receiver<Outgoing>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), ProtocolError>
where
    Writer: AsyncWrite + Unpin,
    Outgoing: Serialize,
{
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return writer.finish().await;
                }
            }
            message = outbound.recv() => match message {
                Some(message) => {
                    tokio::select! {
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() {
                                return writer.finish().await;
                            }
                        }
                        result = writer.transmit(&message) => result?,
                    }
                }
                None => return writer.finish().await,
            }
        }
    }
}

fn lifecycle(handle: &LifecycleHandle) -> ConnectionLifecycle {
    ConnectionLifecycle::from_u8(handle.load(Ordering::Acquire))
}

fn ensure_open(handle: &LifecycleHandle) -> Result<(), ProtocolError> {
    match lifecycle(handle) {
        ConnectionLifecycle::Established | ConnectionLifecycle::Open => {
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
        ConnectionLifecycle::Established | ConnectionLifecycle::Open => {}
    }
    lifecycle_handle.store(ConnectionLifecycle::Closing.as_u8(), Ordering::Release);
    let result = stream.shutdown().await.map_err(ProtocolError::Io);
    lifecycle_handle.store(ConnectionLifecycle::Closed.as_u8(), Ordering::Release);
    result
}
