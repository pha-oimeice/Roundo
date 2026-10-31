//! Concrete Body assembly; a Bundle is a recipe, not an attribute type.

use crate::{
    CreatureMotionError, EnvironmentSample, ExecutionBodyId, GazeAction, IntentBodyId,
    MovementAction, RecordBodyId,
    attributes::{
        AabbDimensions, BodyOrientation, Collider, ContactTolerance, GazeControl, GazeOrientation,
        GroundHorizontalMovement, GroundJump, GroundLocomotion, KinematicMass, KinematicVelocity,
        LocomotionState, RigidBody, Transform,
    },
};
use bevy::prelude::{Bundle, Quat};

/// Intent Body source-role tag. The runtime Life identity and routing truth are
/// the independent generational Handles owned by `roundo_life`.
#[derive(Bundle)]
pub struct IntentBodyBundle {
    pub id: IntentBodyId,
}

impl IntentBodyBundle {
    pub fn materialize(id: IntentBodyId) -> Self {
        Self { id }
    }
}

#[derive(Bundle)]
pub struct RecordBodyBundle {
    pub id: RecordBodyId,
}

impl RecordBodyBundle {
    pub fn materialize(id: RecordBodyId) -> Self {
        Self { id }
    }
}

/// One existing motion recipe's native attribute selection. It is not a
/// requirement imposed on all Concept Bodies by the attribute model.
#[derive(Bundle)]
pub struct ExecutionMotionBundle {
    pub locomotion: LocomotionState,
    pub mass: KinematicMass,
    pub velocity: KinematicVelocity,
    pub ground_locomotion: GroundLocomotion,
    pub aabb: AabbDimensions,
    pub contact_tolerance: ContactTolerance,
    pub body_orientation: BodyOrientation,
    pub gaze_orientation: GazeOrientation,
    pub ground_movement: GroundHorizontalMovement,
    pub ground_jump: GroundJump,
    pub gaze_control: GazeControl,
}
impl ExecutionMotionBundle {
    pub fn new(
        mass: f32,
        aabb: AabbDimensions,
        ground_locomotion: GroundLocomotion,
        contact_tolerance: f32,
    ) -> Result<Self, CreatureMotionError> {
        let mass = KinematicMass::new(mass)?;
        let contact_tolerance = ContactTolerance::new(contact_tolerance)?;
        Ok(Self {
            locomotion: LocomotionState::Floating,
            mass,
            velocity: KinematicVelocity::default(),
            ground_locomotion,
            aabb,
            contact_tolerance,
            body_orientation: BodyOrientation(Quat::IDENTITY),
            gaze_orientation: GazeOrientation(Quat::IDENTITY),
            ground_movement: GroundHorizontalMovement,
            ground_jump: GroundJump,
            gaze_control: GazeControl,
        })
    }
}

/// Runtime data of an Execution Body, also reusable by a non-authoritative
/// prediction projection that is deliberately not a Life instance.
/// `transform.translation` is the logical bottom-center position, not the physical
/// AABB center. Rotation is identity and scale is one; BodyOrientation remains a
/// separate locomotion frame and never rotates the movement collision geometry.
#[derive(Bundle)]
pub struct ExecutionRuntimeBundle {
    pub motion: ExecutionMotionBundle,
    pub transform: Transform,
    pub movement: MovementAction,
    pub gaze: GazeAction,
    pub environment: EnvironmentSample,
    pub rigid_body: RigidBody,
    pub collider: Collider,
}

#[derive(Bundle)]
pub struct ExecutionBodyBundle {
    pub id: ExecutionBodyId,
    pub runtime: ExecutionRuntimeBundle,
}
