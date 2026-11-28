//! Incremental extraction, asynchronous SVO handoff and model invalidation.

use super::*;

pub(super) fn extract_loaded_chunks(
    mut extracted: ResMut<ExtractedChunks>,
    chunks: Extract<
        Query<(
            &LocalCoordinateIdentity,
            Ref<LocalCoordinate>,
            Ref<GlobalTransform>,
            Option<&PcgRenderView>,
        )>,
    >,
) {
    extracted.epoch = extracted.epoch.wrapping_add(1);
    extracted.changes.clear();
    let mut observed_local_coordinates = std::collections::HashSet::new();

    for (identity, local_coordinate, global_transform, view) in &chunks {
        let local_coordinate_id = identity.0;
        let view = view.map(|view| view.for_installation(&local_coordinate, local_coordinate_id));
        observed_local_coordinates.insert(local_coordinate_id);
        let transform_changed = global_transform.is_changed();
        if transform_changed {
            extracted.changes.push(ExtractedChunkChange::Transform {
                local_coordinate_id,
                world_from_local: global_transform.to_matrix().to_cols_array(),
            });
        }
        if !local_coordinate.is_changed()
            && extracted
                .tracked
                .get(&local_coordinate_id)
                .is_some_and(|tracked| tracked.view == view)
        {
            continue;
        }

        let previous = extracted
            .tracked
            .remove(&local_coordinate_id)
            .unwrap_or_default()
            .revisions;
        let mut current = HashMap::new();
        for chunk in local_coordinate.loaded_chunk_views() {
            let coordinate = [
                i64::from(chunk.position.x),
                i64::from(chunk.position.y),
                i64::from(chunk.position.z),
            ];
            if view.is_some_and(|view| !view.contains(coordinate)) {
                continue;
            }
            current.insert(coordinate, chunk.revision);
            if previous.get(&coordinate) == Some(&chunk.revision) && !transform_changed {
                continue;
            }
            let chunk_translation = bevy::math::Affine3A::from_translation(
                (chunk.position * CHUNK_EDGE_LENGTH as i32).as_vec3(),
            );
            let world_from_chunk = global_transform.affine() * chunk_translation;
            extracted
                .changes
                .push(ExtractedChunkChange::Upsert(ExtractedChunk {
                    id: ChunkId {
                        local_coordinate_id,
                        coordinate,
                    },
                    revision: chunk.revision,
                    svo: Arc::clone(chunk.svo),
                    world_from_chunk: Mat4::from(world_from_chunk).to_cols_array(),
                }));
        }
        for coordinate in previous.keys() {
            if !current.contains_key(coordinate) {
                extracted
                    .changes
                    .push(ExtractedChunkChange::Remove(ChunkId {
                        local_coordinate_id,
                        coordinate: *coordinate,
                    }));
            }
        }
        extracted.tracked.insert(
            local_coordinate_id,
            TrackedChunks {
                view,
                revisions: current,
            },
        );
    }

    let removed_local_coordinates = extracted
        .tracked
        .keys()
        .filter(|id| !observed_local_coordinates.contains(id))
        .copied()
        .collect::<Vec<_>>();
    for local_coordinate_id in removed_local_coordinates {
        if let Some(chunks) = extracted.tracked.remove(&local_coordinate_id) {
            extracted
                .changes
                .extend(chunks.revisions.into_keys().map(|coordinate| {
                    ExtractedChunkChange::Remove(ChunkId {
                        local_coordinate_id,
                        coordinate,
                    })
                }));
        }
    }
}

