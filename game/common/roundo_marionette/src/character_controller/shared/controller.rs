//! Generic sequence validation for discrete controller actions.

use bevy::prelude::Component;
use roundo_contracts::ControllerCommand;
use std::{fmt::Debug, marker::PhantomData};

/// Defines domain validation for a sequenced controller payload.
pub trait ControllerAction: Copy + Debug + Send + Sync + 'static {
    fn is_valid(self) -> bool;
}

#[derive(Clone, Debug, Default)]
/// Monotonic sequence shared by command issuance and acceptance.
struct ControllerSequence {
    last_accepted: u64,
}

impl ControllerSequence {
    // Sequence numbers advance only for valid actions.
    fn issue<Action>(
        &mut self,
        action: Action,
    ) -> Result<ControllerCommand<Action>, ControllerError>
    where
        Action: ControllerAction,
    {
        if !action.is_valid() {
            return Err(ControllerError::InvalidAction);
        }
        let sequence = self
            .last_accepted
            .checked_add(1)
            .ok_or(ControllerError::SequenceExhausted)?;
        self.last_accepted = sequence;
        Ok(ControllerCommand { sequence, action })
    }

    // Acceptance rejects replay before validating the action payload.
    fn apply_command<Action>(
        &mut self,
        command: ControllerCommand<Action>,
    ) -> Result<Action, ControllerError>
    where
        Action: ControllerAction,
    {
        if command.sequence <= self.last_accepted {
            return Err(ControllerError::StaleSequence {
                last_accepted: self.last_accepted,
                received: command.sequence,
            });
        }
        if !command.action.is_valid() {
            return Err(ControllerError::InvalidAction);
        }
        self.last_accepted = command.sequence;
        Ok(command.action)
    }
}

#[derive(Component, Clone, Debug)]
/// Bevy component that owns sequencing for one action domain.
pub struct EventController<Action>
where
    Action: ControllerAction,
{
    sequence: ControllerSequence,
    action: PhantomData<fn() -> Action>,
}

impl<Action> Default for EventController<Action>
where
    Action: ControllerAction,
{
    fn default() -> Self {
        Self {
            sequence: ControllerSequence::default(),
            action: PhantomData,
        }
    }
}

impl<Action> EventController<Action>
where
    Action: ControllerAction,
{
    /// Returns the latest issued or accepted command sequence.
    pub(crate) const fn last_accepted_sequence(&self) -> u64 {
        self.sequence.last_accepted
    }

    /// Validates an action and wraps it in the next command sequence.
    pub fn issue(&mut self, action: Action) -> Result<ControllerCommand<Action>, ControllerError> {
        self.sequence.issue(action)
    }

    /// Accepts a valid command whose sequence is newer than local state.
    pub fn apply_command(
        &mut self,
        command: ControllerCommand<Action>,
    ) -> Result<Action, ControllerError> {
        self.sequence.apply_command(command)
    }

    pub const fn last_sequence(&self) -> u64 {
        self.sequence.last_accepted
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Reasons a controller cannot issue or accept a command.
pub enum ControllerError {
    StaleSequence { last_accepted: u64, received: u64 },
    SequenceExhausted,
    InvalidAction,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct TestAction(bool);

    impl ControllerAction for TestAction {
        fn is_valid(self) -> bool {
            self.0
        }
    }

    #[test]
    fn event_controller_rejects_invalid_and_stale_actions() {
        let mut controller = EventController::<TestAction>::default();
        assert_eq!(
            controller.issue(TestAction(false)),
            Err(ControllerError::InvalidAction)
        );
        let command = controller.issue(TestAction(true)).unwrap();
        assert_eq!(command.sequence, 1);

        let mut receiver = EventController::<TestAction>::default();
        assert!(receiver.apply_command(command).is_ok());
        assert_eq!(
            receiver.apply_command(command),
            Err(ControllerError::StaleSequence {
                last_accepted: 1,
                received: 1,
            })
        );
    }
}
