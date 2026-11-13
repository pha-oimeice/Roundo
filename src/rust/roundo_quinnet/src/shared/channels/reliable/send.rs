use bevy::log::trace;
use futures::sink::SinkExt;
use quinn::SendStream;
use tokio::sync::mpsc;
use tokio_util::codec::FramedWrite;

use crate::shared::{
    channels::{
        tasks::{report_connection_failure, requested_close, wait_for_abort, SendChannelTaskData},
        ChannelId, CloseReason, CloseRecv,
    },
    error::{TransportFailure, TransportOperation},
    peer_connection::ConnectionTermination,
};

use super::codec::QuinnetProtocolCodecEncoder;

type FrameSender = FramedWrite<SendStream, QuinnetProtocolCodecEncoder>;

async fn new_uni_frame_sender(
    connection: &quinn::Connection,
    channel_id: ChannelId,
    max_frame_len: usize,
) -> Result<FrameSender, quinn::ConnectionError> {
    let stream = connection.open_uni().await?;
    Ok(FramedWrite::new(
        stream,
        QuinnetProtocolCodecEncoder::new(channel_id, max_frame_len),
    ))
}

// Keep a dequeued frame outside the cancellable send future until the sink
// accepts it. Cancelling poll_ready during graceful close must not lose it;
// once accepted, FramedWrite itself retains any bytes still awaiting flush.
async fn send_pending_frame(
    sender: &mut FrameSender,
    pending: &mut Option<bytes::Bytes>,
    id: ChannelId,
) -> Result<(), TransportFailure> {
    if pending.is_some() {
        futures::future::poll_fn(|cx| sender.poll_ready_unpin(cx))
            .await
            .map_err(|error| {
                TransportFailure::io(TransportOperation::SendFrame, Some(id), error)
            })?;
        if let Some(bytes) = pending.take() {
            sender.start_send_unpin(bytes).map_err(|error| {
                TransportFailure::io(TransportOperation::SendFrame, Some(id), error)
            })?;
        }
    }
    sender
        .flush()
        .await
        .map_err(|error| TransportFailure::io(TransportOperation::SendFrame, Some(id), error))
}

pub(crate) async fn ordered_reliable_channel_task(
    mut task: SendChannelTaskData,
    max_frame_len: usize,
) {
    if task.termination.reason().is_some() {
        return;
    }
    let connection = task.connection.clone();
    let id = task.id;
    let mut frame_sender = None;
    let mut pending = None;
    let result = if task.draining {
        Ok(CloseReason::LocalOrder)
    } else {
        tokio::select! {
            biased;
            close = requested_close(&mut task.close_recv, &mut task.channel_close_recv, &task.termination) => {
                close.map_err(|error| TransportFailure::close_signal(id, error))
            }
            result = async {
                let stream = new_uni_frame_sender(&connection, id, max_frame_len)
                    .await
                    .map_err(|error| TransportFailure::quic(TransportOperation::OpenStream, Some(id), error))?;
                let sender = frame_sender.insert(stream);
                while let Some(bytes) = task.bytes_recv.recv().await {
                    pending = Some(bytes);
                    send_pending_frame(sender, &mut pending, id).await?;
                }
                Ok::<_, TransportFailure>(CloseReason::LocalOrder)
            } => result,
        }
    };
    match result {
        Ok(CloseReason::ConnectionLost) => return,
        Err(failure) => {
            report_connection_failure(&task.termination, &task.from_channels_send, failure).await;
            return;
        }
        Ok(CloseReason::LocalOrder) => {}
    }

    task.bytes_recv.close();
    if frame_sender.is_none() && pending.is_none() && task.bytes_recv.is_empty() {
        return;
    }
    // If local close interrupted opening, queued data still needs a stream.
    // Keep the existing stream when opening already completed; never restart
    // an ordered stream simply because its owning channel is closing.
    let result = tokio::select! {
        biased;
        abort = wait_for_abort(&mut task.close_recv, &task.termination) => {
            abort.map_err(|error| TransportFailure::close_signal(id, error))
        }
        result = async {
            let mut sender = match frame_sender {
                Some(sender) => sender,
                None => new_uni_frame_sender(&connection, id, max_frame_len)
                    .await
                    .map_err(|error| TransportFailure::quic(TransportOperation::OpenStream, Some(id), error))?,
            };
            if pending.is_some() {
                send_pending_frame(&mut sender, &mut pending, id).await?;
            }
            while let Some(bytes) = task.bytes_recv.recv().await {
                pending = Some(bytes);
                send_pending_frame(&mut sender, &mut pending, id).await?;
            }
            sender.flush().await.map_err(|error| {
                TransportFailure::io(TransportOperation::FlushStream, Some(id), error)
            })?;
            sender.into_inner().finish().map_err(|error| {
                TransportFailure::io(TransportOperation::FinishStream, Some(id), std::io::Error::other(error))
            })
        } => result,
    };
    if let Err(failure) = result {
        report_connection_failure(&task.termination, &task.from_channels_send, failure).await;
    }
}

