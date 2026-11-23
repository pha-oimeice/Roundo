//! Integer identity allocation with an explicit starting value.

use num_traits::PrimInt;

/// Allocates recycled identities first, then monotonically increasing integers.
///
/// The caller owns the rules for recycling: the generator cannot determine
/// whether external references to an identity are still alive. No starting
/// value, wrapping policy, or automatic recycling is inferred.
#[derive(Debug)]
pub struct IdGenerator<T: PrimInt> {
    recycled: Vec<T>,
    next_id: Option<T>,
}

impl<T: PrimInt> IdGenerator<T> {
    pub fn new(first: T) -> Self {
        Self {
            recycled: Vec::new(),
            next_id: Some(first),
        }
    }

    /// Returns `None` after the integer range and recycled pool are exhausted.
    pub fn get_next(&mut self) -> Option<T> {
        if let Some(value) = self.recycled.pop() {
            return Some(value);
        }
        let value = self.next_id?;
        self.next_id = value.checked_add(&T::one());
        Some(value)
    }

    /// Makes an identity available again. Call only after retiring every use of
    /// the previous allocation; duplicate recycling can issue duplicate IDs.
    pub fn recycle(&mut self, value: T) {
        self.recycled.push(value);
    }
}
