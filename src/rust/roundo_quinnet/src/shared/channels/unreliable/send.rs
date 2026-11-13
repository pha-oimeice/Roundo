use bevy::log::{error, trace, warn};
use bytes::{BufMut, Bytes, BytesMut};
use quinn::SendDatagramError;

use crate::shared::{
    channels::{
        tasks::{report_connection_failure, requested_close, SendChannelTaskData},
        ChannelId, CloseReason, PROTOCOL_HEADER_LEN,
    },
    error::{TransportFailure, TransportOperation},
};

pub(crate) async fn unreliable_channel_task(mut task: SendChannelTaskData) {
    if task.termination.reason().is_some() {
        return;
    }
    let result = if task.draining {
        Ok(CloseReason::LocalOrder)
    } else {
        tokio::select! {
            biased;
            close = requested_close(&mut task.close_recv, &mut task.channel_close_recv, &task.termination) => {
                close.map_err(|error| TransportFailure::close_signal(task.id, error))
            }
            result = async {
                while let Some(bytes) = task.bytes_recv.recv().await {
                    if let Err(error) = send_unreliable_message(&task.connection, bytes, task.id) {
                        match error {
                            SendDatagramError::ConnectionLost(error) => return Err(TransportFailure::quic(
                                TransportOperation::SendFrame, Some(task.id), error,
                            )),
                            error => error!("Error while sending message on Unreliable Channel: {error}"),
                        }
                    }
                }
                Ok(CloseReason::LocalOrder)
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
    while let Some(bytes) = task.bytes_recv.recv().await {
        if task.termination.reason().is_some() {
            break;
        }
        if let Err(error) = send_unreliable_message(&task.connection, bytes, task.id) {
            match error {
                SendDatagramError::ConnectionLost(error) => {
                    report_connection_failure(
                        &task.termination,
                        &task.from_channels_send,
                        TransportFailure::quic(TransportOperation::SendFrame, Some(task.id), error),
                    )
                    .await;
                    return;
                }
                error => warn!("Failed to send remaining Unreliable Channel message: {error}"),
            }
        }
    }
    trace!("Unreliable channel finished: channel={}", task.id);
}

fn send_unreliable_message(
    connection: &quinn::Connection,
    msg_bytes: Bytes,
    channel_id: ChannelId,
) -> Result<(), SendDatagramError> {
    let mut datagram = BytesMut::with_capacity(PROTOCOL_HEADER_LEN + msg_bytes.len());
    datagram.put_u8(channel_id);
    datagram.extend_from_slice(&msg_bytes[..]);
    connection.send_datagram(datagram.into())
}
