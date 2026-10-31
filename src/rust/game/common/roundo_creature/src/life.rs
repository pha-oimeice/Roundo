//! Source-role tags and their allocation, not Factory identities or attribute values.

use bevy::prelude::{Component, Resource};

macro_rules! body_id {
    ($name:ident) => {
        #[derive(Component, Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(pub u64);
    };
}

body_id!(IntentBodyId);
body_id!(ExecutionBodyId);
body_id!(RecordBodyId);

/// Allocates independent source-role tags. These are not Life identity, Factory
/// Handles, or capabilities; runtime identity is owned by `roundo_life`.
#[derive(Resource, Default)]
pub struct BodyInstanceIds {
    next_intent: u64,
    next_execution: u64,
    next_record: u64,
}

impl BodyInstanceIds {
    pub fn intent(&mut self) -> IntentBodyId {
        self.next_intent = self
            .next_intent
            .checked_add(1)
            .expect("intent body id exhausted");
        IntentBodyId(self.next_intent)
    }

    pub fn execution(&mut self) -> ExecutionBodyId {
        self.next_execution = self
            .next_execution
            .checked_add(1)
            .expect("execution body id exhausted");
        ExecutionBodyId(self.next_execution)
    }

    pub fn record(&mut self) -> RecordBodyId {
        self.next_record = self
            .next_record
            .checked_add(1)
            .expect("record body id exhausted");
        RecordBodyId(self.next_record)
    }
}
