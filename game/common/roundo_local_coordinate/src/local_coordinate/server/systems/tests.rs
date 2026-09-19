use super::*;
use crate::VoxelChunkSvo;
use crate::local_coordinate::data::{Chunk, PositionedAtomicVoxel, SOLID_VOXEL_ID};
use bevy::prelude::{FixedUpdate, IVec3, Schedule, Update, Vec3, World};

fn test_anchor(player_id: PlayerId) -> RenderingAnchorState {
    RenderingAnchorState {
        id: ChunkLoadingAnchorId(1),
        owner: player_id,
        scene_id: SceneId::S1,
        position: [0.0; 3],
        radius_chunks: DEFAULT_CHUNK_VIEW_DISTANCE,
    }
}

#[test]
fn core_fixed_tick_does_not_run_world_streaming() {
    let plugin = LocalCoordinateServerPlugin::new();
    let commands = plugin.ipc();
    let connection_id = ConnectionId(99);
    let player_id = PlayerId(99);
    let mut app = App::new();
    app.add_plugins(plugin);
    app.world_mut().run_schedule(Startup);
    commands
        .try_send(LocalCoordinateServerCommand::SubscribePlayer {
            connection_id,
            player_id,
        })
        .unwrap();

    app.world_mut().run_schedule(FixedUpdate);
    assert!(
        app.world()
            .resource::<LocalCoordinateServerWorld>()
            .subscriptions
            .is_empty(),
        "world streaming ran inside the core FixedUpdate schedule"
    );

    app.world_mut().run_schedule(Update);
    assert!(
        app.world()
            .resource::<LocalCoordinateServerWorld>()
            .subscriptions
            .contains_key(&connection_id)
    );
}

#[test]
fn rendering_anchor_starts_at_origin_and_does_not_depend_on_player_transform() {
    let plugin = LocalCoordinateServerPlugin::new();
    let transport = plugin.ipc();
    let connection_id = ConnectionId(5);
    let player_id = PlayerId(8);
    let mut app = App::new();
    app.add_plugins(plugin);
    app.world_mut().run_schedule(Startup);
    transport
        .try_send(LocalCoordinateServerCommand::SubscribePlayer {
            connection_id,
            player_id,
        })
        .unwrap();
    app.world_mut().run_schedule(Update);

    let subscription = &app
        .world()
        .resource::<LocalCoordinateServerWorld>()
        .subscriptions[&connection_id];
    assert_eq!(subscription.anchor.position, [0.0; 3]);
    assert_eq!(subscription.anchor.owner, player_id);
    assert_eq!(
        absolute_chunk_coordinate(subscription.anchor.position),
        [0, 0, 0]
    );
    let demand = app
        .world()
        .resource::<LocalCoordinateServerWorld>()
        .observation_by_anchor[&subscription.anchor.id];
    assert_eq!(demand.center, [0.0; 3]);
    assert_eq!(demand.streaming_center, [0.0; 3]);
    assert!(matches!(
        transport.try_receive(),
        Some(LocalCoordinateServerEvent::RenderingAnchorSpawned { anchor, .. })
            if anchor == subscription.anchor
    ));
}

#[test]
fn chunk_view_distance_is_clamped_and_retained_before_subscription() {
    let transport = CrossbeamThreadPipe::new();
    let commands = transport.endpoint_a();
    let connection_id = ConnectionId(11);
    let mut app = App::new();
    app.insert_resource(LocalCoordinateServerPipe(transport.endpoint_b()))
        .init_resource::<LocalCoordinateServerWorld>()
        .init_resource::<LocalCoordinatePhysicsInterests>()
        .add_systems(Update, prepare_player_chunks);
    commands
        .try_send(LocalCoordinateServerCommand::SetChunkViewDistance {
            connection_id,
            chunks: u16::MAX,
        })
        .unwrap();
    app.update();
    assert_eq!(
        app.world()
            .resource::<LocalCoordinateServerWorld>()
            .requested_view_distances[&connection_id],
        MAX_CHUNK_VIEW_DISTANCE
    );
}

