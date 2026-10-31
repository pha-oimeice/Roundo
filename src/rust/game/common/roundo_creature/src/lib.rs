//! Creature Body roles, typed motion data, and fixed-tick Execution Body motion.
//!
//! Intent, Execution, and Record Bodies are independently created; their source
//! role tags are not Life identity. Runtime Life and Body Handles are owned by
//! `roundo_life`; Controllers remain external adapters, and typed values cross
//! body boundaries only through the approved routing path.

mod actions;
pub mod attributes;
mod bundles;
mod collision;
mod command_data;
mod discrete_command;
mod environment;
mod execution_policy;
pub use execution_policy::{FactoryMotionPolicy, FactoryMotionReadiness, JumpInputMode};
mod formula;
mod gaze_state;
pub use gaze_state::GazeCommandState;
mod life;
mod plugin;
mod prototypes;
mod viewpoint;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreatureMotionError {
    InvalidValue,
    InvalidFriction,
}

pub use viewpoint::CreatureViewpoint;

pub use actions::{GazeAction, MovementAction};
pub use attributes::{
    AabbDimensions, BodyOrientation, ContactTolerance, GazeControl, GazeOrientation,
    GroundHorizontalMovement, GroundJump, GroundLocomotion, KinematicMass, KinematicVelocity,
    LocomotionState,
};
pub use bundles::{
    ExecutionBodyBundle, ExecutionMotionBundle, ExecutionRuntimeBundle, IntentBodyBundle,
    RecordBodyBundle,
};
pub use command_data::{
    CreatureLifeDataPlugin, CreatureMotionExperienceState, CreatureMotionRecord,
    CreatureObservation, GazeCommandEvent, MovementCommandState,
};
pub use discrete_command::{JumpCommandEvent, OneTickCommandBudget, horizontal_movement_command};
pub use environment::EnvironmentSample;
pub use life::{BodyInstanceIds, ExecutionBodyId, IntentBodyId, RecordBodyId};
pub use plugin::{
    CommittedFactoryMotion, CreatureLifeSet, CreatureMotionPlugin, CreatureMotionSet,
    ExecutedFactoryMotion, FactoryMotionExecutions, FactoryMotionLifecycle, FactoryMotionReceipts,
    FactoryMotionRoute, FactoryMotionRouteValidator, FactoryMotionRouteValidatorInstallError,
    FactoryMotionRoutes, commit_creature_motion, committed_factory_motion, executed_factory_motion,
    install_factory_motion_route_validator, is_live_factory_motion_route,
};
pub use prototypes::{
    TestExecutionBodyPrototype, TestIntentBodyPrototype, TestLifePrototype, TestRecordBodyPrototype,
};
