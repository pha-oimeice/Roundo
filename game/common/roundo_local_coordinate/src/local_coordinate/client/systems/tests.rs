use super::*;
use roundo_algorithm::tree::{BreadthFirstLosslessSvo, UnoptimizedOctree};

fn add_ingest_systems(app: &mut App) {
    app.add_systems(
        Update,
        (ingest_commands, collect_derived_svo_results).chain(),
    );
}

fn wait_for_cached_chunk(app: &mut App) {
    for _ in 0..100 {
        app.update();
        if app
            .world()
            .resource::<LocalCoordinateClientWorld>()
            .cached_chunk_count()
            > 0
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("derived SVO worker did not decode the chunk");
}

#[test]
fn matching_cached_versions_do_not_request_chunk_data_again() {
    let transport = CrossbeamThreadPipe::new();
    let client = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<LocalCoordinateClientWorld>()
        .insert_resource(LocalCoordinateClientPipe(transport.endpoint_b()));
    add_ingest_systems(&mut app);
    let chunk = ChunkVersion {
        local_coordinate_id: LocalCoordinateId(1),
        coordinate: [2, 3, 4],
        version: UpdateVersion::new(5),
    };

    client
        .try_send(LocalCoordinateClientCommand::VersionUpdates(vec![chunk]))
        .unwrap();
    app.update();
    assert!(matches!(
        client.try_receive(),
        Some(LocalCoordinateClientEvent::RequestChunks(chunks)) if chunks == vec![chunk.id()]
    ));

    let source = UnoptimizedOctree::new(0, crate::EMPTY_VOXEL_ID);
    let svo = BreadthFirstLosslessSvo::from_unoptimized_mapped(&source, 4, |data| *data).unwrap();
    let payload = SerializedPayload::encode(&svo).unwrap();
    client
        .try_send(LocalCoordinateClientCommand::LoadChunk { chunk, payload })
        .unwrap();
    wait_for_cached_chunk(&mut app);
    assert_eq!(
        app.world()
            .resource::<LocalCoordinateClientWorld>()
            .cached_chunk_count(),
        1
    );

    client
        .try_send(LocalCoordinateClientCommand::VersionUpdates(vec![chunk]))
        .unwrap();
    app.update();
    assert!(client.try_receive().is_none());
}

#[test]
fn version_updates_do_not_unload_chunks_omitted_from_the_event() {
    let transport = CrossbeamThreadPipe::new();
    let client = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<LocalCoordinateClientWorld>()
        .insert_resource(LocalCoordinateClientPipe(transport.endpoint_b()));
    add_ingest_systems(&mut app);
    let active = ChunkVersion {
        local_coordinate_id: LocalCoordinateId(1),
        coordinate: [0, 0, 0],
        version: UpdateVersion::new(2),
    };
    let added = ChunkVersion {
        local_coordinate_id: LocalCoordinateId(1),
        coordinate: [1, 0, 0],
        version: UpdateVersion::new(1),
    };
    app.world_mut()
        .resource_mut::<LocalCoordinateClientWorld>()
        .active_server_versions
        .insert(active.id(), active.version);

    client
        .try_send(LocalCoordinateClientCommand::VersionUpdates(vec![added]))
        .unwrap();
    app.update();

    assert_eq!(
        app.world()
            .resource::<LocalCoordinateClientWorld>()
            .active_server_versions
            .get(&active.id()),
        Some(&active.version)
    );
}

#[test]
fn unloading_a_chunk_keeps_its_transferred_data_cached() {
    let transport = CrossbeamThreadPipe::new();
    let client = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<LocalCoordinateClientWorld>()
        .insert_resource(LocalCoordinateClientPipe(transport.endpoint_b()));
    add_ingest_systems(&mut app);
    let chunk = ChunkVersion {
        local_coordinate_id: LocalCoordinateId(1),
        coordinate: [2, 3, 4],
        version: UpdateVersion::new(5),
    };

    client
        .try_send(LocalCoordinateClientCommand::VersionUpdates(vec![chunk]))
        .unwrap();
    app.update();
    let _ = client.try_receive();
    let source = UnoptimizedOctree::new(0, crate::EMPTY_VOXEL_ID);
    let svo = BreadthFirstLosslessSvo::from_unoptimized_mapped(&source, 4, |data| *data).unwrap();
    let payload = SerializedPayload::encode(&svo).unwrap();
    client
        .try_send(LocalCoordinateClientCommand::LoadChunk { chunk, payload })
        .unwrap();
    wait_for_cached_chunk(&mut app);

    client
        .try_send(LocalCoordinateClientCommand::UnloadChunk {
            local_coordinate_id: chunk.local_coordinate_id,
            coordinate: chunk.coordinate,
        })
        .unwrap();
    app.update();

    let world = app.world().resource::<LocalCoordinateClientWorld>();
    assert_eq!(world.cached_chunk_count(), 1);
    assert!(!world.active_server_versions.contains_key(&chunk.id()));
}

#[test]
fn a_new_session_clears_old_active_and_cached_chunks() {
    let transport = CrossbeamThreadPipe::new();
    let client = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<LocalCoordinateClientWorld>()
        .insert_resource(LocalCoordinateClientPipe(transport.endpoint_b()))
        .add_systems(Update, ingest_commands);
    let chunk = ChunkVersion {
        local_coordinate_id: LocalCoordinateId(1),
        coordinate: [0, 0, 0],
        version: UpdateVersion::new(3),
    };
    let source = UnoptimizedOctree::new(0, crate::EMPTY_VOXEL_ID);
    let svo = Arc::new(
        BreadthFirstLosslessSvo::from_unoptimized_mapped(&source, 4, |data| *data).unwrap(),
    );
    {
        let mut world = app.world_mut().resource_mut::<LocalCoordinateClientWorld>();
        world.cached_chunks.insert(
            chunk.id(),
            CachedClientChunk {
                server_version: chunk.version,
                svo,
            },
        );
        world
            .active_server_versions
            .insert(chunk.id(), chunk.version);
    }

    client
        .try_send(LocalCoordinateClientCommand::BeginSession)
        .unwrap();
    app.update();

    let world = app.world().resource::<LocalCoordinateClientWorld>();
    assert!(world.cached_chunks.is_empty());
    assert!(world.active_server_versions.is_empty());
}