#[test]
fn streaming_crosses_the_coordinate_origin_without_wrapping() {
    let chunks = superflat_chunks_intersecting_radius([0.0, 2.0, 0.0], CHUNK_EDGE_LENGTH as f64, 0);

    assert!(chunks.contains(&[0, 0, 0]));
    assert!(chunks.contains(&[-1, 0, 0]));
    assert!(chunks.contains(&[0, 0, -1]));
}

#[test]
fn moving_anchor_does_not_accumulate_pristine_generated_chunks() {
    let transport = CrossbeamThreadPipe::new();
    let commands = transport.endpoint_a();
    let connection_id = ConnectionId(12);
    let player_id = PlayerId(12);
    let mut app = App::new();
    app.insert_resource(LocalCoordinateServerPipe(transport.endpoint_b()))
        .init_resource::<LocalCoordinateServerWorld>()
        .init_resource::<LocalCoordinatePhysicsInterests>()
        .add_systems(
            Update,
            (
                prepare_player_chunks,
                commit_generated_chunks,
                update_authoritative_chunk_versions,
            )
                .chain(),
        );
    app.world_mut().spawn((
        PcgLocalCoordinate::new(DEFAULT_PCG_LOCAL_COORDINATE_ID, SuperflatGenerator::new(0)),
        GlobalTransform::default(),
        LocalCoordinate::default(),
    ));
    commands
        .try_send(LocalCoordinateServerCommand::SetChunkViewDistance {
            connection_id,
            chunks: 1,
        })
        .unwrap();
    commands
        .try_send(LocalCoordinateServerCommand::SubscribePlayer {
            connection_id,
            player_id,
        })
        .unwrap();
    for _ in 0..100 {
        app.update();
        if app
            .world()
            .resource::<LocalCoordinateServerWorld>()
            .loaded_chunk_count(DEFAULT_PCG_LOCAL_COORDINATE_ID)
            >= 4
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let initial_count = app
        .world()
        .resource::<LocalCoordinateServerWorld>()
        .loaded_chunks[&DEFAULT_PCG_LOCAL_COORDINATE_ID]
        .len();

    app.world_mut()
        .resource_mut::<LocalCoordinateServerWorld>()
        .subscriptions
        .get_mut(&connection_id)
        .unwrap()
        .anchor
        .position = [160.0, 0.0, 0.0];
    for _ in 0..100 {
        app.update();
        let server_world = app.world().resource::<LocalCoordinateServerWorld>();
        let chunk_sets = &server_world.loaded_chunks;
        let loaded_chunks = chunk_sets.get(&DEFAULT_PCG_LOCAL_COORDINATE_ID);
        if loaded_chunks.is_some_and(|chunks| chunks.contains_key(&[10, 0, 0])) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let loaded = &app
        .world()
        .resource::<LocalCoordinateServerWorld>()
        .loaded_chunks[&DEFAULT_PCG_LOCAL_COORDINATE_ID];

    assert!(
        loaded.contains_key(&[10, 0, 0]),
        "new observation was not generated"
    );
    assert!(
        !loaded.contains_key(&[0, 0, 0]),
        "old pristine chunk survived eviction"
    );
    assert!(
        loaded.len() <= initial_count,
        "pristine generated chunks accumulated from {initial_count} to {}",
        loaded.len()
    );
}

#[test]
fn generated_coordinates_are_static_identity_local_coordinates() {
    let mut world = World::new();
    world.insert_resource(GeneratedLocalCoordinates(vec![
        PcgLocalCoordinate::new(DEFAULT_PCG_LOCAL_COORDINATE_ID, SuperflatGenerator::new(1)),
        PcgLocalCoordinate::new(LocalCoordinateId(2), SuperflatGenerator::new(2)),
    ]));
    let mut schedule = Schedule::default();
    schedule.add_systems(spawn_generated_local_coordinates);
    schedule.run(&mut world);

    let mut query = world.query::<(
        &PcgLocalCoordinate,
        &LocalCoordinate,
        &RigidBody,
        &Transform,
    )>();
    let coordinates = query.iter(&world).collect::<Vec<_>>();
    assert_eq!(coordinates.len(), 2);
    for (generated, _, rigid_body, transform) in coordinates {
        assert_eq!(generated.scene_id, SceneId::S1);
        assert_eq!(*rigid_body, RigidBody::Static);
        assert_eq!(transform.translation, Vec3::ZERO);
        assert_eq!(transform.rotation, Default::default());
        assert_eq!(transform.scale, Vec3::ONE);
    }
}

#[test]
fn only_advertised_chunk_ids_enter_the_response_queue_once() {
    let key = ChunkId {
        local_coordinate_id: LocalCoordinateId(1),
        coordinate: [2, 3, 4],
    };
    let current = UpdateVersion::new(7);
    let mut subscription = PlayerSubscription {
        anchor: test_anchor(PlayerId(1)),
        spawned_coordinates: HashSet::new(),
        advertised_chunks: HashMap::from([(key, current)]),
        pending_requests: VecDeque::new(),
    };
    let unavailable = ChunkId {
        local_coordinate_id: LocalCoordinateId(1),
        coordinate: [8, 9, 10],
    };

    enqueue_chunk_requests(&mut subscription, [key, key, unavailable]);

    assert_eq!(subscription.pending_requests.len(), 1);
    assert_eq!(subscription.pending_requests[0], key);
}

#[test]
fn version_events_include_only_added_or_changed_chunks() {
    let unchanged = ChunkId {
        local_coordinate_id: LocalCoordinateId(1),
        coordinate: [0, 0, 0],
    };
    let changed = ChunkId {
        local_coordinate_id: LocalCoordinateId(1),
        coordinate: [1, 0, 0],
    };
    let removed = ChunkId {
        local_coordinate_id: LocalCoordinateId(1),
        coordinate: [2, 0, 0],
    };
    let added = ChunkId {
        local_coordinate_id: LocalCoordinateId(1),
        coordinate: [3, 0, 0],
    };
    let advertised = HashMap::from([
        (unchanged, UpdateVersion::new(4)),
        (changed, UpdateVersion::new(4)),
        (removed, UpdateVersion::new(4)),
    ]);
    let current = HashMap::from([
        (unchanged, UpdateVersion::new(4)),
        (changed, UpdateVersion::new(5)),
        (added, UpdateVersion::INITIAL),
    ]);

    let updates = changed_chunk_versions(&advertised, &current);

    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].id(), changed);
    assert_eq!(updates[0].version, UpdateVersion::new(5));
    assert_eq!(updates[1].id(), added);
}

