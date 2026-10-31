//! Source contracts for the Controller-free recipe; no motion writer is installed here.

use crate::{CreatureMotionError, GazeCommandEvent, MovementCommandState};
use roundo_life::{EventData, EventPolicy};

/// A single ground-jump request, not a held axis or a delayed jump buffer.
/// The recipe must publish and consume it in the same authoritative tick.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct JumpCommandEvent;

// Charge a positive byte even for a zero-sized payload so all configured budgets
// have an observable lower bound, independent of Rust's zero-sized representation.
roundo_life::declare_event_data!(JumpCommandEvent, "roundo.creature.jump-command.v1", |_| 1);

/// Constructs the recipe's horizontal movement without changing the old held-up schema.
pub fn horizontal_movement_command(
    right: f32,
    forward: f32,
) -> Result<MovementCommandState, CreatureMotionError> {
    MovementCommandState::new([right, 0.0, forward])
}

/// Explicit capacity and work limits for one one-tick Command FIFO.
/// There is no default: the host must choose a finite capacity before composition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OneTickCommandBudget<E> {
    entries: usize,
    retained_bytes: usize,
    consume_per_tick: usize,
    source: std::marker::PhantomData<fn() -> E>,
}

impl OneTickCommandBudget<JumpCommandEvent> {
    /// Jump entries each charge one retained byte, including the zero-sized payload.
    pub fn new(
        entries: usize,
        retained_bytes: usize,
        consume_per_tick: usize,
    ) -> Result<Self, CreatureMotionError> {
        Self::for_payload(
            entries,
            retained_bytes,
            consume_per_tick,
            JumpCommandEvent.retained_size(),
        )
    }
}

impl OneTickCommandBudget<GazeCommandEvent> {
    /// Rejects insufficient storage for every accepted fixed-size gaze payload.
    pub fn new(
        entries: usize,
        retained_bytes: usize,
        consume_per_tick: usize,
    ) -> Result<Self, CreatureMotionError> {
        Self::for_payload(
            entries,
            retained_bytes,
            consume_per_tick,
            std::mem::size_of::<GazeCommandEvent>(),
        )
    }
}

impl<E> OneTickCommandBudget<E> {
    fn for_payload(
        entries: usize,
        retained_bytes: usize,
        consume_per_tick: usize,
        maximum_payload_bytes: usize,
    ) -> Result<Self, CreatureMotionError> {
        let required = entries
            .checked_mul(maximum_payload_bytes)
            .ok_or(CreatureMotionError::InvalidValue)?;
        if entries == 0
            || maximum_payload_bytes == 0
            || retained_bytes < required
            || consume_per_tick < entries
        {
            return Err(CreatureMotionError::InvalidValue);
        }
        Ok(Self {
            entries,
            retained_bytes,
            consume_per_tick,
            source: std::marker::PhantomData,
        })
    }

    pub const fn entries(self) -> usize {
        self.entries
    }

    pub const fn retained_bytes(self) -> usize {
        self.retained_bytes
    }

    pub const fn consume_per_tick(self) -> usize {
        self.consume_per_tick
    }

    /// The same validated limits govern the native Life Interface, lifetime exactly one.
    pub fn event_policy(self) -> EventPolicy {
        EventPolicy::new(self.entries, self.retained_bytes, 1, 1)
            .expect("validated one-tick Command budget")
    }
}
