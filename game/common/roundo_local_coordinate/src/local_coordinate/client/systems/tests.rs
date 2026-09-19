use super::*;
use crate::AtomicVoxel;
use roundo_algorithm::tree::{BreadthFirstLosslessSvo, UnoptimizedOctree};

#[test]
fn decoded_chunks_reject_voxel_ids_missing_from_the_resource_registry() {
    let source = UnoptimizedOctree::new(0, AtomicVoxelId(99));
    let svo = BreadthFirstLosslessSvo::from_unoptimized_mapped(&source, 4, |data| *data).unwrap();

    assert_eq!(
        first_unknown_voxel(&svo, &AtomicVoxelRegistry::builtin()),
        Some(AtomicVoxelId(99))
    );
}

fn add_ingest_systems(app: &mut App) {
    app.init_resource::<ClientChunkViewDistance>().add_systems(
        Update,
        (ingest_commands, collect_derived_svo_results).chain(),
    );
}

#[test]
fn rendering_anchors_are_server_projected_and_removed_by_identity() {
    let transport = CrossbeamThreadPipe::new();
    let server_projection = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<LocalCoordinateClientWorld>()
        .init_resource::<ClientChunkViewDistance>()
        .insert_resource(LocalCoordinateClientPipe(transport.endpoint_b()))
        .add_systems(Update, ingest_commands);
    let anchor = RenderingAnchorState {
        id: ChunkLoadingAnchorId(7),
        owner: roundo_contracts::PlayerId(3),
        scene_id: roundo_contracts::SceneId::S1,
        position: [0.0; 3],
        radius_chunks: 64,
    };

    server_projection
        .try_send(LocalCoordinateClientCommand::RenderingAnchorSpawned(anchor))
        .unwrap();
    app.update();
    assert_eq!(
        app.world()
            .resource::<LocalCoordinateClientWorld>()
            .rendering_anchors()
            .copied()
            .collect::<Vec<_>>(),
        vec![anchor]
    );

    server_projection
        .try_send(LocalCoordinateClientCommand::RenderingAnchorDespawned(
            anchor.id,
        ))
        .unwrap();
    app.update();
    assert_eq!(
        app.world()
            .resource::<LocalCoordinateClientWorld>()
            .rendering_anchors()
            .count(),
        0
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

    let client_world = app.world().resource::<LocalCoordinateClientWorld>();
    let active_version = client_world.active_server_versions.get(&active.id());
    assert_eq!(active_version, Some(&active.version));
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
fn pending_chunk_updates_are_latest_wins_per_chunk() {
    let id = ChunkId {
        local_coordinate_id: LocalCoordinateId(1),
        coordinate: [2, 0, 3],
    };
    let mut world = LocalCoordinateClientWorld::default();
    queue_pending_chunk_update(
        &mut world,
        PendingChunkUpdate::Unload {
            local_coordinate_id: id.local_coordinate_id,
            coordinate: id.coordinate,
        },
    );
    let source = UnoptimizedOctree::new(0, AtomicVoxel::default());
    let svo = Arc::new(
        BreadthFirstLosslessSvo::from_unoptimized_mapped(&source, 4, |data| *data).unwrap(),
    );
    queue_pending_chunk_update(
        &mut world,
        PendingChunkUpdate::Load {
            chunk: ChunkVersion {
                local_coordinate_id: id.local_coordinate_id,
                coordinate: id.coordinate,
                version: UpdateVersion::INITIAL,
            },
            svo,
        },
    );
    assert_eq!(world.pending_chunk_updates.len(), 1);
    assert!(matches!(
        world.pending_chunk_updates.front(),
        Some(PendingChunkUpdate::Load { .. })
    ));
}

#[test]
fn a_new_session_publishes_the_configured_chunk_view_distance() {
    let transport = CrossbeamThreadPipe::new();
    let client = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<LocalCoordinateClientWorld>()
        .insert_resource(ClientChunkViewDistance::new(128))
        .insert_resource(LocalCoordinateClientPipe(transport.endpoint_b()))
        .add_systems(Update, ingest_commands);
    client
        .try_send(LocalCoordinateClientCommand::BeginSession)
        .unwrap();
    app.update();
    assert!(matches!(
        client.try_receive(),
        Some(LocalCoordinateClientEvent::SetChunkViewDistance { chunks: 128 })
    ));
}

#[test]
fn a_new_session_clears_old_active_and_cached_chunks() {
    let transport = CrossbeamThreadPipe::new();
    let client = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<LocalCoordinateClientWorld>()
        .init_resource::<ClientChunkViewDistance>()
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