pub(super) fn prepare_residency(
    extracted: Res<ExtractedChunks>,
    settings: Res<WorldRenderSettings>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<WorldPipelines>,
    mut residency: ResMut<WorldResidency>,
) {
    collect_gpu_status(&mut residency);
    residency
        .geometry_arena
        .get_or_insert_with(|| GeometryArena::new(&render_device, settings.geometry_budget_bytes));
    residency.svo_uploads.get_or_insert_with(|| {
        SvoUploads::spawn(
            (*render_device).clone(),
            (*render_queue).clone(),
            DEFAULT_SVO_BUDGET_BYTES,
        )
    });
    residency
        .gpu_culling
        .get_or_insert_with(|| create_gpu_culling(&render_device, &pipeline_cache, &pipelines));
    for change in &extracted.changes {
        let ExtractedChunkChange::Transform {
            local_coordinate_id,
            world_from_local,
        } = change
        else {
            continue;
        };
        let cull_descriptors = residency.gpu_culling.as_ref().unwrap().descriptors.clone();
        for (id, resident) in &mut residency.chunks {
            if id.local_coordinate_id != *local_coordinate_id {
                continue;
            }
            let updated = transformed_chunk_matrix(*world_from_local, id.coordinate);
            if resident.world_from_chunk != updated {
                resident.world_from_chunk = updated;
                write_chunk_transform(&render_queue, &cull_descriptors, resident);
            }
        }
        for (id, pending) in &mut residency.pending_svos {
            if id.local_coordinate_id == *local_coordinate_id {
                pending.observed.world_from_chunk =
                    transformed_chunk_matrix(*world_from_local, id.coordinate);
            }
        }
    }

    for change in &extracted.changes {
        let ExtractedChunkChange::Remove(id) = change else {
            continue;
        };
        residency.pending_svos.remove(id);
        residency.svo_uploads.as_ref().unwrap().cancel(*id);
        mark_chunk_and_neighbors_dirty(&mut residency.geometry_dirty, *id);
        if let Some(previous) = residency.chunks.remove(id) {
            residency.free_gpu_slots.push(previous.gpu_slot);
            clear_gpu_draw(
                &render_queue,
                residency.gpu_culling.as_ref().unwrap(),
                previous.gpu_slot,
            );
            // All previous-frame uses have been submitted before PrepareResources.
            residency.svo_uploads.as_ref().unwrap().retire(previous.svo);
        }
    }

    for change in &extracted.changes {
        let ExtractedChunkChange::Upsert(observed) = change else {
            continue;
        };
        if let Some(pending) = residency.pending_svos.get_mut(&observed.id)
            && pending.observed.revision == observed.revision
        {
            pending.observed.world_from_chunk = observed.world_from_chunk;
            continue;
        }
        let resident = residency.chunks.get(&observed.id);
        let update = resident_update(
            resident.map(|resident| (resident.revision, resident.world_from_chunk)),
            observed.revision,
            observed.world_from_chunk,
        );
        if update != ResidentUpdate::Replace {
            if residency.pending_svos.remove(&observed.id).is_some() {
                residency.svo_uploads.as_ref().unwrap().cancel(observed.id);
            }
            if update == ResidentUpdate::Transform {
                let descriptors = residency.gpu_culling.as_ref().unwrap().descriptors.clone();
                let resident = residency.chunks.get_mut(&observed.id).unwrap();
                resident.world_from_chunk = observed.world_from_chunk;
                write_chunk_transform(&render_queue, &descriptors, resident);
            }
            continue;
        }
        let request = residency
            .svo_uploads
            .as_ref()
            .unwrap()
            .request(observed.id, Arc::clone(&observed.svo));
        residency.pending_svos.insert(
            observed.id,
            PendingSvo {
                request,
                observed: observed.clone(),
            },
        );
    }

    let (buffers, ready) = residency.svo_uploads.as_ref().unwrap().take_ready();
    residency.svo_buffers = buffers;
    for ready in ready {
        if residency
            .pending_svos
            .get(&ready.id)
            .map(|pending| pending.request)
            != Some(ready.request)
        {
            residency
                .svo_uploads
                .as_ref()
                .unwrap()
                .retire(ready.allocation);
            continue;
        }
        let pending = residency.pending_svos.remove(&ready.id).unwrap();
        let observed = &pending.observed;
        let reused_gpu_slot = residency.chunks.contains_key(&observed.id);
        let gpu_slot = if let Some(resident) = residency.chunks.get(&observed.id) {
            resident.gpu_slot
        } else if let Some(slot) = residency.free_gpu_slots.pop() {
            slot
        } else {
            let slot = residency.next_gpu_slot;
            assert!(
                slot < MAX_GPU_CHUNK_DESCRIPTORS,
                "world GPU Chunk descriptor capacity exhausted"
            );
            residency.next_gpu_slot += 1;
            slot
        };
        let incarnation = if let Some(previous) = residency.chunks.get(&observed.id) {
            previous.incarnation
        } else {
            residency.next_incarnation = residency
                .next_incarnation
                .checked_add(1)
                .expect("GPU Chunk incarnation exhausted");
            residency.next_incarnation
        };
        let replacement = resident_chunk(
            observed,
            settings.material_seed,
            gpu_slot,
            incarnation,
            ready.allocation,
        );
        if let Some(previous) = residency.chunks.remove(&observed.id) {
            // Geometry lives in GPU model state, not this CPU snapshot. Retaining
            // the incarnation preserves the last committed model until GPU commit.
            residency.svo_uploads.as_ref().unwrap().retire(previous.svo);
        }
        if !reused_gpu_slot {
            clear_gpu_draw(
                &render_queue,
                residency.gpu_culling.as_ref().unwrap(),
                replacement.gpu_slot,
            );
        }
        residency.chunks.insert(observed.id, replacement);
        mark_chunk_and_neighbors_dirty(&mut residency.geometry_dirty, observed.id);
    }

    // Geometry retirement is GPU-owned. Its reclaim pass runs after prior queue
    // submissions and before any current-frame draw can reference a recycled page.
}

