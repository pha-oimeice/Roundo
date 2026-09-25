//! Deep client Creature prediction and authoritative reconciliation module.
use crate::creature_snapshot::restore_creature_snapshot;
use bevy::prelude::{
    App, Component, Entity, Fixed, FixedUpdate, IntoScheduleConfigs, MessageReader, Plugin, Query,
    Res, ResMut, Resource, Time, Transform, Update, Vec3, With, Without, World,
};
use roundo_creature::{
    CreatureMotionPlugin, CreatureMotionSet, EnvironmentSample, GazeAction, MovementAction,
    TestExecutionBodyPrototype, commit_creature_motion,
};
use roundo_local_coordinate::LocalCoordinateClientWorld;
use roundo_marionette::{
    ClientPlayerController, CreatureAuthorityUpdate, LocallyRoutedControllerIntent,
    MarionetteClientSet,
};
use std::{collections::VecDeque, time::Duration};

#[derive(Component)]
struct PredictedCreature;
#[derive(Resource, Default)]
struct PredictionRuntime {
    session_epoch: Option<u64>,
    entity: Option<Entity>,
    applied_tick: Option<u64>,
    predicted_tick: Option<u64>,
}

#[derive(Resource, Default)]
struct PendingAuthoritySnapshot {
    session_epoch: Option<u64>,
    snapshot: Option<roundo_contracts::CreatureMotionSnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PredictionInputJournalEntry {
    tick: u64,
    movement: Option<(u64, [f32; 3])>,
    gaze: Option<(u64, f32, f32)>,
}

/// Owns retained semantic intent, effective tick input, and replay history.
#[derive(Resource, Clone, Debug, Default)]
struct PredictionInputs {
    movement: VecDeque<(u64, [f32; 3])>,
    gaze: VecDeque<(u64, f32, f32)>,
    journal: VecDeque<PredictionInputJournalEntry>,
    effective_movement: Option<(u64, [f32; 3])>,
    movement_cursor: u64,
    gaze_cursor: u64,
}

impl PredictionInputs {
    fn record(&mut self, command: roundo_contracts::PlayerControllerCommand) {
        match command {
            roundo_contracts::PlayerControllerCommand::Movement3D(command) => {
                self.record_movement(command.sequence, command.action.direction);
            }
            roundo_contracts::PlayerControllerCommand::Gaze(command) => {
                self.record_gaze(
                    command.sequence,
                    command.action.yaw_delta,
                    command.action.pitch_delta,
                );
            }
            roundo_contracts::PlayerControllerCommand::DestroyBlock(_)
            | roundo_contracts::PlayerControllerCommand::PlaceBlock(_)
            | roundo_contracts::PlayerControllerCommand::SpawnTestCreature(_) => {}
        }
    }

    fn record_movement(&mut self, sequence: u64, axis: [f32; 3]) {
        self.movement.push_back((sequence, axis));
    }

    fn record_gaze(&mut self, sequence: u64, yaw_delta: f32, pitch_delta: f32) {
        self.gaze.push_back((sequence, yaw_delta, pitch_delta));
    }

    fn append_tick(&mut self, tick: u64) -> PredictionInputJournalEntry {
        if let Some(change) = self
            .movement
            .iter()
            .filter(|(sequence, _)| *sequence > self.movement_cursor)
            .copied()
            .last()
        {
            self.effective_movement = Some(change);
            self.movement_cursor = change.0;
        }
        let gaze_cursor = self.gaze_cursor;
        let mut gaze = None;
        for (sequence, yaw, pitch) in self
            .gaze
            .iter()
            .filter(|(sequence, _, _)| *sequence > gaze_cursor)
            .copied()
        {
            let (_, accumulated_yaw, accumulated_pitch) = gaze.unwrap_or((sequence, 0.0, 0.0));
            gaze = Some((sequence, accumulated_yaw + yaw, accumulated_pitch + pitch));
            self.gaze_cursor = sequence;
        }
        let entry = PredictionInputJournalEntry {
            tick,
            movement: self.effective_movement,
            gaze,
        };
        self.journal.push_back(entry);
        entry
    }

