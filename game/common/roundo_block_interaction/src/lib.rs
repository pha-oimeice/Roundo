//! Authoritative Block Interaction policy.
//!
//! This module translates accepted Marionette interactions into Local
//! Coordinate mutations. Hosts only install the plugin; raycast distance,
//! destroy/place semantics, and schedule placement stay local to this module.

use bevy::prelude::{
    App, FixedUpdate, IntoScheduleConfigs, MessageReader, MessageWriter, Plugin, Query, Transform,
};
use roundo_local_coordinate::{
    EMPTY_VOXEL_ID, LocalCoordinateCRUDMessage, LocalCoordinateCRUDMessageEnum, LocalCoordinateSet,
    PositionedAtomicVoxel, VoxelRaycaster,
};
use roundo_marionette::{BlockInteraction, BlockInteractionMessage, MarionetteServerSet};

pub const DEFAULT_BLOCK_INTERACTION_DISTANCE: f32 = 8.0;

pub struct BlockInteractionPlugin {
    maximum_distance: f32,
}

impl BlockInteractionPlugin {
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
        app.insert_resource(BlockInteractionSettings {
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

fn apply_block_interactions(
    settings: bevy::prelude::Res<BlockInteractionSettings>,
    mut messages: MessageReader<BlockInteractionMessage>,
    transforms: Query<&Transform>,
    raycaster: VoxelRaycaster,
    mut voxel_updates: MessageWriter<LocalCoordinateCRUDMessage>,
) {
    for message in messages.read() {
        let Ok(transform) = transforms.get(message.entity) else {
            continue;
        };
        let Some(hit) = raycaster.cast(
            bevy::math::Ray3d::new(transform.translation, transform.forward()),
            settings.maximum_distance,
        ) else {
            continue;
        };
        let (position, voxel) = match message.interaction {
            BlockInteraction::Destroy => (hit.voxel_position(), EMPTY_VOXEL_ID),
            BlockInteraction::Place { voxel_id } => (
                hit.previous_voxel_position,
                roundo_local_coordinate::AtomicVoxelId(voxel_id),
            ),
        };
        voxel_updates.write(LocalCoordinateCRUDMessage(
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
    use bevy::prelude::{App, Fixed, GlobalTransform, IVec3, Time};
    use roundo_local_coordinate::{
        LocalCoordinate, SOLID_VOXEL_ID, local_coordinate::plugins::LocalCoordinateBasePlugin,
    };
    use roundo_marionette::{
        ConnectionId, ControllerCommand, DestroyBlockControllerAction, MarionetteServerPlugin,
        NetworkControllerTarget, PlaceBlockControllerAction, PlayerControllerCommand,
        PlayerControllers, ServerMarionetteCommand,
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
            Transform::from_xyz(0.5, 0.5, 1.5),
        ));
        app.world_mut().run_schedule(FixedUpdate);
        (app, ipc, coordinate)
    }

    #[test]
    fn destroy_replaces_the_hit_voxel_with_air() {
        let (mut app, ipc, coordinate) = interaction_app();
        ipc.try_send(ServerMarionetteCommand::UsePlayerController {
            connection_id: ConnectionId(3),
            command: PlayerControllerCommand::DestroyBlock(ControllerCommand {
                sequence: 1,
                action: DestroyBlockControllerAction,
            }),
        })
        .unwrap();
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
        let (mut app, ipc, coordinate) = interaction_app();
        ipc.try_send(ServerMarionetteCommand::UsePlayerController {
            connection_id: ConnectionId(3),
            command: PlayerControllerCommand::PlaceBlock(ControllerCommand {
                sequence: 1,
                action: PlaceBlockControllerAction { voxel_id: 1 },
            }),
        })
        .unwrap();
        app.world_mut().run_schedule(FixedUpdate);

        assert_eq!(
            app.world()
                .get::<LocalCoordinate>(coordinate)
                .unwrap()
                .voxel(IVec3::Z),
            Some(roundo_local_coordinate::AtomicVoxelId(1))
        );
    }
}
