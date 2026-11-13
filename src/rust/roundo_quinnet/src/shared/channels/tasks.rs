use bevy::log::trace;
use bytes::Bytes;
use quinn::VarInt;
use tokio::sync::{
    broadcast,
    mpsc::{self},
};

use crate::shared::{
    error::{TransportError, TransportFailure, TransportOperation},
    peer_connection::ConnectionTermination,
};

use crate::shared::channels::{
    reliable::{
        recv::reliable_channels_receiver_task,
        send::{ordered_reliable_channel_task, unordered_reliable_channel_task},
    },
    unreliable::{recv::unreliable_channel_receiver_task, send::unreliable_channel_task},
    ChannelAsyncMessage, ChannelConfig, ChannelId, ChannelSyncMessage, CloseReason, CloseRecv,
};

/// Spawn a task to handle send channels creation for this connection
pub(crate) fn spawn_send_channels_tasks_spawner(
    connection_handle: quinn::Connection,
    close_recv: broadcast::Receiver<CloseReason>,
    to_channels_recv: mpsc::Receiver<ChannelSyncMessage>,
    from_channels_send: mpsc::Sender<ChannelAsyncMessage>,
    termination: ConnectionTermination,
) {
    tokio::spawn(async move {
        send_channels_tasks_spawner(
            connection_handle,
            close_recv,
            to_channels_recv,
            from_channels_send,
            termination,
        )
        .await
    });
}

pub(crate) struct SendChannelTaskData {
    pub(crate) connection: quinn::Connection,
    pub(crate) termination: ConnectionTermination,
    pub(crate) draining: bool,
    pub(crate) id: ChannelId,
    pub(crate) channels_keepalive: mpsc::Sender<()>,
    pub(crate) from_channels_send: mpsc::Sender<ChannelAsyncMessage>,
    pub(crate) close_recv: CloseRecv,
    pub(crate) channel_close_recv: mpsc::Receiver<()>,
    pub(crate) bytes_recv: mpsc::Receiver<Bytes>,
}

pub(crate) async fn send_channels_tasks_spawner(
    connection: quinn::Connection,
    mut close_recv: broadcast::Receiver<CloseReason>,
    mut to_channels_recv: mpsc::Receiver<ChannelSyncMessage>,
    from_channels_send: mpsc::Sender<ChannelAsyncMessage>,
    termination: ConnectionTermination,
) {
    // Use an mpsc channel where, instead of sending messages, we wait for the channel to be closed, which happens when every sender has been dropped. We can't use a JoinSet as simply here since we would also need to drain closed channels from it.
    let (channel_tasks_keepalive, mut channel_tasks_waiter) = mpsc::channel::<()>(1);

    {
        let stream_close_recv = close_recv.resubscribe();
        let spawn_channel = |message: ChannelSyncMessage, draining: bool| {
            let ChannelSyncMessage::CreateChannel {
                id, config, bytes_to_channel_recv: bytes_recv, channel_close_recv,
            } = message;
            let task = SendChannelTaskData {
                connection: connection.clone(),
                termination: termination.clone(),
                draining,
                id,
                channels_keepalive: channel_tasks_keepalive.clone(),
                from_channels_send: from_channels_send.clone(),
                close_recv: stream_close_recv.resubscribe(),
                channel_close_recv,
                bytes_recv,
            };
            match config {
                ChannelConfig::OrderedReliable { max_frame_size } => {
                    tokio::spawn(ordered_reliable_channel_task(task, max_frame_size));
                }
                ChannelConfig::UnorderedReliable { max_frame_size } => {
                    tokio::spawn(unordered_reliable_channel_task(task, max_frame_size));
                }
                ChannelConfig::Unreliable => {
                    tokio::spawn(unreliable_channel_task(task));
                }
            }
        };
        let graceful = loop {
            tokio::select! {
                biased;
                _ = termination.ended() => break false,
                reason = close_recv.recv() => match reason {
                    Ok(CloseReason::LocalOrder) | Err(broadcast::error::RecvError::Closed) => break true,
                    Ok(CloseReason::ConnectionLost) => break false,
                    Err(error) => {
                        report_connection_failure(&termination, &from_channels_send, TransportFailure {
                            operation: TransportOperation::ReceiveCloseSignal,
                            channel_id: None,
                            source: TransportError::CloseSignal(error),
                        }).await;
                        break false;
                    }
                },
                message = to_channels_recv.recv() => match message {
                    Some(message) => spawn_channel(message, false),
                    None => break true,
                },
            }
        };
        // A channel may already own admitted bytes while its creation request
        // is still queued. Graceful close must drain those requests too, and
        // explicitly mark the new workers as draining (they missed the signal).
        to_channels_recv.close();
        if graceful {
            while let Some(message) = to_channels_recv.recv().await {
                if termination.reason().is_some() {
                    break;
                }
                spawn_channel(message, true);
            }
        }
    }
    drop(to_channels_recv);

    // Wait for all the channels to have flushed/finished:
    // We drop our sender first because the recv() call otherwise sleeps forever.
    // When every sender has gone out of scope, the recv call will return with an error. We ignore the error.
    drop(channel_tasks_keepalive);
    let _ = channel_tasks_waiter.recv().await;

    connection.close(VarInt::from_u32(0), "closed".as_bytes());
}

