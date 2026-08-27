use crate::mod_assets::mod_asset_plugin;
use avian3d::PhysicsPlugins;
use bevy::MinimalPlugins;
use bevy::app::App;
use bevy::mesh::MeshPlugin;
use bevy::prelude::{
    Fixed, FixedUpdate, IntoScheduleConfigs, MessageReader, MessageWriter, Query, Time, Transform,
};
use roundo_local_coordinate::{
    EMPTY_VOXEL_ID, LocalCoordinateCRUDMessage, LocalCoordinateCRUDMessageEnum,
    LocalCoordinateServerIpc, LocalCoordinateServerPlugin, LocalCoordinateSet,
    PositionedAtomicVoxel, VoxelRaycaster,
};
use roundo_marionette::{
    BlockInteraction, BlockInteractionMessage, MarionetteServerPlugin, MarionetteServerSet,
    ServerMarionetteIpc,
};
use roundo_portal::RoundoPortalPlugin;
use roundo_presence::{PresenceServerIpc, PresenceServerSettings, RoundoPresenceServerPlugin};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU32, Ordering};

const DEFAULT_TICK_RATE: u32 = 20;
const DEFAULT_BLOCK_INTERACTION_DISTANCE: f32 = 8.0;

static PRESENCE_PLUGIN: LazyLock<RoundoPresenceServerPlugin> =
    LazyLock::new(RoundoPresenceServerPlugin::new);
static MARIONETTE_PLUGIN: LazyLock<MarionetteServerPlugin> =
    LazyLock::new(MarionetteServerPlugin::new);
static LOCAL_COORDINATE_PLUGIN: LazyLock<LocalCoordinateServerPlugin> =
    LazyLock::new(LocalCoordinateServerPlugin::new);
static TICK_RATE: AtomicU32 = AtomicU32::new(DEFAULT_TICK_RATE);
static PRESENCE_RADIUS: AtomicU32 = AtomicU32::new(128.0_f32.to_bits());

pub fn server_presence_ipc() -> PresenceServerIpc {
    PRESENCE_PLUGIN.ipc()
}

pub fn server_marionette_ipc() -> ServerMarionetteIpc {
    MARIONETTE_PLUGIN.ipc()
}

pub fn server_local_coordinate_ipc() -> LocalCoordinateServerIpc {
    LOCAL_COORDINATE_PLUGIN.ipc()
}

pub fn configure_server_tick_rate(tick_rate: u32) {
    TICK_RATE.store(tick_rate.max(1), Ordering::Relaxed);
}

pub fn configure_server_presence_radius(radius: f32) {
    PRESENCE_RADIUS.store(radius.to_bits(), Ordering::Relaxed);
}

pub fn create_ecs_server_app() -> App {
    let mut app = App::new();
    app.insert_resource(Time::<Fixed>::from_hz(f64::from(
        TICK_RATE.load(Ordering::Relaxed),
    )))
    .insert_resource(PresenceServerSettings::new(f32::from_bits(
        PRESENCE_RADIUS.load(Ordering::Relaxed),
    )));
    app.add_plugins((
        MinimalPlugins,
        mod_asset_plugin(),
        MeshPlugin,
        PhysicsPlugins::default(),
        RoundoPortalPlugin,
        (*MARIONETTE_PLUGIN).clone(),
        (*PRESENCE_PLUGIN).clone(),
        (*LOCAL_COORDINATE_PLUGIN).clone(),
    ))
    .add_systems(
        FixedUpdate,
        apply_block_interactions
            .in_set(MarionetteServerSet::BlockInteractions)
            .before(LocalCoordinateSet::ApplyCrud),
    );
    app
}

fn apply_block_interactions(
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
            DEFAULT_BLOCK_INTERACTION_DISTANCE,
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

pub fn run_ecs_server() {
    create_ecs_server_app().run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::{App, GlobalTransform, IVec3};
    use roundo_local_coordinate::local_coordinate::plugins::LocalCoordinateBasePlugin;
    use roundo_local_coordinate::{LocalCoordinate, SOLID_VOXEL_ID};
    use roundo_marionette::{
        ConnectionId, ControllerCommand, DestroyBlockController, DestroyBlockControllerAction,
        NetworkControllerTarget, PlaceBlockController, PlaceBlockControllerAction,
        PlayerControllerCommand, PlayerControllers, ServerMarionetteCommand,
    };

    fn interaction_app() -> (App, ServerMarionetteIpc, bevy::prelude::Entity) {
        let marionette = MarionetteServerPlugin::new();
        let ipc = marionette.ipc();
        let mut app = App::new();
        app.init_resource::<Time<Fixed>>()
            .add_plugins((marionette, LocalCoordinateBasePlugin))
            .add_systems(
                FixedUpdate,
                apply_block_interactions
                    .in_set(MarionetteServerSet::BlockInteractions)
                    .before(LocalCoordinateSet::ApplyCrud),
            );
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
    fn destroy_event_replaces_the_dda_hit_with_air() {
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

        let local_coordinate = app.world().get::<LocalCoordinate>(coordinate).unwrap();
        assert!(
            local_coordinate.chunks[&IVec3::ZERO]
                .voxel(IVec3::ZERO)
                .is_none()
        );
    }

    #[test]
    fn place_event_writes_the_fixed_voxel_before_the_dda_hit() {
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

        let local_coordinate = app.world().get::<LocalCoordinate>(coordinate).unwrap();
        assert_eq!(
            local_coordinate.chunks[&IVec3::ZERO]
                .voxel(IVec3::Z)
                .unwrap()
                .0,
            1
        );
    }

    #[test]
    fn accepted_block_interactions_are_independent_of_transform() {
        let marionette = MarionetteServerPlugin::new();
        let ipc = marionette.ipc();
        let mut app = App::new();
        app.init_resource::<Time<Fixed>>()
            .add_plugins((marionette, LocalCoordinateBasePlugin))
            .add_systems(
                FixedUpdate,
                apply_block_interactions
                    .in_set(MarionetteServerSet::BlockInteractions)
                    .before(LocalCoordinateSet::ApplyCrud),
            );
        let entity = app
            .world_mut()
            .spawn((
                DestroyBlockController::default(),
                PlaceBlockController::default(),
                NetworkControllerTarget {
                    connection_id: ConnectionId(4),
                },
            ))
            .id();

        ipc.try_send(ServerMarionetteCommand::UsePlayerController {
            connection_id: ConnectionId(4),
            command: PlayerControllerCommand::DestroyBlock(ControllerCommand {
                sequence: 1,
                action: DestroyBlockControllerAction,
            }),
        })
        .unwrap();
        ipc.try_send(ServerMarionetteCommand::UsePlayerController {
            connection_id: ConnectionId(4),
            command: PlayerControllerCommand::PlaceBlock(ControllerCommand {
                sequence: 1,
                action: PlaceBlockControllerAction { voxel_id: 1 },
            }),
        })
        .unwrap();
        app.world_mut().run_schedule(FixedUpdate);

        assert_eq!(
            app.world()
                .get::<DestroyBlockController>(entity)
                .unwrap()
                .last_sequence(),
            1
        );
        assert_eq!(
            app.world()
                .get::<PlaceBlockController>(entity)
                .unwrap()
                .last_sequence(),
            1
        );
    }
}
