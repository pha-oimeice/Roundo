use crate::ExecutionBodyId;
use avian3d::prelude::{Collider, RigidBody};
use bevy::prelude::{Bundle, Component, Quat, Transform, Vec3};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreatureMotionError {
    InvalidValue,
    InvalidFriction,
}

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LocomotionState {
    Grounded,
    #[default]
    Floating,
}
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct KinematicMass(pub f32);
#[derive(Component, Clone, Copy, Debug, PartialEq, Default)]
pub struct KinematicVelocity(pub Vec3);
/// Tunable response owned by an Execution Body with grounded locomotion.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct GroundLocomotion {
    pub(crate) max_speed: f32,
    pub(crate) acceleration: f32,
    pub(crate) braking_acceleration: f32,
    pub(crate) jump_speed: f32,
}

impl GroundLocomotion {
    pub fn new(
        max_speed: f32,
        acceleration: f32,
        braking_acceleration: f32,
        jump_speed: f32,
    ) -> Result<Self, CreatureMotionError> {
        if !max_speed.is_finite()
            || max_speed < 0.0
            || !acceleration.is_finite()
            || acceleration < 0.0
            || !braking_acceleration.is_finite()
            || braking_acceleration < 0.0
            || !jump_speed.is_finite()
            || jump_speed < 0.0
        {
            return Err(CreatureMotionError::InvalidValue);
        }
        Ok(Self {
            max_speed,
            acceleration,
            braking_acceleration,
            jump_speed,
        })
    }
}
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct CapsuleDimensions {
    pub radius: f32,
    pub cylindrical_height: f32,
}

/// Maximum resting separation and the base tolerance used by ground snapping.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct ContactTolerance(pub f32);
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct BodyOrientation(pub Quat);
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct GazeOrientation(pub Quat);
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct GroundHorizontalMovement;
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct GroundJump;
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct GazeControl;

/// Source-defined lowest-level movement effect requested by an Intent Body.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct MovementIntent {
    pub local_axis: [f32; 3],
}
impl MovementIntent {
    pub fn new(local_axis: [f32; 3]) -> Result<Self, CreatureMotionError> {
        validate_axis(local_axis)?;
        Ok(Self { local_axis })
    }
}
impl Default for MovementIntent {
    fn default() -> Self {
        Self {
            local_axis: [0.0; 3],
        }
    }
}

/// Source-defined lowest-level gaze effect requested by an Intent Body.
#[derive(Component, Clone, Copy, Debug, PartialEq, Default)]
pub struct GazeIntent {
    pub yaw_delta: f32,
    pub pitch_delta: f32,
}
impl GazeIntent {
    pub fn new(yaw_delta: f32, pitch_delta: f32) -> Result<Self, CreatureMotionError> {
        validate_gaze(yaw_delta, pitch_delta)?;
        Ok(Self {
            yaw_delta,
            pitch_delta,
        })
    }
}

/// Execution-local action value resolved from an Intent Body. It contains no
/// Controller identity, sequencing state, or mutable access to that body.
#[derive(Component, Clone, Copy, Debug, PartialEq, Default)]
pub struct MovementAction {
    pub local_axis: [f32; 3],
}
impl MovementAction {
    pub fn new(local_axis: [f32; 3]) -> Result<Self, CreatureMotionError> {
        validate_axis(local_axis)?;
        Ok(Self { local_axis })
    }
}
impl From<MovementIntent> for MovementAction {
    fn from(value: MovementIntent) -> Self {
        Self {
            local_axis: value.local_axis,
        }
    }
}

#[derive(Component, Clone, Copy, Debug, PartialEq, Default)]
pub struct GazeAction {
    pub yaw_delta: f32,
    pub pitch_delta: f32,
}
impl GazeAction {
    pub fn new(yaw_delta: f32, pitch_delta: f32) -> Result<Self, CreatureMotionError> {
        validate_gaze(yaw_delta, pitch_delta)?;
        Ok(Self {
            yaw_delta,
            pitch_delta,
        })
    }
}
impl From<GazeIntent> for GazeAction {
    fn from(value: GazeIntent) -> Self {
        Self {
            yaw_delta: value.yaw_delta,
            pitch_delta: value.pitch_delta,
        }
    }
}