#[test]
fn subscription_snapshot_drops_chunks_outside_the_latest_observation() {
    let removed = ChunkId {
        local_coordinate_id: LocalCoordinateId(1),
        coordinate: [0, 0, 0],
    };
    let added = ChunkId {
        local_coordinate_id: LocalCoordinateId(1),
        coordinate: [8, 0, 0],
    };
    let mut subscription = PlayerSubscription {
        anchor: test_anchor(PlayerId(1)),
        spawned_coordinates: HashSet::from([LocalCoordinateId(1)]),
        advertised_chunks: HashMap::from([(removed, UpdateVersion::new(4))]),
        pending_requests: VecDeque::from([removed]),
    };

    update_subscription_snapshot(
        &mut subscription,
        HashSet::from([LocalCoordinateId(1)]),
        HashMap::from([(added, UpdateVersion::new(2))]),
    );

    assert!(!subscription.advertised_chunks.contains_key(&removed));
    let advertised = subscription.advertised_chunks.get(&added);
    assert_eq!(advertised, Some(&UpdateVersion::new(2)));
    assert_eq!(subscription.pending_requests, VecDeque::from([added]));
}

#[test]
fn stale_or_unsubscribed_svo_jobs_are_not_published() {
    let connection_id = ConnectionId(9);
    let chunk = ChunkVersion {
        local_coordinate_id: LocalCoordinateId(1),
        coordinate: [2, 0, 0],
        version: UpdateVersion::new(3),
    };
    let mut world = LocalCoordinateServerWorld::default();
    world.subscriptions.insert(
        connection_id,
        PlayerSubscription {
            anchor: test_anchor(PlayerId(1)),
            spawned_coordinates: HashSet::new(),
            advertised_chunks: HashMap::from([(chunk.id(), chunk.version)]),
            pending_requests: VecDeque::new(),
        },
    );
    world
        .derived_svo_jobs
        .insert((connection_id, chunk.id()), chunk.version);
    assert!(complete_svo_job(&mut world, connection_id, chunk));

    world
        .derived_svo_jobs
        .insert((connection_id, chunk.id()), chunk.version);
    world
        .subscriptions
        .get_mut(&connection_id)
        .unwrap()
        .advertised_chunks
        .clear();
    assert!(!complete_svo_job(&mut world, connection_id, chunk));
}

