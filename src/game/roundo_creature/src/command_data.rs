use crate::{CreatureMotionError, GazeAction, LocomotionState, MovementAction};
use bevy::prelude::{Component, Quat, Vec3};
use roundo_life::{AuthoritativeFixedTick, EventData, EventShape, LifeData, LifeDataTypeKey};

/// Held movement input published by an Intent Body through its Life State slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovementCommandState {
    local_axis: [f32; 3],
}

impl MovementCommandState {
    pub fn new(local_axis: [f32; 3]) -> Result<Self, CreatureMotionError> {
        let axis = Vec3::from_array(local_axis);
        if !axis.is_finite() || axis.length() > 1.0 {
            return Err(CreatureMotionError::InvalidValue);
        }
        Ok(Self { local_axis })
    }

    pub const fn local_axis(&self) -> &[f32; 3] {
        &self.local_axis
    }
}

impl Default for MovementCommandState {
    fn default() -> Self {
        Self {
            local_axis: [0.0; 3],
        }
    }
}

roundo_life::declare_state_data!(MovementCommandState, "roundo.creature.movement-command.v1");

impl From<MovementCommandState> for MovementAction {
    fn from(value: MovementCommandState) -> Self {
        Self {
            local_axis: value.local_axis,
        }
    }
}

/// One-tick accumulated gaze input published through an Intent Event interface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GazeCommandEvent {
    yaw_delta: f32,
    pitch_delta: f32,
}

impl GazeCommandEvent {
    pub fn new(yaw_delta: f32, pitch_delta: f32) -> Result<Self, CreatureMotionError> {
        if !yaw_delta.is_finite() || !pitch_delta.is_finite() {
            return Err(CreatureMotionError::InvalidValue);
        }
        Ok(Self {
            yaw_delta,
            pitch_delta,
        })
    }

    pub const fn yaw_delta(&self) -> f32 {
        self.yaw_delta
    }

    pub const fn pitch_delta(&self) -> f32 {
        self.pitch_delta
    }
}

impl roundo_life::__private::Sealed for GazeCommandEvent {}
impl LifeData for GazeCommandEvent {
    type Shape = EventShape;
    const KEY: LifeDataTypeKey = LifeDataTypeKey::from_source("roundo.creature.gaze-command.v1");
}
impl EventData for GazeCommandEvent {
    fn retained_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn validate(&self) -> bool {
        self.yaw_delta.is_finite() && self.pitch_delta.is_finite()
    }
}

impl From<GazeCommandEvent> for GazeAction {
    fn from(value: GazeCommandEvent) -> Self {
        Self {
            yaw_delta: value.yaw_delta,
            pitch_delta: value.pitch_delta,
        }
    }
}

/// Coherent authoritative pose and motion state committed by an Execution Body.
/// Translation is the AABB's logical bottom-center anchor, not a collision or mass center.
/// Record and Observation copies preserve that coordinate without applying offsets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CreatureMotionExperienceState {
    committed_tick: AuthoritativeFixedTick,
    translation: [f32; 3],
    body_orientation: [f32; 4],
    gaze_orientation: [f32; 4],
    linear_velocity: [f32; 3],
    locomotion: LocomotionState,
}

impl CreatureMotionExperienceState {
    pub fn new(
        committed_tick: AuthoritativeFixedTick,
        translation: [f32; 3],
        body_orientation: [f32; 4],
        gaze_orientation: [f32; 4],
        linear_velocity: [f32; 3],
        locomotion: LocomotionState,
    ) -> Result<Self, CreatureMotionError> {
        validate_motion_values(
            translation,
            body_orientation,
            gaze_orientation,
            linear_velocity,
        )?;
        Ok(Self {
            committed_tick,
            translation,
            body_orientation,
            gaze_orientation,
            linear_velocity,
            locomotion,
        })
    }

    pub const fn committed_tick(&self) -> AuthoritativeFixedTick {
        self.committed_tick
    }

    pub const fn translation(&self) -> &[f32; 3] {
        &self.translation
    }

    pub const fn body_orientation(&self) -> &[f32; 4] {
        &self.body_orientation
    }

    pub const fn gaze_orientation(&self) -> &[f32; 4] {
        &self.gaze_orientation
    }

    pub const fn linear_velocity(&self) -> &[f32; 3] {
        &self.linear_velocity
    }

    pub const fn locomotion(&self) -> LocomotionState {
        self.locomotion
    }
}

roundo_life::declare_state_data!(
    CreatureMotionExperienceState,
    "roundo.creature.motion-experience.v1"
);

/// Caller-owned baseline copied from validated Experience; it is not a Life State.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct CreatureMotionRecord {
    committed_tick: AuthoritativeFixedTick,
    translation: [f32; 3],
    body_orientation: [f32; 4],
    gaze_orientation: [f32; 4],
    linear_velocity: [f32; 3],
    locomotion: LocomotionState,
}