fn validate_axis(local_axis: [f32; 3]) -> Result<(), CreatureMotionError> {
    let axis = Vec3::from_array(local_axis);
    if !axis.is_finite() || axis.length_squared() > 1.0 + f32::EPSILON {
        Err(CreatureMotionError::InvalidValue)
    } else {
        Ok(())
    }
}

fn validate_gaze(yaw_delta: f32, pitch_delta: f32) -> Result<(), CreatureMotionError> {
    if !yaw_delta.is_finite() || !pitch_delta.is_finite() {
        Err(CreatureMotionError::InvalidValue)
    } else {
        Ok(())
    }
}

/// Complete, immutable local environment for one logic tick.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct EnvironmentSample {
    pub gravity: Vec3,
    pub static_friction: f32,
    pub kinetic_friction: f32,
    pub revision: u64,
}
impl EnvironmentSample {
    pub fn new(
        gravity: Vec3,
        static_friction: f32,
        kinetic_friction: f32,
        revision: u64,
    ) -> Result<Self, CreatureMotionError> {
        if !gravity.is_finite()
            || !static_friction.is_finite()
            || !kinetic_friction.is_finite()
            || static_friction < 0.0
            || kinetic_friction < 0.0
        {
            return Err(CreatureMotionError::InvalidValue);
        }
        if kinetic_friction > static_friction {
            return Err(CreatureMotionError::InvalidFriction);
        }
        Ok(Self {
            gravity,
            static_friction,
            kinetic_friction,
            revision,
        })
    }
}
impl Default for EnvironmentSample {
    fn default() -> Self {
        Self::new(Vec3::new(0.0, -9.81, 0.0), 0.6, 0.5, 0).unwrap()
    }
}

impl CapsuleDimensions {
    pub fn new(radius: f32, cylindrical_height: f32) -> Result<Self, CreatureMotionError> {
        if !radius.is_finite()
            || !cylindrical_height.is_finite()
            || radius <= 0.0
            || cylindrical_height < 0.0
        {
            Err(CreatureMotionError::InvalidValue)
        } else {
            Ok(Self {
                radius,
                cylindrical_height,
            })
        }
    }
}

#[derive(Bundle)]
pub struct ExecutionMotionBundle {
    pub locomotion: LocomotionState,
    pub mass: KinematicMass,
    pub velocity: KinematicVelocity,
    pub ground_locomotion: GroundLocomotion,
    pub capsule: CapsuleDimensions,
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
        capsule: CapsuleDimensions,
        ground_locomotion: GroundLocomotion,
        contact_tolerance: f32,
    ) -> Result<Self, CreatureMotionError> {
        if !mass.is_finite()
            || mass <= 0.0
            || !contact_tolerance.is_finite()
            || contact_tolerance < 0.0
        {
            return Err(CreatureMotionError::InvalidValue);
        }
        Ok(Self {
            locomotion: LocomotionState::Floating,
            mass: KinematicMass(mass),
            velocity: KinematicVelocity::default(),
            ground_locomotion,
            capsule,
            contact_tolerance: ContactTolerance(contact_tolerance),
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

/// Named prefab data for the current test Execution Body. Materialization copies
/// all data into the instance; no prototype identity is retained.
#[derive(Clone, Copy, Debug)]
pub struct TestExecutionBodyPrototype {
    pub mass: f32,
    pub capsule: CapsuleDimensions,
    pub locomotion: GroundLocomotion,
    pub contact_tolerance: f32,
}

impl Default for TestExecutionBodyPrototype {
    fn default() -> Self {
        Self {
            mass: 1.0,
            capsule: CapsuleDimensions::new(0.4, 1.0).unwrap(),
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
                self.capsule,
                self.locomotion,
                self.contact_tolerance,
            )
            .expect("test Execution Body prototype is valid"),
            transform: Transform::from_translation(translation),
            movement: MovementAction::default(),
            gaze: GazeAction::default(),
            environment,
            rigid_body: RigidBody::Kinematic,
            collider: Collider::capsule(self.capsule.radius, self.capsule.cylindrical_height),
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