    fn acknowledge(
        &mut self,
        tick: u64,
        movement_sequence: u64,
        gaze_sequence: u64,
    ) -> Vec<PredictionInputJournalEntry> {
        self.movement
            .retain(|(sequence, _)| *sequence > movement_sequence);
        self.gaze
            .retain(|(sequence, _, _)| *sequence > gaze_sequence);
        self.journal.retain(|entry| entry.tick > tick);
        self.journal.iter().copied().collect()
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    #[cfg(test)]
    fn journal_len(&self) -> usize {
        self.journal.len()
    }

    #[cfg(test)]
    fn journal_front(&self) -> Option<PredictionInputJournalEntry> {
        self.journal.front().copied()
    }

    #[cfg(test)]
    fn pending_movement_len(&self) -> usize {
        self.movement.len()
    }
}

#[derive(Resource, Default)]
struct CameraTranslationProjection {
    initialized: bool,
    displayed: Vec3,
}

const CAMERA_TRANSLATION_RESPONSE: f32 = 20.0;
const CAMERA_TRANSLATION_SNAP_DISTANCE: f32 = 2.0;
/// Owns prediction, reconciliation, replay, and camera projection behind one plugin interface.
pub(crate) struct CreaturePredictionPlugin;
impl Plugin for CreaturePredictionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PredictionRuntime>()
            .init_resource::<PendingAuthoritySnapshot>()
            .init_resource::<PredictionInputs>()
            .init_resource::<CameraTranslationProjection>()
            .add_message::<CreatureAuthorityUpdate>()
            .add_message::<LocallyRoutedControllerIntent>()
            .add_plugins(CreatureMotionPlugin)
            .add_systems(
                FixedUpdate,
                (apply_prediction_environment, apply_prediction_journal_tick)
                    .chain()
                    .before(CreatureMotionSet::Commit),
            )
            .add_systems(
                Update,
                (
                    receive_authority_updates,
                    restore_authoritative_creature,
                    project_predicted_camera,
                )
                    .chain()
                    .after(MarionetteClientSet::Commands),
            )
            .add_systems(
                Update,
                collect_routed_controller_intents.after(MarionetteClientSet::Input),
            );
    }
}
fn receive_authority_updates(
    mut updates: MessageReader<CreatureAuthorityUpdate>,
    mut authority: ResMut<PendingAuthoritySnapshot>,
) {
    for update in updates.read() {
        match update {
            CreatureAuthorityUpdate::SessionStarted { epoch } => {
                authority.session_epoch = Some(*epoch);
                authority.snapshot = None;
            }
            CreatureAuthorityUpdate::Snapshot { epoch, snapshot }
                if authority.session_epoch == Some(*epoch) =>
            {
                authority.snapshot = Some(*snapshot);
            }
            CreatureAuthorityUpdate::Snapshot { .. } => {}
            CreatureAuthorityUpdate::SessionEnded { epoch }
                if authority.session_epoch == Some(*epoch) =>
            {
                authority.session_epoch = None;
                authority.snapshot = None;
            }
            CreatureAuthorityUpdate::SessionEnded { .. } => {}
        }
    }
}

fn collect_routed_controller_intents(
    mut routed: MessageReader<LocallyRoutedControllerIntent>,
    mut inputs: ResMut<PredictionInputs>,
) {
    for intent in routed.read() {
        inputs.record(intent.0);
    }
}

/// Samples Stream1 overrides at the predicted capsule center for every logic tick.
/// The Stream0 snapshot remains the authority baseline, while this lookup supplies
/// the next-cell environment used by local replay and prediction.
fn apply_prediction_environment(
    world: Option<Res<LocalCoordinateClientWorld>>,
    mut creatures: Query<(&Transform, &mut EnvironmentSample), With<PredictedCreature>>,
) {
    let Some(world) = world else { return };
    for (transform, mut sample) in &mut creatures {
        if let Some((gravity, static_friction, kinetic_friction, revision)) =
            world.environment_at(transform.translation)
        {
            if let Ok(next) =
                EnvironmentSample::new(gravity, static_friction, kinetic_friction, revision)
            {
                *sample = next;
            }
        }
    }
}

/// Creates one entry for every normal fixed tick; replay consumes the stored entry.
fn apply_prediction_journal_tick(
    mut runtime: ResMut<PredictionRuntime>,
    mut inputs: ResMut<PredictionInputs>,
    mut executions: Query<(&mut MovementAction, &mut GazeAction), With<PredictedCreature>>,
) {
    let Some(entity) = runtime.entity else { return };
    let entry = inputs.append_tick(runtime.predicted_tick.unwrap_or(0).wrapping_add(1));
    if let Ok((mut movement, mut gaze)) = executions.get_mut(entity) {
        apply_prediction_entry(&mut movement, &mut gaze, entry);
        runtime.predicted_tick = Some(entry.tick);
    }
}