fn resident_chunk(
    observed: &ExtractedChunk,
    material_seed: u32,
    gpu_slot: u32,
    incarnation: u32,
    svo: SvoAllocation,
) -> ResidentChunk {
    ResidentChunk {
        revision: observed.revision,
        gpu_slot,
        material_seed,
        coordinate: observed.id.coordinate,
        svo,
        generation: 0,
        incarnation,
        world_from_chunk: observed.world_from_chunk,
    }
}

fn transformed_chunk_matrix(world_from_local: [f32; 16], coordinate: [i64; 3]) -> [f32; 16] {
    let translation = Mat4::from_translation(bevy::math::Vec3::new(
        coordinate[0] as f32 * CHUNK_EDGE as f32,
        coordinate[1] as f32 * CHUNK_EDGE as f32,
        coordinate[2] as f32 * CHUNK_EDGE as f32,
    ));
    (Mat4::from_cols_array(&world_from_local) * translation).to_cols_array()
}

pub(super) fn world_matrix_bytes(matrix: [f32; 16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(64);
    for value in matrix {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

pub(super) const CHUNK_NEIGHBOR_OFFSETS: [[i64; 3]; 6] = [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
];

pub(super) fn neighbor_id(id: ChunkId, offset: [i64; 3]) -> Option<ChunkId> {
    Some(ChunkId {
        local_coordinate_id: id.local_coordinate_id,
        coordinate: [
            id.coordinate[0].checked_add(offset[0])?,
            id.coordinate[1].checked_add(offset[1])?,
            id.coordinate[2].checked_add(offset[2])?,
        ],
    })
}

fn mark_chunk_and_neighbors_dirty(dirty: &mut HashSet<ChunkId>, id: ChunkId) {
    dirty.insert(id);
    for offset in CHUNK_NEIGHBOR_OFFSETS {
        if let Some(neighbor) = neighbor_id(id, offset) {
            dirty.insert(neighbor);
        }
    }
}

pub(super) fn geometry_build_dispatch(build_count: u32) -> (u32, u32, u32) {
    (
        MESH_WORKGROUPS,
        // Every request, including count-only work, uses the same padded grid.
        build_count.min(64),
        build_count.div_ceil(64),
    )
}