impl CreatureMotionRecord {
    pub fn new(
        committed_tick: AuthoritativeFixedTick,
        translation: [f32; 3],
        body_orientation: [f32; 4],
        gaze_orientation: [f32; 4],
        linear_velocity: [f32; 3],
        locomotion: LocomotionState,
    ) -> Result<Self, CreatureMotionError> {
        validate_motion_values(
            translation,
            body_orientation,
            gaze_orientation,
            linear_velocity,
        )?;
        Ok(Self {
            committed_tick,
            translation,
            body_orientation,
            gaze_orientation,
            linear_velocity,
            locomotion,
        })
    }

    pub fn from_experience(value: CreatureMotionExperienceState) -> Self {
        Self {
            committed_tick: value.committed_tick,
            translation: value.translation,
            body_orientation: value.body_orientation,
            gaze_orientation: value.gaze_orientation,
            linear_velocity: value.linear_velocity,
            locomotion: value.locomotion,
        }
    }

    pub const fn committed_tick(&self) -> AuthoritativeFixedTick {
        self.committed_tick
    }

    pub const fn translation(&self) -> &[f32; 3] {
        &self.translation
    }

    pub const fn body_orientation(&self) -> &[f32; 4] {
        &self.body_orientation
    }

    pub const fn gaze_orientation(&self) -> &[f32; 4] {
        &self.gaze_orientation
    }

    pub const fn linear_velocity(&self) -> &[f32; 3] {
        &self.linear_velocity
    }

    pub const fn locomotion(&self) -> LocomotionState {
        self.locomotion
    }
}

/// Owned domain projection of a motion Record; contains no network envelope.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CreatureObservation {
    observed_tick: AuthoritativeFixedTick,
    translation: [f32; 3],
    body_orientation: [f32; 4],
    gaze_orientation: [f32; 4],
    linear_velocity: [f32; 3],
    locomotion: LocomotionState,
}

impl CreatureObservation {
    pub fn from_record(record: &CreatureMotionRecord) -> Self {
        Self {
            observed_tick: record.committed_tick,
            translation: record.translation,
            body_orientation: record.body_orientation,
            gaze_orientation: record.gaze_orientation,
            linear_velocity: record.linear_velocity,
            locomotion: record.locomotion,
        }
    }

    pub const fn observed_tick(&self) -> AuthoritativeFixedTick {
        self.observed_tick
    }

    pub const fn translation(&self) -> &[f32; 3] {
        &self.translation
    }

    pub const fn body_orientation(&self) -> &[f32; 4] {
        &self.body_orientation
    }

    pub const fn gaze_orientation(&self) -> &[f32; 4] {
        &self.gaze_orientation
    }

    pub const fn linear_velocity(&self) -> &[f32; 3] {
        &self.linear_velocity
    }

    pub const fn locomotion(&self) -> LocomotionState {
        self.locomotion
    }
}

fn validate_motion_values(
    translation: [f32; 3],
    body_orientation: [f32; 4],
    gaze_orientation: [f32; 4],
    linear_velocity: [f32; 3],
) -> Result<(), CreatureMotionError> {
    let body = Quat::from_array(body_orientation);
    let gaze = Quat::from_array(gaze_orientation);
    if !translation.into_iter().all(f32::is_finite)
        || !linear_velocity.into_iter().all(f32::is_finite)
        || !body_orientation.into_iter().all(f32::is_finite)
        || !gaze_orientation.into_iter().all(f32::is_finite)
        || !body.is_normalized()
        || !gaze.is_normalized()
    {
        return Err(CreatureMotionError::InvalidValue);
    }
    Ok(())
}

/// Registers Creature's concrete Life schemas, without creating any instances.
pub struct CreatureLifeDataPlugin;

impl bevy::prelude::Plugin for CreatureLifeDataPlugin {
    fn build(&self, app: &mut bevy::prelude::App) {
        roundo_life::register_state_type::<crate::GazeCommandState>(app)
            .expect("Creature Life schema startup registration failed: GazeCommandState");
        roundo_life::register_state_type::<MovementCommandState>(app)
            .expect("Creature Life schema startup registration failed: MovementCommandState");
        roundo_life::register_event_type::<GazeCommandEvent>(app)
            .expect("Creature Life schema startup registration failed: GazeCommandEvent");
        roundo_life::register_event_type::<crate::JumpCommandEvent>(app)
            .expect("Creature Life schema startup registration failed: JumpCommandEvent");
        roundo_life::register_state_type::<CreatureMotionExperienceState>(app).expect(
            "Creature Life schema startup registration failed: CreatureMotionExperienceState",
        );
    }
}