// A dropped owner requests graceful draining. A lagged notification cannot be
// assumed graceful because the overwritten signal may have required an abort.
pub(crate) async fn requested_close(
    close_recv: &mut CloseRecv,
    channel_close_recv: &mut mpsc::Receiver<()>,
    termination: &ConnectionTermination,
) -> Result<CloseReason, broadcast::error::RecvError> {
    tokio::select! {
        biased;
        _ = termination.ended() => Ok(CloseReason::ConnectionLost),
        reason = close_recv.recv() => match reason {
            Ok(reason) => Ok(reason),
            Err(broadcast::error::RecvError::Closed) => Ok(CloseReason::LocalOrder),
            Err(error) => Err(error),
        },
        _ = channel_close_recv.recv() => Ok(CloseReason::LocalOrder),
    }
}

// Local graceful close must not cancel already-admitted data. The shared cause
// also catches termination signalled before a new stream task subscribed.
pub(crate) async fn wait_for_abort(
    close_recv: &mut CloseRecv,
    termination: &ConnectionTermination,
) -> Result<(), broadcast::error::RecvError> {
    loop {
        tokio::select! {
            biased;
            _ = termination.ended() => return Ok(()),
            reason = close_recv.recv() => match reason {
                Ok(CloseReason::ConnectionLost) => return Ok(()),
                Ok(CloseReason::LocalOrder) => {}
                Err(broadcast::error::RecvError::Closed) => {
                    // The routing owner is gone, but another stream can still
                    // fail while graceful draining is in progress.
                    termination.ended().await;
                    return Ok(());
                }
                Err(error) => return Err(error),
            }
        }
    }
}

/// The receiving half is private to PeerConnection. A failed notification means
/// that routing owner has been released/reset, not a second transport failure.
/// Record before notification so queue closure and conn.closed() cannot replace
/// the primary cause with a cleanup error.
pub(crate) async fn report_connection_failure(
    termination: &ConnectionTermination,
    sender: &mpsc::Sender<ChannelAsyncMessage>,
    failure: TransportFailure,
) {
    let (reason, primary) = termination.record(failure);
    if primary && sender.send(ChannelAsyncMessage::LostConnection(reason)).await.is_err() {
        trace!("Connection owner released before transport failure notification");
    }
}

pub(crate) fn spawn_recv_channels_tasks(
    connection_handle: quinn::Connection,
    connection_id: u64,
    close_recv: broadcast::Receiver<CloseReason>,
    bytes_incoming_send: mpsc::Sender<(ChannelId, Bytes)>,
) {
    // Spawn a task to listen for reliable messages
    {
        let connection_handle = connection_handle.clone();
        let close_recv = close_recv.resubscribe();
        let bytes_incoming_send = bytes_incoming_send.clone();
        tokio::spawn(async move {
            reliable_channels_receiver_task(
                connection_id,
                connection_handle,
                close_recv,
                bytes_incoming_send,
            )
            .await
        });
    }

    // Spawn a task to listen for unreliable datagrams
    {
        let connection_handle = connection_handle.clone();
        let close_recv = close_recv.resubscribe();
        let bytes_incoming_send = bytes_incoming_send.clone();
        tokio::spawn(async move {
            unreliable_channel_receiver_task(
                connection_id,
                connection_handle,
                close_recv,
                bytes_incoming_send,
            )
            .await
        });
    }
}
