//! Native initial-value recipes; materialization does not retain a live binding.

use crate::{
    EnvironmentSample, ExecutionBodyBundle, ExecutionBodyId, ExecutionMotionBundle,
    ExecutionRuntimeBundle, GazeAction, IntentBodyBundle, IntentBodyId, MovementAction,
    RecordBodyBundle, RecordBodyId,
    attributes::{AabbDimensions, GroundLocomotion, RigidBody, Transform},
};
use bevy::prelude::Vec3;

#[derive(Clone, Copy, Debug, Default)]
pub struct TestIntentBodyPrototype;
impl TestIntentBodyPrototype {
    pub fn materialize(self, id: IntentBodyId) -> IntentBodyBundle {
        IntentBodyBundle::materialize(id)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TestRecordBodyPrototype;
impl TestRecordBodyPrototype {
    pub fn materialize(self, id: RecordBodyId) -> RecordBodyBundle {
        RecordBodyBundle::materialize(id)
    }
}

/// A test creation recipe retaining independent per-Body prototype choices.
/// It does not compose a tuple identity or install cross-Body relationships.
#[derive(Clone, Copy, Debug, Default)]
pub struct TestLifePrototype {
    pub intent: TestIntentBodyPrototype,
    pub execution: TestExecutionBodyPrototype,
    pub record: TestRecordBodyPrototype,
}

/// Named prefab data for the current test Execution Body. Materialization copies
/// all data into the instance; no prototype identity is retained.
#[derive(Clone, Copy, Debug)]
pub struct TestExecutionBodyPrototype {
    pub mass: f32,
    pub aabb: AabbDimensions,
    pub locomotion: GroundLocomotion,
    pub contact_tolerance: f32,
}

impl Default for TestExecutionBodyPrototype {
    fn default() -> Self {
        Self {
            mass: 1.0,
            aabb: AabbDimensions::new(Vec3::new(0.4, 0.9, 0.4)).unwrap(),
            locomotion: GroundLocomotion::new(5.0, 30.0, 40.0, 6.0).unwrap(),
            contact_tolerance: 0.01,
        }
    }
}

impl TestExecutionBodyPrototype {
    pub fn materialize_runtime(
        self,
        translation: Vec3,
        environment: EnvironmentSample,
    ) -> ExecutionRuntimeBundle {
        ExecutionRuntimeBundle {
            motion: ExecutionMotionBundle::new(
                self.mass,
                self.aabb,
                self.locomotion,
                self.contact_tolerance,
            )
            .expect("test Execution Body prototype is valid"),
            transform: Transform::from_translation(translation),
            movement: MovementAction::default(),
            gaze: GazeAction::default(),
            environment,
            rigid_body: RigidBody::Kinematic,
            // Offset geometry, not the logical pose; Avian derives the physical
            // center of mass from the same bottom-anchored AABB used by movement.
            collider: self.aabb.collider(),
        }
    }

    pub fn materialize(
        self,
        id: ExecutionBodyId,
        translation: Vec3,
        environment: EnvironmentSample,
    ) -> ExecutionBodyBundle {
        ExecutionBodyBundle {
            id,
            runtime: self.materialize_runtime(translation, environment),
        }
    }
}
