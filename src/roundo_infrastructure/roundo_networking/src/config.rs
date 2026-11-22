//! Application-layer deadline and batching configuration.

use crate::NetworkError;
use std::num::{NonZeroU64, NonZeroUsize};

/// Relative application flush deadline measured in the enclosing World's
/// FixedUpdate cycles, not a guarantee of network delivery or remote commit.
/// The default value is one tick; sync interfaces require an explicit Some
/// to use a deadline. None does not implicitly select this default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Deadline {
    pub ticks: NonZeroU64,
}

impl Deadline {
    pub fn new(ticks: u64) -> Result<Self, NetworkError> {
        NonZeroU64::new(ticks)
            .map(|ticks| Self { ticks })
            .ok_or(NetworkError::InvalidDeadline)
    }
}
impl Default for Deadline {
    fn default() -> Self {
        Self {
            ticks: NonZeroU64::MIN,
        }
    }
}

/// Application-only early-flush triggers. Neither is an admission/capacity
/// limit. The caller explicitly chooses each trigger or disables it with None;
/// no implicit byte/count defaults are installed. Default disables both triggers.
#[derive(Clone, Copy, Debug, Default)]
pub struct BatchPolicy {
    pub payload_bytes_trigger: Option<NonZeroUsize>,
    pub entries_trigger: Option<NonZeroUsize>,
}

impl BatchPolicy {
    pub fn new(
        payload_bytes_trigger: Option<NonZeroUsize>,
        entries_trigger: Option<NonZeroUsize>,
    ) -> Self {
        Self {
            payload_bytes_trigger,
            entries_trigger,
        }
    }

    pub(crate) fn should_flush(self, bytes: usize, entries: usize) -> bool {
        self.payload_bytes_trigger
            .is_some_and(|trigger| bytes >= trigger.get())
            || self
                .entries_trigger
                .is_some_and(|trigger| entries >= trigger.get())
    }

    pub(crate) fn prefer_flush_before(self, bytes: usize, predicted: Option<usize>) -> bool {
        let (Some(target), Some(predicted)) = (self.payload_bytes_trigger, predicted) else {
            return false;
        };
        if bytes == 0 {
            return false;
        }
        let target = target.get();
        match bytes.checked_add(predicted) {
            Some(next) => next > target && bytes.abs_diff(target) < next.abs_diff(target),
            None => true,
        }
    }
}
