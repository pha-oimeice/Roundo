//! Authoritative Block Interaction policy.
//!
//! This module translates accepted Marionette interactions into Local
//! Coordinate mutations. Hosts only install the plugin; raycast distance,
//! destroy/place semantics, and schedule placement stay local to this module.

use bevy::prelude::{
    App, Component, FixedUpdate, IntoScheduleConfigs, MessageReader, MessageWriter, Plugin, Query,
    Transform, With,
};
use roundo_local_coordinate::{
    AtomicVoxelId, AtomicVoxelRegistry, EMPTY_VOXEL_ID, LocalCoordinateCRUDMessage,
    LocalCoordinateCRUDMessageEnum, LocalCoordinateSet, PositionedAtomicVoxel, VoxelRaycaster,
};
use roundo_marionette::{BlockInteraction, MarionetteServerSet};

/// Fallback maximum ray distance in world units.
pub const DEFAULT_BLOCK_INTERACTION_DISTANCE: f32 = 8.0;

/// Marks the entity whose Creature pose is authoritative for block interaction.
/// Composition adapters route controller messages to this entity.
#[derive(Component)]
pub struct BlockInteractionPose;

/// A composition adapter's resolved interaction target. It deliberately names a
/// Creature pose entity, never a Controller entity.
#[derive(bevy::prelude::Message, Clone, Copy, Debug, PartialEq)]
pub struct RoutedBlockInteractionMessage {
    pub entity: bevy::prelude::Entity,
    pub gaze_direction: bevy::math::Dir3,
    pub interaction: BlockInteraction,
}

/// Applies validated controller interactions to authoritative voxel state.
///
/// Each interaction casts from the controlled Creature's translation using the
/// gaze direction resolved by the composition adapter. A destroy writes the empty
/// ID at the first hit; a placement writes a registry-approved ID at the preceding voxel. The
/// resulting CRUD message is applied later in the same fixed schedule.
pub struct BlockInteractionPlugin {
    maximum_distance: f32,
}

impl BlockInteractionPlugin {
    /// Creates policy with a finite positive maximum world-space distance.
    ///
    /// Invalid values are replaced by [`DEFAULT_BLOCK_INTERACTION_DISTANCE`].
    pub fn new(maximum_distance: f32) -> Self {
        Self {
            maximum_distance: if maximum_distance.is_finite() && maximum_distance > 0.0 {
                maximum_distance
            } else {
                DEFAULT_BLOCK_INTERACTION_DISTANCE
            },
        }
    }
}

impl Default for BlockInteractionPlugin {
    fn default() -> Self {
        Self::new(DEFAULT_BLOCK_INTERACTION_DISTANCE)
    }
}

#[derive(bevy::prelude::Resource)]
struct BlockInteractionSettings {
    maximum_distance: f32,
}

impl Plugin for BlockInteractionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AtomicVoxelRegistry>()
            .add_message::<RoutedBlockInteractionMessage>()
            .insert_resource(BlockInteractionSettings {
                maximum_distance: self.maximum_distance,
            })
            .add_systems(
                FixedUpdate,
                apply_block_interactions
                    .in_set(MarionetteServerSet::BlockInteractions)
                    .before(LocalCoordinateSet::ApplyCrud),
            );
    }
}