pub(crate) async fn unordered_reliable_channel_task(
    mut task: SendChannelTaskData,
    max_frame_len: usize,
) {
    if task.termination.reason().is_some() {
        return;
    }
    let stream_close_recv = task.close_recv.resubscribe();
    let result = if task.draining {
        Ok(CloseReason::LocalOrder)
    } else {
        tokio::select! {
            biased;
            close = requested_close(&mut task.close_recv, &mut task.channel_close_recv, &task.termination) => close,
            _ = async {
                while let Some(bytes) = task.bytes_recv.recv().await {
                    spawn_unordered_message(
                        &task.connection, task.id, &task.termination,
                        &task.from_channels_send, &task.channels_keepalive,
                        stream_close_recv.resubscribe(), bytes, max_frame_len,
                    );
                }
            } => Ok(CloseReason::LocalOrder),
        }
    };
    match result {
        Ok(CloseReason::ConnectionLost) => return,
        Err(error) => {
            report_connection_failure(
                &task.termination,
                &task.from_channels_send,
                TransportFailure::close_signal(task.id, error),
            )
            .await;
            return;
        }
        Ok(CloseReason::LocalOrder) => {}
    }
    task.bytes_recv.close();
    while let Some(bytes) = task.bytes_recv.recv().await {
        if task.termination.reason().is_some() {
            break;
        }
        spawn_unordered_message(
            &task.connection,
            task.id,
            &task.termination,
            &task.from_channels_send,
            &task.channels_keepalive,
            task.close_recv.resubscribe(),
            bytes,
            max_frame_len,
        );
    }
    trace!(
        "Unordered reliable channel stopped admitting messages: channel={}",
        task.id
    );
}

fn spawn_unordered_message(
    connection: &quinn::Connection,
    id: ChannelId,
    termination: &ConnectionTermination,
    from_channels_send: &mpsc::Sender<crate::shared::channels::ChannelAsyncMessage>,
    keepalive: &mpsc::Sender<()>,
    mut close_recv: CloseRecv,
    bytes: bytes::Bytes,
    max_frame_len: usize,
) {
    let connection = connection.clone();
    let termination = termination.clone();
    let from_channels_send = from_channels_send.clone();
    let keepalive = keepalive.clone();
    tokio::spawn(async move {
        let result = tokio::select! {
            biased;
            abort = wait_for_abort(&mut close_recv, &termination) => {
                abort.map_err(|error| TransportFailure::close_signal(id, error))
            }
            result = async {
                let mut sender = new_uni_frame_sender(&connection, id, max_frame_len)
                    .await
                    .map_err(|error| TransportFailure::quic(TransportOperation::OpenStream, Some(id), error))?;
                sender.send(bytes).await.map_err(|error| {
                    TransportFailure::io(TransportOperation::SendFrame, Some(id), error)
                })?;
                sender.into_inner().finish().map_err(|error| {
                    TransportFailure::io(TransportOperation::FinishStream, Some(id), std::io::Error::other(error))
                })
            } => result,
        };
        if let Err(failure) = result {
            report_connection_failure(&termination, &from_channels_send, failure).await;
        }
        drop(keepalive);
    });
}