#[test]
fn derived_chunk_responses_keep_the_requested_server_version() {
    let requested = ChunkId {
        local_coordinate_id: LocalCoordinateId(1),
        coordinate: [2, 3, 4],
    };
    let mut chunk = Chunk::default();
    assert!(chunk.set_voxel(IVec3::ZERO, SOLID_VOXEL_ID));
    assert!(chunk.set_voxel(IVec3::X, SOLID_VOXEL_ID));
    let server_version = UpdateVersion::new(7);

    let worker = DerivedSvoWorker::spawn("roundo-test-derived-svo");
    let response = ChunkVersion {
        local_coordinate_id: requested.local_coordinate_id,
        coordinate: requested.coordinate,
        version: server_version,
    };
    assert!(
        worker
            .submit(DerivedSvoJob::Encode {
                connection_id: ConnectionId(3),
                chunk: response,
                source: chunk.svo_source(),
            })
            .is_ok()
    );
    for _ in 0..100 {
        if let Some(DerivedSvoResult::Encoded { chunk, payload, .. }) = worker.try_receive() {
            let decoded: Result<VoxelChunkSvo, _> = payload.decode();
            decoded.unwrap();
            assert_eq!(chunk.id(), requested);
            assert_eq!(chunk.version, server_version);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("derived SVO worker did not encode the chunk");
}

#[test]
fn server_advances_each_changed_chunk_version_once_per_update() {
    let generated = PcgLocalCoordinate::new(LocalCoordinateId(1), SuperflatGenerator::new(0));
    let mut coordinate = LocalCoordinate::default();
    assert!(coordinate.apply_voxel(PositionedAtomicVoxel {
        position: IVec3::ZERO,
        voxel: SOLID_VOXEL_ID,
    }));
    assert!(coordinate.apply_voxel(PositionedAtomicVoxel {
        position: IVec3::X,
        voxel: SOLID_VOXEL_ID,
    }));
    let chunk_id = ChunkId {
        local_coordinate_id: generated.id,
        coordinate: [0, 0, 0],
    };
    let mut app = App::new();
    app.init_resource::<LocalCoordinateServerWorld>()
        .add_systems(Update, update_authoritative_chunk_versions);
    let entity = app.world_mut().spawn((generated, coordinate)).id();

    app.update();
    let server_world = app.world().resource::<LocalCoordinateServerWorld>();
    let version = server_world.chunk_versions.get(&chunk_id);
    assert_eq!(version, Some(&UpdateVersion::INITIAL));

    assert!(
        app.world_mut()
            .get_mut::<LocalCoordinate>(entity)
            .unwrap()
            .apply_voxel(PositionedAtomicVoxel {
                position: IVec3::Y,
                voxel: SOLID_VOXEL_ID,
            })
    );
    app.update();
    let server_world = app.world().resource::<LocalCoordinateServerWorld>();
    let version = server_world.chunk_versions.get(&chunk_id);
    assert_eq!(version, Some(&UpdateVersion::new(1)));
}
