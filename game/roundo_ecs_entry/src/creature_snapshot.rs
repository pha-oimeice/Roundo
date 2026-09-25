//! Mapping between committed Creature motion and its wire-visible snapshot.

use bevy::prelude::{Entity, Quat, Transform, Vec3, World};
use roundo_contracts::{
    CreatureEnvironmentSample, CreatureLocomotion, CreatureMotionSnapshot, EnvironmentRevision,
    InputSequence, SimulationTick,
};
use roundo_creature::{
    BodyOrientation, EnvironmentSample, GazeAction, GazeOrientation, KinematicVelocity,
    LocomotionState, MovementAction,
};

pub(crate) struct CommittedCreatureMotion<'a> {
    pub transform: &'a Transform,
    pub body: &'a BodyOrientation,
    pub gaze: &'a GazeOrientation,
    pub velocity: &'a KinematicVelocity,
    pub locomotion: &'a LocomotionState,
    pub environment: &'a EnvironmentSample,
}

pub(crate) struct CreatureSnapshotMetadata {
    pub fixed_dt_seconds: f32,
    pub simulation_tick: u64,
    pub acknowledged_movement: u64,
    pub acknowledged_gaze: u64,
}

/// Captures one immutable wire observation from committed Creature state.
pub(crate) fn encode_creature_snapshot(
    motion: CommittedCreatureMotion<'_>,
    metadata: CreatureSnapshotMetadata,
) -> CreatureMotionSnapshot {
    CreatureMotionSnapshot {
        fixed_dt_seconds: metadata.fixed_dt_seconds,
        simulation_tick: SimulationTick(metadata.simulation_tick),
        translation: motion.transform.translation.to_array(),
        body_orientation: motion.body.0.to_array(),
        gaze_orientation: motion.gaze.0.to_array(),
        velocity: motion.velocity.0.to_array(),
        locomotion: match motion.locomotion {
            LocomotionState::Grounded => CreatureLocomotion::Grounded,
            LocomotionState::Floating => CreatureLocomotion::Floating,
        },
        environment: CreatureEnvironmentSample {
            gravity: motion.environment.gravity.to_array(),
            static_friction: motion.environment.static_friction,
            kinetic_friction: motion.environment.kinetic_friction,
            revision: EnvironmentRevision(motion.environment.revision),
        },
        acknowledged_movement: InputSequence(metadata.acknowledged_movement),
        acknowledged_gaze: InputSequence(metadata.acknowledged_gaze),
    }
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
        rotation: body_orientation,
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

#[cfg(test)]
mod tests {
    use super::*;
    use roundo_creature::TestExecutionBodyPrototype;

    #[test]
    fn encode_then_restore_preserves_committed_motion_fields() {
        let mut source = World::new();
        let source_entity = source
            .spawn(TestExecutionBodyPrototype::default().materialize_runtime(
                Vec3::new(1.0, 2.0, 3.0),
                EnvironmentSample::new(Vec3::new(0.0, -3.0, 1.0), 0.7, 0.4, 9).unwrap(),
            ))
            .id();
        source
            .get_mut::<KinematicVelocity>(source_entity)
            .unwrap()
            .0 = Vec3::new(4.0, 5.0, 6.0);
        let snapshot = encode_creature_snapshot(
            CommittedCreatureMotion {
                transform: source.get(source_entity).unwrap(),
                body: source.get(source_entity).unwrap(),
                gaze: source.get(source_entity).unwrap(),
                velocity: source.get(source_entity).unwrap(),
                locomotion: source.get(source_entity).unwrap(),
                environment: source.get(source_entity).unwrap(),
            },
            CreatureSnapshotMetadata {
                fixed_dt_seconds: 1.0 / 64.0,
                simulation_tick: 11,
                acknowledged_movement: 7,
                acknowledged_gaze: 8,
            },
        );

        let mut target = World::new();
        let target_entity = target
            .spawn(
                TestExecutionBodyPrototype::default()
                    .materialize_runtime(Vec3::ZERO, EnvironmentSample::default()),
            )
            .id();
        restore_creature_snapshot(&mut target, target_entity, snapshot);

        assert_eq!(
            target.get::<Transform>(target_entity).unwrap().translation,
            Vec3::new(1.0, 2.0, 3.0)
        );
        assert_eq!(
            target.get::<KinematicVelocity>(target_entity).unwrap().0,
            Vec3::new(4.0, 5.0, 6.0)
        );
        assert_eq!(
            target
                .get::<EnvironmentSample>(target_entity)
                .unwrap()
                .revision,
            9
        );
    }
}
