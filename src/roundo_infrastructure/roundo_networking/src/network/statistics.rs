//! Encoded-length observations and the arithmetic sample-mean predictor.

use super::Network;
use crate::{NetworkError, NetworkTypeId};

/// Arithmetic sample mean, with no smoothing/window constants or initial-size
/// guesses. Statistics predict encoded payload length, not Rust layout or UDP
/// packet length. Exact lengths are used after serialization.
#[derive(Clone, Copy, Debug, Default)]
pub struct LengthStatistics {
    pub samples: u128,
    pub encoded_bytes: u128,
}

impl LengthStatistics {
    pub fn predicted_bytes(&self) -> Option<usize> {
        if self.samples == 0 {
            None
        } else {
            usize::try_from(self.encoded_bytes / self.samples).ok()
        }
    }

    pub(super) fn with_sample(self, size: usize) -> Result<Self, NetworkError> {
        Ok(Self {
            samples: self
                .samples
                .checked_add(1)
                .ok_or(NetworkError::SizeOverflow)?,
            encoded_bytes: self
                .encoded_bytes
                .checked_add(size as u128)
                .ok_or(NetworkError::SizeOverflow)?,
        })
    }
}

impl Network {
    pub fn length_statistics<T: 'static>(&self) -> LengthStatistics {
        let id = NetworkTypeId::of::<T>();
        self.statistics.get(&id).copied().unwrap_or_default()
    }
}
