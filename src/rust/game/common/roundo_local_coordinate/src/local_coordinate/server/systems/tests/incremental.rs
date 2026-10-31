use super::*;
use crate::AtomicVoxelRegistry;
use crate::local_coordinate::attributes::SparseEnvironmentOverride;
use bevy::prelude::{Entity, IntoScheduleConfigs};

pub(super) fn fixture() -> (App, LocalCoordinateServerIpc, Entity) {
    let transport = CrossbeamThreadPipe::new();
    let mut app = App::new();
    app.add_plugins(LocalCoordinateBasePlugin)
        .init_resource::<LocalCoordinateServerWorld>()
        .init_resource::<crate::VirtualChunkEnvironmentMap>()
        .insert_resource(LocalCoordinateServerPipe(transport.endpoint_b()))
        .add_observer(invalidate_installed_coordinate)
        .configure_sets(
            Update,
            (
                LocalCoordinateSet::RebuildIndex,
                LocalCoordinateServerSet::Commit,
            )
                .chain(),
        )
        .add_systems(
            Update,
            (
                update_authoritative_chunk_versions,
                publish_subscription_changes,
            )
                .chain()
                .in_set(LocalCoordinateServerSet::Commit),
        );
    let entity = app
        .world_mut()
        .spawn((
            PcgLocalCoordinate::new(LocalCoordinateId(1), SuperflatGenerator::new(0)),
            LocalCoordinate::default(),
            GlobalTransform::default(),
        ))
        .id();
    for id in [1, 2] {
        app.world_mut()
            .resource_mut::<LocalCoordinateServerWorld>()
            .apply_stream_command(LocalCoordinateServerCommand::SubscribePlayer {
                connection_id: ConnectionId(id),
                player_id: PlayerId(id),
            });
    }
    (app, transport.endpoint_a(), entity)
}
pub(super) fn install(app: &mut App, entity: Entity, coordinate: [i64; 3]) {
    let generated =
        SuperflatGenerator::new(0).generate_chunk(coordinate[0], coordinate[1], coordinate[2]);
    replace_generated_chunk(
        &mut app.world_mut().get_mut::<LocalCoordinate>(entity).unwrap(),
        &generated,
        &AtomicVoxelRegistry::builtin(),
    );
    app.world_mut()
        .resource_mut::<LocalCoordinateServerWorld>()
        .loaded_chunks
        .entry(LocalCoordinateId(1))
        .or_default()
        .insert(coordinate, generated);
}
fn observations(app: &mut App) {
    let mut world = app.world_mut().resource_mut::<LocalCoordinateServerWorld>();
    world.observation_by_anchor = world
        .subscriptions
        .values()
        .map(|subscription| {
            let anchor = subscription.anchor;
            (
                anchor.id,
                ObservationRegion {
                    center: anchor.position,
                    streaming_center: anchor.position,
                    radius: f64::from(anchor.radius_chunks) * CHUNK_EDGE_LENGTH as f64,
                    scene_id: anchor.scene_id,
                },
            )
        })
        .collect();
}
pub(super) fn compare_with_full_rebuild(app: &mut App, ipc: &LocalCoordinateServerIpc) {
    observations(app);
    // One connection exercises incremental publication. The other runs the
    // existing full-snapshot algorithm as an independent membership oracle.
    app.world_mut()
        .resource_mut::<LocalCoordinateServerWorld>()
        .subscriptions
        .get_mut(&ConnectionId(2))
        .unwrap()
        .last_view = None;
    app.world_mut().run_schedule(Update);
    let mut events = [Vec::new(), Vec::new()];
    while let Some(event) = ipc.try_receive() {
        let (connection, value) = match event {
            LocalCoordinateServerEvent::Spawned {
                connection_id,
                local_coordinate_id,
            } => (connection_id, format!("spawn:{local_coordinate_id:?}")),
            LocalCoordinateServerEvent::ChunkVersions {
                connection_id,
                chunks,
            } => (connection_id, format!("versions:{chunks:?}")),
            LocalCoordinateServerEvent::ChunkUnloaded {
                connection_id,
                local_coordinate_id,
                coordinate,
            } => (
                connection_id,
                format!("unload:{local_coordinate_id:?}:{coordinate:?}"),
            ),
            LocalCoordinateServerEvent::EnvironmentOverrides {
                connection_id,
                overrides,
            } => (connection_id, format!("environment:{overrides:?}")),
            event => panic!("unexpected event {event:?}"),
        };
        events[(connection.0 - 1) as usize].push(value);
    }
    events[0].sort();
    events[1].sort();
    assert_eq!(events[0], events[1]);
    let world = app.world().resource::<LocalCoordinateServerWorld>();
    let incremental = &world.subscriptions[&ConnectionId(1)];
    let full = &world.subscriptions[&ConnectionId(2)];
    assert_eq!(incremental.advertised_chunks, full.advertised_chunks);
    assert_eq!(incremental.spawned_coordinates, full.spawned_coordinates);
    assert_eq!(
        incremental.advertised_environment_revisions,
        full.advertised_environment_revisions
    );
    assert_eq!(incremental.pending_payloads, full.pending_payloads);
}
#[test]
fn incremental_subscription_matches_full_membership_for_edits_loads_removals_and_view_changes() {
    let (mut app, ipc, entity) = fixture();
    install(&mut app, entity, [0, 0, 0]);
    install(&mut app, entity, [30, 0, 0]);
    compare_with_full_rebuild(&mut app, &ipc);
    let initial_rebuilds = app
        .world()
        .resource::<LocalCoordinateServerWorld>()
        .subscription_rebuilds;
    compare_with_full_rebuild(&mut app, &ipc);
    assert_eq!(
        app.world()
            .resource::<LocalCoordinateServerWorld>()
            .subscription_rebuilds,
        initial_rebuilds + 1,
        "an idle cached connection must not rebuild its subscription"
    );
    app.world_mut()
        .get_mut::<LocalCoordinate>(entity)
        .unwrap()
        .apply_voxel(PositionedAtomicVoxel {
            position: IVec3::ZERO,
            voxel: crate::EMPTY_VOXEL_ID,
        });
    compare_with_full_rebuild(&mut app, &ipc);
    assert_eq!(
        app.world()
            .resource::<LocalCoordinateServerWorld>()
            .subscription_rebuilds,
        initial_rebuilds + 2,
        "a voxel revision must not invalidate the whole membership"
    );
    install(&mut app, entity, [1, 0, 0]);
    install(&mut app, entity, [200, 0, 0]);
    compare_with_full_rebuild(&mut app, &ipc);
    assert_eq!(
        app.world()
            .resource::<LocalCoordinateServerWorld>()
            .subscription_rebuilds,
        initial_rebuilds + 3,
        "new chunks must be classified individually, not cause a whole-view scan"
    );
    app.world_mut()
        .get_mut::<LocalCoordinate>(entity)
        .unwrap()
        .remove_chunk(IVec3::ZERO);
    app.world_mut()
        .resource_mut::<LocalCoordinateServerWorld>()
        .loaded_chunks
        .get_mut(&LocalCoordinateId(1))
        .unwrap()
        .remove(&[0, 0, 0]);
    compare_with_full_rebuild(&mut app, &ipc);

    for radius in [1, 64] {
        for subscription in app
            .world_mut()
            .resource_mut::<LocalCoordinateServerWorld>()
            .subscriptions
            .values_mut()
        {
            subscription.anchor.radius_chunks = radius;
        }
        compare_with_full_rebuild(&mut app, &ipc);
    }
    for position in [[480.0, 0.0, 0.0], [0.0; 3]] {
        for subscription in app
            .world_mut()
            .resource_mut::<LocalCoordinateServerWorld>()
            .subscriptions
            .values_mut()
        {
            subscription.prediction_anchor.position = position;
        }
        compare_with_full_rebuild(&mut app, &ipc);
    }
    *app.world_mut().get_mut::<GlobalTransform>(entity).unwrap() =
        GlobalTransform::from_translation(Vec3::new(3000.0, 0.0, 0.0));
    compare_with_full_rebuild(&mut app, &ipc);
    app.world_mut()
        .get_mut::<PcgLocalCoordinate>(entity)
        .unwrap()
        .scene_id = SceneId::S0 { room_id: 9 };
    compare_with_full_rebuild(&mut app, &ipc);
    app.world_mut()
        .get_mut::<PcgLocalCoordinate>(entity)
        .unwrap()
        .scene_id = SceneId::S1;
    *app.world_mut().get_mut::<GlobalTransform>(entity).unwrap() = GlobalTransform::default();
    compare_with_full_rebuild(&mut app, &ipc);

    app.world_mut()
        .resource_mut::<crate::VirtualChunkEnvironmentMap>()
        .set_override(
            [0, 0, 0],
            SparseEnvironmentOverride {
                gravity: Some(Vec3::Y),
                ..Default::default()
            },
        );
    compare_with_full_rebuild(&mut app, &ipc);
    app.world_mut()
        .resource_mut::<crate::VirtualChunkEnvironmentMap>()
        .set_override([0, 0, 0], SparseEnvironmentOverride::default());
    compare_with_full_rebuild(&mut app, &ipc);
    app.world_mut()
        .entity_mut(entity)
        .insert(LocalCoordinate::default());
    compare_with_full_rebuild(&mut app, &ipc);
    assert!(
        app.world()
            .resource::<LocalCoordinateServerWorld>()
            .subscriptions[&ConnectionId(1)]
            .advertised_chunks
            .is_empty(),
        "a same-Entity installation must not retain the removed world's advertised chunks"
    );
    install(&mut app, entity, [0, 0, 0]);
    compare_with_full_rebuild(&mut app, &ipc);
    app.world_mut().entity_mut(entity).despawn();
    compare_with_full_rebuild(&mut app, &ipc);
}
