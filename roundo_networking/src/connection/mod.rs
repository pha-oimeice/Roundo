//! Typed, framed streams and their shared I/O lifecycle.

use crate::ProtocolError;
use crate::frame;
use crate::protocol::{ClientMessage, ServerMessage};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{mpsc, watch};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionLifecycle {
    /// The transport stream exists; no application I/O has happened yet.
    Established,
    Open,
    Closing,
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

pub type ClientConnection<Stream> = Connection<Stream, ServerMessage, ClientMessage>;
pub type ServerConnection<Stream> = Connection<Stream, ClientMessage, ServerMessage>;

impl<Stream, Incoming, Outgoing> Connection<Stream, Incoming, Outgoing> {
    pub fn new(stream: Stream) -> Self {
        Self {
            stream,
            lifecycle: Arc::new(AtomicU8::new(ConnectionLifecycle::Established.as_u8())),
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
    mut outbound: mpsc::UnboundedReceiver<Outgoing>,
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
    outbound: mpsc::UnboundedSender<Outgoing>,
    events: mpsc::UnboundedReceiver<ConnectionIoEvent<Incoming>>,
    shutdown: watch::Sender<bool>,
}

impl<Incoming, Outgoing> ConnectionIo<Incoming, Outgoing>
where
    Incoming: DeserializeOwned + Send + 'static,
    Outgoing: Serialize + Send + Sync + 'static,
{
    pub(crate) fn spawn<Stream>(connection: Connection<Stream, Incoming, Outgoing>) -> Self
    where
        Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (reader, writer) = connection.into_split();
        let (outbound, outbound_receiver) = mpsc::unbounded_channel();
        let (events, event_receiver) = mpsc::unbounded_channel();
        let (shutdown, shutdown_receiver) = watch::channel(false);

        let reader_events = events.clone();
        let reader_shutdown = shutdown.clone();
        let reader_shutdown_receiver = shutdown_receiver.clone();
        tokio::spawn(async move {
            let result = run_reader(reader, reader_events.clone(), reader_shutdown_receiver).await;
            let _ = reader_events.send(ConnectionIoEvent::Stopped {
                side: ConnectionIoSide::Reader,
                result,
            });
            let _ = reader_shutdown.send(true);
        });

        let writer_events = events;
        let writer_shutdown = shutdown.clone();
        tokio::spawn(async move {
            let result = run_managed_writer(writer, outbound_receiver, shutdown_receiver).await;
            let _ = writer_events.send(ConnectionIoEvent::Stopped {
                side: ConnectionIoSide::Writer,
                result,
            });
            let _ = writer_shutdown.send(true);
        });

        Self {
            outbound,
            events: event_receiver,
            shutdown,
        }
    }

    pub(crate) fn sender(&self) -> mpsc::UnboundedSender<Outgoing> {
        self.outbound.clone()
    }

    pub(crate) fn send(&self, message: Outgoing) -> bool {
        self.outbound.send(message).is_ok()
    }

    pub(crate) async fn receive(&mut self) -> Option<ConnectionIoEvent<Incoming>> {
        self.events.recv().await
    }

    pub(crate) fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }
}

impl<Incoming, Outgoing> Drop for ConnectionIo<Incoming, Outgoing> {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
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
                if events.send(ConnectionIoEvent::Message(message)).is_err() {
                    return Ok(());
                }
            }
        }
    }
}

async fn run_managed_writer<Writer, Outgoing>(
    mut writer: ConnectionWriter<Writer, Outgoing>,
    mut outbound: mpsc::UnboundedReceiver<Outgoing>,
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
                    return writer.close().await;
                }
            }
            message = outbound.recv() => match message {
                Some(message) => {
                    tokio::select! {
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() {
                                return writer.close().await;
                            }
                        }
                        result = writer.send(&message) => result?,
                    }
                }
                None => return writer.close().await,
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

#[cfg(test)]
mod tests {
    use super::{ClientConnection, ConnectionIo, ConnectionIoEvent};
    use crate::frame;
    use crate::protocol::{
        ClientGameMessage, ClientMessage, ControllerCommand, Movement3DAction,
        PlayerControllerCommand, PlayerId, PlayerState, SceneId, ServerGameMessage, ServerMessage,
    };
    use tokio::io::{AsyncWriteExt, split};
    use tokio::time::{Duration, sleep, timeout};

    const TEST_TIMEOUT: Duration = Duration::from_secs(1);

    #[tokio::test]
    async fn outbound_message_does_not_cancel_a_partial_inbound_frame() {
        let (client_stream, server_stream) = tokio::io::duplex(8);
        let mut connection = ConnectionIo::spawn(ClientConnection::new(client_stream));
        let (mut server_reader, mut server_writer) = split(server_stream);

        let inbound = ServerMessage::Game(ServerGameMessage::PlayerState {
            state: PlayerState {
                player_id: PlayerId(7),
                scene_id: SceneId::S1,
                translation: [1.0, 2.0, 3.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
            },
        });
        let payload = frame::encode(&inbound).expect("server message should encode");
        let mut frame_bytes = (payload.len() as u32).to_be_bytes().to_vec();
        frame_bytes.extend(payload);
        let split_at = 4 + (frame_bytes.len() - 4) / 2;

        server_writer
            .write_all(&frame_bytes[..split_at])
            .await
            .expect("first frame fragment should write");
        sleep(Duration::from_millis(10)).await;

        let outbound = ClientMessage::Game(ClientGameMessage::UsePlayerController {
            command: PlayerControllerCommand::Movement3D(ControllerCommand {
                sequence: 1,
                action: Movement3DAction {
                    translation_delta: [0.0, 0.0, 1.0],
                },
            }),
        });
        assert!(connection.send(outbound.clone()));
        let received_outbound: ClientMessage =
            timeout(TEST_TIMEOUT, frame::read(&mut server_reader))
                .await
                .expect("client message should arrive")
                .expect("client message should decode");
        assert_eq!(received_outbound, outbound);

        server_writer
            .write_all(&frame_bytes[split_at..])
            .await
            .expect("remaining frame fragment should write");
        let received_inbound = timeout(TEST_TIMEOUT, connection.receive())
            .await
            .expect("server message should arrive")
            .expect("connection event channel should remain open");
        match received_inbound {
            ConnectionIoEvent::Message(message) => assert_eq!(message, inbound),
            ConnectionIoEvent::Stopped { side, result } => {
                panic!(
                    "connection {} stopped unexpectedly: {result:?}",
                    side.name()
                )
            }
        }
    }
}