// Silently rejects missing transforms, ray misses, and unregistered/unplaceable IDs.
// Accepted interactions enqueue one voxel replacement; they do not mutate the
// coordinate immediately or report completion to the controller domain.
fn apply_block_interactions(
    settings: bevy::prelude::Res<BlockInteractionSettings>,
    voxels: bevy::prelude::Res<AtomicVoxelRegistry>,
    mut messages: MessageReader<RoutedBlockInteractionMessage>,
    transforms: Query<&Transform, With<BlockInteractionPose>>,
    raycaster: VoxelRaycaster,
    mut voxel_updates: MessageWriter<LocalCoordinateCRUDMessage>,
) {
    let interactions = messages.read();
    for message in interactions {
        let Ok(transform) = transforms.get(message.entity) else {
            continue;
        };
        let Some(hit) = raycaster.cast(
            bevy::math::Ray3d::new(transform.translation, message.gaze_direction),
            settings.maximum_distance,
        ) else {
            continue;
        };
        let (position, voxel) = match message.interaction {
            BlockInteraction::Destroy => (hit.voxel_position(), EMPTY_VOXEL_ID),
            BlockInteraction::Place { voxel_id } => {
                let voxel = AtomicVoxelId(voxel_id);
                if !voxels.permits_placement(voxel) {
                    continue;
                }
                (hit.previous_voxel_position, voxel)
            }
        };
        let _update_message = voxel_updates.write(LocalCoordinateCRUDMessage(
            LocalCoordinateCRUDMessageEnum::Update {
                key: hit.chunk.local_coordinate_entity(),
                value: vec![PositionedAtomicVoxel { position, voxel }],
            },
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::{App, Fixed, GlobalTransform, IVec3, Quat, Time};
    use roundo_local_coordinate::{
        LocalCoordinate, SOLID_VOXEL_ID, local_coordinate::plugins::LocalCoordinateBasePlugin,
    };
    use roundo_marionette::{
        ConnectionId, MarionetteServerPlugin, NetworkControllerTarget, PlayerControllers,
    };

    fn interaction_app() -> (
        App,
        roundo_marionette::ServerMarionetteIpc,
        bevy::prelude::Entity,
    ) {
        let marionette = MarionetteServerPlugin::new();
        let ipc = marionette.ipc();
        let mut app = App::new();
        app.init_resource::<Time<Fixed>>().add_plugins((
            marionette,
            LocalCoordinateBasePlugin,
            BlockInteractionPlugin::default(),
        ));
        let coordinate = app
            .world_mut()
            .spawn((
                LocalCoordinate::from_voxels([PositionedAtomicVoxel {
                    position: IVec3::ZERO,
                    voxel: SOLID_VOXEL_ID,
                }]),
                GlobalTransform::default(),
            ))
            .id();
        app.world_mut().spawn((
            PlayerControllers::default(),
            NetworkControllerTarget {
                connection_id: ConnectionId(3),
            },
            BlockInteractionPose,
            Transform::from_xyz(0.5, 0.5, 1.5),
        ));
        app.world_mut().run_schedule(FixedUpdate);
        (app, ipc, coordinate)
    }

    #[test]
    fn destroy_replaces_the_hit_voxel_with_air() {
        let (mut app, _ipc, coordinate) = interaction_app();
        let pose = app
            .world_mut()
            .query::<(bevy::prelude::Entity, &BlockInteractionPose)>()
            .single(app.world())
            .unwrap()
            .0;
        app.world_mut().get_mut::<Transform>(pose).unwrap().rotation =
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        app.world_mut()
            .write_message(RoutedBlockInteractionMessage {
                entity: pose,
                gaze_direction: bevy::math::Dir3::NEG_Z,
                interaction: BlockInteraction::Destroy,
            });
        app.world_mut().run_schedule(FixedUpdate);

        assert_eq!(
            app.world()
                .get::<LocalCoordinate>(coordinate)
                .unwrap()
                .voxel(IVec3::ZERO),
            None
        );
    }

    #[test]
    fn place_writes_the_requested_voxel_before_the_hit() {
        let (mut app, _ipc, coordinate) = interaction_app();
        let pose = app
            .world_mut()
            .query::<(bevy::prelude::Entity, &BlockInteractionPose)>()
            .single(app.world())
            .unwrap()
            .0;
        app.world_mut()
            .write_message(RoutedBlockInteractionMessage {
                entity: pose,
                gaze_direction: bevy::math::Dir3::NEG_Z,
                interaction: BlockInteraction::Place { voxel_id: 1 },
            });
        app.world_mut().run_schedule(FixedUpdate);

        assert_eq!(
            app.world()
                .get::<LocalCoordinate>(coordinate)
                .unwrap()
                .voxel(IVec3::Z),
            Some(roundo_local_coordinate::AtomicVoxelId(1))
        );
    }

    #[test]
    fn unknown_voxel_id_cannot_mutate_authoritative_state() {
        let (mut app, _ipc, coordinate) = interaction_app();
        let pose = app
            .world_mut()
            .query::<(bevy::prelude::Entity, &BlockInteractionPose)>()
            .single(app.world())
            .unwrap()
            .0;
        app.world_mut()
            .write_message(RoutedBlockInteractionMessage {
                entity: pose,
                gaze_direction: bevy::math::Dir3::NEG_Z,
                interaction: BlockInteraction::Place { voxel_id: 99 },
            });
        app.world_mut().run_schedule(FixedUpdate);

        assert_eq!(
            app.world()
                .get::<LocalCoordinate>(coordinate)
                .unwrap()
                .voxel(IVec3::Z),
            None
        );
    }
}
