//! Mapping between a committed Creature Record and its wire-visible snapshot.

use bevy::prelude::{Entity, Quat, Transform, Vec3, World};
use roundo_contracts::{
    ControllerBindingId, ControllerInputAcknowledgement, CreatureEnvironmentSample,
    CreatureLocomotion, CreatureMotionSnapshot, EnvironmentRevision, InputSequence, SimulationTick,
};
use roundo_creature::{
    BodyOrientation, EnvironmentSample, GazeAction, GazeOrientation, KinematicVelocity,
    LocomotionState, MovementAction,
};

#[cfg(feature = "server")]
use roundo_creature::{CommittedFactoryMotion, CreatureMotionRecord, FactoryMotionRoute};

/// ACK metadata captured before motion commit for one exact authorized route/tick.
#[cfg(feature = "server")]
pub(crate) struct CreatureSnapshotMetadata {
    pub route: FactoryMotionRoute,
    pub tick: u64,
    pub acknowledged_movement: u64,
    pub acknowledged_gaze: u64,
    pub movement_binding: Option<ControllerBindingId>,
    pub gaze_binding: Option<ControllerBindingId>,
}

/// Encodes one Record only when it exactly matches the immutable motion receipt
/// and the pre-commit metadata still names the same route and tick.
#[cfg(feature = "server")]
pub(crate) fn encode_creature_snapshot(
    record: &CreatureMotionRecord,
    motion: CommittedFactoryMotion,
    metadata: CreatureSnapshotMetadata,
) -> Option<CreatureMotionSnapshot> {
    let experience = motion.experience();
    let record_tick = record.committed_tick().get();
    if metadata.route != motion.route()
        || metadata.tick != record_tick
        || metadata.tick != experience.committed_tick().get()
        || *record != CreatureMotionRecord::from_experience(experience)
    {
        return None;
    }
    let environment = motion.environment();
    Some(CreatureMotionSnapshot {
        fixed_dt_seconds: motion.fixed_dt_seconds(),
        // Record is the sole published tick source; metadata cannot override it.
        simulation_tick: SimulationTick(record_tick),
        translation: *record.translation(),
        body_orientation: *record.body_orientation(),
        gaze_orientation: *record.gaze_orientation(),
        velocity: *record.linear_velocity(),
        locomotion: match record.locomotion() {
            LocomotionState::Grounded => CreatureLocomotion::Grounded,
            LocomotionState::Floating => CreatureLocomotion::Floating,
        },
        environment: CreatureEnvironmentSample {
            gravity: environment.gravity.to_array(),
            static_friction: environment.static_friction,
            kinetic_friction: environment.kinetic_friction,
            revision: EnvironmentRevision(environment.revision),
        },
        acknowledged_movement: ControllerInputAcknowledgement {
            binding_id: metadata.movement_binding,
            sequence: InputSequence(metadata.acknowledged_movement),
        },
        acknowledged_gaze: ControllerInputAcknowledgement {
            binding_id: metadata.gaze_binding,
            sequence: InputSequence(metadata.acknowledged_gaze),
        },
    })
}

/// Replaces one predicted Creature with an authoritative committed observation.
pub(crate) fn restore_creature_snapshot(
    world: &mut World,
    entity: Entity,
    snapshot: CreatureMotionSnapshot,
) {
    let environment = EnvironmentSample::new(
        Vec3::from_array(snapshot.environment.gravity),
        snapshot.environment.static_friction,
        snapshot.environment.kinetic_friction,
        snapshot.environment.revision.0,
    )
    .unwrap_or_default();
    let locomotion = match snapshot.locomotion {
        CreatureLocomotion::Grounded => LocomotionState::Grounded,
        CreatureLocomotion::Floating => LocomotionState::Floating,
    };
    let body_orientation = Quat::from_array(snapshot.body_orientation);
    let mut entity = world.entity_mut(entity);
    *entity
        .get_mut::<Transform>()
        .expect("Creature has Transform") = Transform {
        translation: Vec3::from_array(snapshot.translation),
        // The locomotion frame is restored below, never onto the movement AABB.
        rotation: Quat::IDENTITY,
        ..Default::default()
    };
    *entity
        .get_mut::<BodyOrientation>()
        .expect("Creature has BodyOrientation") = BodyOrientation(body_orientation);
    *entity
        .get_mut::<GazeOrientation>()
        .expect("Creature has GazeOrientation") =
        GazeOrientation(Quat::from_array(snapshot.gaze_orientation));
    *entity
        .get_mut::<KinematicVelocity>()
        .expect("Creature has KinematicVelocity") =
        KinematicVelocity(Vec3::from_array(snapshot.velocity));
    *entity
        .get_mut::<LocomotionState>()
        .expect("Creature has LocomotionState") = locomotion;
    *entity
        .get_mut::<EnvironmentSample>()
        .expect("Creature has EnvironmentSample") = environment;
    *entity
        .get_mut::<MovementAction>()
        .expect("execution projection has MovementAction") = MovementAction::default();
    *entity
        .get_mut::<GazeAction>()
        .expect("execution projection has GazeAction") = GazeAction::default();
}
