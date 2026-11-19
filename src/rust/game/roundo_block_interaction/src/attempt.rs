//! Local causal identities. Possession is never registration or edit permission.

use bevy::{
    ecs::{system::SystemParam, world::WorldId},
    prelude::{App, FromWorld, Plugin, ResMut, Resource, World},
};

/// One activation's correlation space, distinct even across independent Apps.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InteractionActivationId {
    world: WorldId,
    generation: u64,
}

/// A checked, non-reusable operation identity; not a Factory Handle or wire ID.
///
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InteractionAttemptId {
    activation: InteractionActivationId,
    sequence: u64,
}

impl InteractionAttemptId {
    pub const fn activation(self) -> InteractionActivationId {
        self.activation
    }

    pub const fn sequence(self) -> u64 {
        self.sequence
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionIdError {
    ActivationExhausted,
    AttemptExhausted,
}

#[derive(Resource)]
pub(crate) struct InteractionSpaces {
    world: WorldId,
    next_activation: Option<u64>,
}

impl FromWorld for InteractionSpaces {
    fn from_world(world: &mut World) -> Self {
        Self {
            world: world.id(),
            next_activation: Some(1),
        }
    }
}

/// Installs local correlation identities only: no Life data, application,
/// controller, behavior or voxel-edit system is registered.
pub struct InteractionIdsPlugin;
impl Plugin for InteractionIdsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InteractionSpaces>();
    }
}

/// Narrow App-local source of fresh correlation spaces, backed by one private
/// Resource installed by the identity Plugin. This does not activate an application.
#[derive(SystemParam)]
pub struct InteractionAttemptIds<'w> {
    spaces: ResMut<'w, InteractionSpaces>,
}

impl InteractionAttemptIds<'_> {
    pub fn begin_activation(&mut self) -> Result<InteractionAttemptAllocator, InteractionIdError> {
        let generation = self
            .spaces
            .next_activation
            .ok_or(InteractionIdError::ActivationExhausted)?;
        self.spaces.next_activation = generation.checked_add(1);
        Ok(InteractionAttemptAllocator {
            activation: InteractionActivationId {
                world: self.spaces.world,
                generation,
            },
            next_sequence: Some(1),
        })
    }
}

/// Single owner of one activation's checked sequence. It cannot be cloned/reset.
/// Publication failure may leave a gap; an allocated ID is never recycled.
pub struct InteractionAttemptAllocator {
    activation: InteractionActivationId,
    next_sequence: Option<u64>,
}

impl InteractionAttemptAllocator {
    pub const fn activation(&self) -> InteractionActivationId {
        self.activation
    }

    pub fn allocate(&mut self) -> Result<InteractionAttemptId, InteractionIdError> {
        let sequence = self
            .next_sequence
            .ok_or(InteractionIdError::AttemptExhausted)?;
        self.next_sequence = sequence.checked_add(1);
        Ok(InteractionAttemptId {
            activation: self.activation,
            sequence,
        })
    }
}