fn apply_prediction_entry(
    movement: &mut MovementAction,
    gaze: &mut GazeAction,
    entry: PredictionInputJournalEntry,
) {
    *movement = entry
        .movement
        .map(|(_, axis)| MovementAction::new(axis).expect("validated input"))
        .unwrap_or_default();
    *gaze = entry
        .gaze
        .map(|(_, yaw, pitch)| GazeAction::new(yaw, pitch).expect("validated input"))
        .unwrap_or_default();
}
/// Applies a newer authority baseline and immediately executes exactly one shared fixed
/// schedule per retained tick. No render system advances physics.
fn restore_authoritative_creature(world: &mut World) {
    let authority_epoch = world.resource::<PendingAuthoritySnapshot>().session_epoch;
    if world.resource::<PredictionRuntime>().session_epoch != authority_epoch {
        reset_prediction_session(world, authority_epoch);
    }
    let snapshot = world.resource::<PendingAuthoritySnapshot>().snapshot;
    let Some(snapshot) = snapshot else { return };
    if world
        .resource::<PredictionRuntime>()
        .applied_tick
        .is_some_and(|t| t >= snapshot.simulation_tick.0)
    {
        return;
    }
    if snapshot.fixed_dt_seconds.is_finite() && snapshot.fixed_dt_seconds > 0. {
        *world.resource_mut::<Time<Fixed>>() =
            Time::<Fixed>::from_seconds(snapshot.fixed_dt_seconds.into());
    }
    let replay_entries = world.resource_mut::<PredictionInputs>().acknowledge(
        snapshot.simulation_tick.0,
        snapshot.acknowledged_movement.0,
        snapshot.acknowledged_gaze.0,
    );
    let entity = match world.resource::<PredictionRuntime>().entity {
        Some(e) => e,
        None => {
            let e = world
                .spawn((
                    PredictedCreature,
                    TestExecutionBodyPrototype::default()
                        .materialize_runtime(Vec3::ZERO, EnvironmentSample::default()),
                ))
                .id();
            world.resource_mut::<PredictionRuntime>().entity = Some(e);
            e
        }
    };
    restore_creature_snapshot(world, entity, snapshot);
    {
        let mut r = world.resource_mut::<PredictionRuntime>();
        r.applied_tick = Some(snapshot.simulation_tick.0);
        r.predicted_tick = Some(snapshot.simulation_tick.0);
    }
    for entry in replay_entries {
        {
            let mut entity = world.entity_mut(entity);
            let mut movement = entity
                .get_mut::<MovementAction>()
                .expect("predicted execution projection has MovementAction");
            *movement = entry
                .movement
                .map(|(_, axis)| MovementAction::new(axis).expect("validated input"))
                .unwrap_or_default();
        }
        {
            let mut entity = world.entity_mut(entity);
            let mut gaze = entity
                .get_mut::<GazeAction>()
                .expect("predicted execution projection has GazeAction");
            *gaze = entry
                .gaze
                .map(|(_, yaw, pitch)| GazeAction::new(yaw, pitch).expect("validated input"))
                .unwrap_or_default();
        }
        world.resource_mut::<PredictionRuntime>().predicted_tick = Some(entry.tick);
        world
            .resource_mut::<Time<Fixed>>()
            .advance_by(Duration::from_secs_f32(snapshot.fixed_dt_seconds));
        commit_creature_motion(world);
    }
}

fn reset_prediction_session(world: &mut World, session_epoch: Option<u64>) {
    let entity = world.resource_mut::<PredictionRuntime>().entity.take();
    if let Some(entity) = entity {
        world.despawn(entity);
    }
    *world.resource_mut::<PredictionRuntime>() = PredictionRuntime {
        session_epoch,
        ..Default::default()
    };
    world.resource_mut::<PredictionInputs>().clear();
    *world.resource_mut::<CameraTranslationProjection>() = CameraTranslationProjection::default();
}

fn project_predicted_camera(
    time: Res<Time>,
    controller: Res<ClientPlayerController>,
    creatures: Query<&Transform, With<PredictedCreature>>,
    mut projection: ResMut<CameraTranslationProjection>,
    mut cameras: Query<&mut Transform, (With<bevy::prelude::Camera>, Without<PredictedCreature>)>,
) {
    if controller.is_spirit_walking() {
        projection.initialized = false;
        return;
    }
    let (Some(camera), Some(pose)) = (controller.camera(), creatures.iter().next()) else {
        return;
    };
    if let Ok(mut camera) = cameras.get_mut(camera) {
        project_bound_camera_translation(
            pose.translation,
            time.delta_secs(),
            &mut projection,
            &mut camera,
        );
    }
}

// Rotation remains an immediate local projection. Copying the lower-frequency
// predicted gaze here would race the mouse-input system and cause visible
// rotate/rollback oscillation between prediction ticks. Translation uses a
// presentation-only smoothing state so reconciliation never feeds visible
// fixed-tick corrections directly into the camera.
fn project_bound_camera_translation(
    target: Vec3,
    delta_seconds: f32,
    projection: &mut CameraTranslationProjection,
    camera: &mut Transform,
) {
    let error = target - projection.displayed;
    if !projection.initialized
        || !error.is_finite()
        || error.length_squared() > CAMERA_TRANSLATION_SNAP_DISTANCE.powi(2)
    {
        projection.displayed = target;
        projection.initialized = true;
    } else {
        let alpha = 1.0 - (-CAMERA_TRANSLATION_RESPONSE * delta_seconds.max(0.0)).exp();
        projection.displayed += error * alpha.clamp(0.0, 1.0);
    }
    camera.translation = projection.displayed;
}

#[cfg(test)]
mod tests;
