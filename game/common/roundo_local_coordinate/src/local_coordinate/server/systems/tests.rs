use super::*;
use crate::VoxelChunkSvo;
use crate::local_coordinate::data::{Chunk, PositionedAtomicVoxel, SOLID_VOXEL_ID};
use bevy::prelude::{IVec3, Schedule, Update, Vec3, World};
use roundo_presence::DEFAULT_S0_ROOM_SIZE;

#[test]
fn player_chunk_generation_stays_inside_scene_bounds() {
    let transport = CrossbeamThreadPipe::new();
    let commands = transport.endpoint_a();
    let connection_id = ConnectionId(1);
    let player_id = PlayerId(1);
    let scene_id = SceneId::S0 { room_id: 7 };
    let mut scenes = ServerSceneWorlds::default();
    assert!(scenes.resize_s0_room(7, DEFAULT_S0_ROOM_SIZE));

    let mut app = App::new();
    app.insert_resource(LocalCoordinateServerPipe(transport.endpoint_b()))
        .insert_resource(scenes)
        .init_resource::<LocalCoordinateServerWorld>()
        .add_systems(Update, prepare_player_chunks);
    app.world_mut().spawn((
        Player { id: player_id },
        PlayerScene { scene_id },
        ServerPlayer,
        GlobalTransform::default(),
    ));
    app.world_mut().spawn((
        PcgLocalCoordinate::new(DEFAULT_PCG_LOCAL_COORDINATE_ID, SuperflatGenerator::new(0))
            .in_scene(scene_id),
        GlobalTransform::default(),
        LocalCoordinate::default(),
    ));
    commands
        .try_send(LocalCoordinateServerCommand::SubscribePlayer {
            connection_id,
            player_id,
        })
        .unwrap();

    app.update();

    let world = app.world().resource::<LocalCoordinateServerWorld>();
    let chunks = &world.loaded_chunks[&DEFAULT_PCG_LOCAL_COORDINATE_ID];
    let chunks_per_axis = (DEFAULT_S0_ROOM_SIZE[0] / CHUNK_EDGE_LENGTH as f32) as i64;
    assert_eq!(chunks.len(), chunks_per_axis.pow(3) as usize);
    assert!(chunks.keys().all(|coordinate| {
        coordinate
            .iter()
            .all(|value| (0..chunks_per_axis).contains(value))
    }));
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
        player_id: PlayerId(1),
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
fn subscription_snapshots_retain_chunks_outside_the_latest_observation() {
    let retained = ChunkId {
        local_coordinate_id: LocalCoordinateId(1),
        coordinate: [0, 0, 0],
    };
    let added = ChunkId {
        local_coordinate_id: LocalCoordinateId(1),
        coordinate: [8, 0, 0],
    };
    let mut subscription = PlayerSubscription {
        player_id: PlayerId(1),
        spawned_coordinates: HashSet::from([LocalCoordinateId(1)]),
        advertised_chunks: HashMap::from([(retained, UpdateVersion::new(4))]),
        pending_requests: VecDeque::new(),
    };

    retain_subscription_snapshot(
        &mut subscription,
        HashSet::from([LocalCoordinateId(1)]),
        HashMap::from([(added, UpdateVersion::new(2))]),
    );

    assert_eq!(
        subscription.advertised_chunks.get(&retained),
        Some(&UpdateVersion::new(4))
    );
    assert_eq!(
        subscription.advertised_chunks.get(&added),
        Some(&UpdateVersion::new(2))
    );
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
    worker.submit(DerivedSvoJob::Encode {
        connection_id: ConnectionId(3),
        chunk: response,
        source: chunk.svo_source(),
    });
    for _ in 0..100 {
        if let Some(DerivedSvoResult::Encoded { chunk, payload, .. }) = worker.try_receive() {
            let _: VoxelChunkSvo = payload.decode().unwrap();
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
    assert_eq!(
        app.world()
            .resource::<LocalCoordinateServerWorld>()
            .chunk_versions
            .get(&chunk_id),
        Some(&UpdateVersion::INITIAL)
    );

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
    assert_eq!(
        app.world()
            .resource::<LocalCoordinateServerWorld>()
            .chunk_versions
            .get(&chunk_id),
        Some(&UpdateVersion::new(1))
    );
}
