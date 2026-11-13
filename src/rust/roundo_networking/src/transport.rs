//! Opaque byte transfers over roundo_quinnet. No game messages or priorities.
//!
//! One acknowledged transfer per direction bounds submitted application bytes to
//! 64 KiB per connection, independently of producers' queue capacities. ACK means
//! bytes were delivered to the receiving adapter, not that gameplay committed.
//! Quinnet owns framing, reliability, TLS and all QUIC scheduling/overhead.

use crate::NetworkError;
use roundo_quinnet::shared::channels::{ChannelConfig, SendChannelsConfiguration};
use bytes::Bytes;

pub(crate) const MAX_TRANSFER_BYTES: usize = 64 * 1024;
const SEQUENCE_BYTES: usize = size_of::<u64>();
// Reserve one ACK slot as well as the data sequence header. A peer cannot
// produce its next conforming transfer until that ACK has reached it.
const MAX_DATA_BYTES: usize = MAX_TRANSFER_BYTES - SEQUENCE_BYTES;
pub(crate) const MAX_BATCH_BYTES: usize = MAX_DATA_BYTES - SEQUENCE_BYTES;
pub(crate) const DATA_CHANNEL: u8 = 0;
pub(crate) const ACK_CHANNEL: u8 = 1;

pub(crate) fn channels() -> SendChannelsConfiguration {
    SendChannelsConfiguration::from_configs(vec![
        ChannelConfig::OrderedReliable {
            max_frame_size: MAX_DATA_BYTES,
        },
        ChannelConfig::OrderedReliable {
            max_frame_size: SEQUENCE_BYTES,
        },
    ])
    .expect("two reliable byte channels fit Quinnet's channel space")
}

#[derive(Default)]
pub(crate) struct ByteTransport {
    prepared: Option<Bytes>,
    flight: Option<u64>,
    send_sequence: u64,
    receive_sequence: u64,
    ack: Option<u64>,
}
impl ByteTransport {
    pub(crate) fn can_prepare(&self) -> bool {
        self.flight.is_none() && self.prepared.is_none()
    }
    pub(crate) fn idle(&self) -> bool {
        self.can_prepare() && self.ack.is_none()
    }

    pub(crate) fn prepare(&mut self, bytes: Vec<u8>) -> Result<(), NetworkError> {
        if !self.can_prepare() || bytes.is_empty() || bytes.len() > MAX_BATCH_BYTES {
            return Err(NetworkError::new("invalid byte transfer admission"));
        }
        let mut transfer = Vec::with_capacity(SEQUENCE_BYTES + bytes.len());
        transfer.extend_from_slice(&self.send_sequence.to_be_bytes());
        transfer.extend_from_slice(&bytes);
        self.prepared = Some(transfer.into());
        Ok(())
    }

    /// `Ok(false)` means Quinnet's local queue is full. Retain the exact bytes;
    /// never interpret local queue admission as network progress or replenish
    /// the transfer window before a matching remote acknowledgment.
    pub(crate) fn flush(
        &mut self,
        mut send: impl FnMut(u8, Bytes) -> Result<bool, NetworkError>,
    ) -> Result<(), NetworkError> {
        if let Some(sequence) = self.ack {
            if send(ACK_CHANNEL, Bytes::copy_from_slice(&sequence.to_be_bytes()))? {
                self.ack = None;
            } else {
                return Ok(());
            }
        }
        if let Some(bytes) = self.prepared.as_ref() {
            if send(DATA_CHANNEL, bytes.clone())? {
                self.prepared = None;
                self.flight = Some(self.send_sequence);
                self.send_sequence = self
                    .send_sequence
                    .checked_add(1)
                    .ok_or_else(|| NetworkError::new("byte transfer sequence exhausted"))?;
            }
        }
        Ok(())
    }

    pub(crate) fn receive(
        &mut self,
        channel: u8,
        bytes: Bytes,
    ) -> Result<Option<Bytes>, NetworkError> {
        if bytes.len() < SEQUENCE_BYTES {
            return Err(NetworkError::new("incomplete byte transfer header"));
        }
        let sequence =
            u64::from_be_bytes(bytes[..SEQUENCE_BYTES].try_into().expect("checked header"));
        match channel {
            ACK_CHANNEL => {
                if bytes.len() != SEQUENCE_BYTES || self.flight != Some(sequence) {
                    return Err(NetworkError::new("unexpected byte transfer acknowledgment"));
                }
                self.flight = None;
                Ok(None)
            }
            DATA_CHANNEL => {
                if bytes.len() <= SEQUENCE_BYTES
                    || bytes.len() > MAX_DATA_BYTES
                    || sequence != self.receive_sequence
                    || self.ack.is_some()
                {
                    return Err(NetworkError::new("invalid byte transfer sequence or size"));
                }
                self.receive_sequence = self
                    .receive_sequence
                    .checked_add(1)
                    .ok_or_else(|| NetworkError::new("byte receive sequence exhausted"))?;
                self.ack = Some(sequence);
                Ok(Some(bytes.slice(SEQUENCE_BYTES..)))
            }
            _ => Err(NetworkError::new("unknown byte transport channel")),
        }
    }
}
