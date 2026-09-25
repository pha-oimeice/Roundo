//! Three-body Life domain and fixed-tick Execution Body motion.
//!
//! A Life is an exclusive relationship among independent Intent, Execution,
//! and Record Body entities. Controllers remain external adapters. Intent,
//! Action, and Record primitives cross body boundaries only as owned values.

mod collision;
mod formula;
mod life;
mod plugin;
#[cfg(test)]
mod plugin_tests;
mod types;

pub use life::{
    BodyInstanceIds, ExecutionBodyId, IntentBodyBundle, IntentBodyId, LifeCompositionError,
    LifeEntities, LifeId, LifeLink, LifeRegistry, RecordBodyBundle, RecordBodyId,
    TestIntentBodyPrototype, TestLifePrototype, TestRecordBodyPrototype, compose_life,
    dissolve_life, validate_life,
};
pub use plugin::{CreatureMotionPlugin, CreatureMotionSet, commit_creature_motion};
pub use types::{
    BodyOrientation, CapsuleDimensions, ContactTolerance, CreatureMotionError, EnvironmentSample,
    ExecutionBodyBundle, ExecutionMotionBundle, ExecutionRuntimeBundle, GazeAction, GazeControl,
    GazeIntent, GazeOrientation, GroundHorizontalMovement, GroundJump, GroundLocomotion,
    KinematicMass, KinematicVelocity, LocomotionState, MovementAction, MovementIntent,
    TestExecutionBodyPrototype,
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validated_constructors_reject_invalid_physical_values() {
        assert!(CapsuleDimensions::new(0.0, 1.0).is_err());
        assert!(
            ExecutionMotionBundle::new(
                0.0,
                CapsuleDimensions::new(0.4, 1.0).unwrap(),
                GroundLocomotion::new(5.0, 30.0, 40.0, 6.0).unwrap(),
                0.01,
            )
            .is_err()
        );
        assert!(MovementIntent::new([2.0, 0.0, 0.0]).is_err());
        assert!(GroundLocomotion::new(5.0, -1.0, 40.0, 6.0).is_err());
        assert!(EnvironmentSample::new(bevy::prelude::Vec3::ZERO, 0.2, 0.3, 0).is_err());
    }

    #[test]
    fn execution_bundle_separates_gaze_and_body_state() {
        let bundle = ExecutionMotionBundle::new(
            1.0,
            CapsuleDimensions::new(0.4, 1.0).unwrap(),
            GroundLocomotion::new(5.0, 30.0, 40.0, 6.0).unwrap(),
            0.01,
        )
        .unwrap();
        assert_eq!(bundle.body_orientation.0, bevy::prelude::Quat::IDENTITY);
        assert_eq!(bundle.gaze_orientation.0, bevy::prelude::Quat::IDENTITY);
    }

    #[test]
    fn test_prototype_materializes_complete_data_without_prototype_identity() {
        let runtime = TestExecutionBodyPrototype::default().materialize_runtime(
            bevy::prelude::Vec3::new(1.0, 2.0, 3.0),
            EnvironmentSample::default(),
        );

        assert_eq!(
            runtime.transform.translation,
            bevy::prelude::Vec3::new(1.0, 2.0, 3.0)
        );
        assert_eq!(runtime.motion.mass.0, 1.0);
        assert_eq!(runtime.motion.capsule.radius, 0.4);
        assert_eq!(runtime.motion.capsule.cylindrical_height, 1.0);
    }
}
