use serde::{Deserialize, Serialize};

/// Monotonic cache version advanced after each successful update.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct UpdateVersion(u64);

impl UpdateVersion {
    pub const INITIAL: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    pub const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }

    pub fn advance(&mut self) -> Self {
        *self = self.next();
        *self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advances_once_per_update() {
        let mut version = UpdateVersion::INITIAL;

        assert_eq!(version.advance(), UpdateVersion::new(1));
        assert_eq!(version.advance(), UpdateVersion::new(2));
        assert_eq!(version.value(), 2);
    }

    #[test]
    fn wraps_without_reserving_a_sentinel() {
        assert_eq!(UpdateVersion::new(u64::MAX).next(), UpdateVersion::INITIAL);
    }
}
