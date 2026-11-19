//! GPU count -> view selection -> paged allocation -> generation/commit.
//! The host supplies immutable inputs and views, never model sizes or LOD choices.

use super::*;

pub(super) fn init_world_pipelines(
    mut commands: Commands,
    asset_server: Res<bevy::prelude::AssetServer>,
    cache: Res<PipelineCache>,
) {
    let shaders = WorldShaders::load(asset_server.as_ref());
    let compute_layout = BindGroupLayoutDescriptor::new(
        "world model generation",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                storage_buffer_read_only::<GpuPackedSvoNode>(false),
                storage_buffer::<GpuGeneratedQuad>(false),
                storage_buffer::<GpuCullDescriptor>(false),
                storage_buffer::<UVec4>(false),
                storage_buffer::<u32>(false),
                uniform_buffer::<GpuBuildPass>(false),
                storage_buffer::<GpuBuildRequest>(false),
                storage_buffer::<GpuModelState>(false),
                storage_buffer::<GpuPagePoolBinding>(false),
            ),
        ),
    );
    let view_layout = BindGroupLayoutDescriptor::new(
        "world view",
        &BindGroupLayoutEntries::single(
            ShaderStages::COMPUTE | ShaderStages::VERTEX,
            uniform_buffer::<ViewUniform>(true),
        ),
    );
    let render_layout = BindGroupLayoutDescriptor::new(
        "world geometry",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::VERTEX | ShaderStages::FRAGMENT,
            (
                storage_buffer_read_only::<GpuGeneratedQuad>(false),
                storage_buffer_read_only::<GpuCullDescriptor>(false),
            ),
        ),
    );
    let cull_layout = BindGroupLayoutDescriptor::new(
        "world layered culling",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                storage_buffer_read_only::<GpuCullDescriptor>(false),
                storage_buffer::<GpuDrawIndirect>(false),
                storage_buffer::<u32>(false),
                uniform_buffer::<UVec4>(false),
            ),
        ),
    );
    let shader = shaders.greedy_emit.clone();
    let build = |entry: &'static str| {
        cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(Cow::Borrowed(entry)),
            layout: vec![compute_layout.clone()],
            shader: shader.clone(),
            entry_point: Some(Cow::Borrowed(entry)),
            ..Default::default()
        })
    };
    let select_layout = BindGroupLayoutDescriptor::new(
        "world GPU model selection and paging",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                storage_buffer::<GpuCullDescriptor>(false),
                storage_buffer::<UVec4>(false),
                storage_buffer::<GpuModelState>(false),
                storage_buffer::<GpuBuildRequest>(false),
                storage_buffer::<GpuPagePoolBinding>(false),
                storage_buffer_read_only::<GpuModelView>(false),
                uniform_buffer::<UVec4>(false),
                storage_buffer::<u32>(false),
            ),
        ),
    );
    let select_shader = shaders.model_select.clone();
    let select = |entry: &'static str| {
        cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(Cow::Borrowed(entry)),
            layout: vec![select_layout.clone()],
            shader: select_shader.clone(),
            entry_point: Some(Cow::Borrowed(entry)),
            ..Default::default()
        })
    };
    let surface_proxy_layout = BindGroupLayoutDescriptor::new(
        "world QEF reference reduction",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                storage_buffer::<GpuQefSummary>(false),
                uniform_buffer::<GpuQefReducePass>(false),
            ),
        ),
    );
    let surface_proxy_reduce = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some(Cow::Borrowed("world QEF reference reduction")),
        layout: vec![surface_proxy_layout.clone()],
        shader: shaders.surface_proxy.clone(),
        entry_point: Some(Cow::Borrowed("reduce_qef_depth")),
        ..Default::default()
    });
    commands.insert_resource(WorldPipelines {
        prepare_boundary: build("prepare_boundary"),
        reset_geometry: build("reset_geometry"),
        count_models: build("count_models"),
        gather_counts: build("gather_counts"),
        compute: build("mesh_chunks"),
        finalize_builds: build("finalize_builds"),
        reclaim_models: select("reclaim_models"),
        reclassify_slabs: select("reclassify_slabs"),
        select_models: select("select_models"),
        finalize_dispatch: select("finalize_dispatch"),
        select_layout,
        cull: cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(Cow::Borrowed("world layered frustum culling")),
            layout: vec![view_layout.clone(), cull_layout.clone()],
            shader: shaders.cull_lod.clone(),
            entry_point: Some(Cow::Borrowed("main")),
            ..Default::default()
        }),
        compute_layout,
        view_layout,
        render_layout,
        cull_layout,
        _surface_proxy_layout: surface_proxy_layout,
        _surface_proxy_reduce: surface_proxy_reduce,
        vertex_shader: shaders.vertex.clone(),
        fragment_shader: shaders.fragment.clone(),
    });
    commands.insert_resource(shaders);
    commands.insert_resource(WorldShaderInitialization::default());
    commands.insert_resource(WorldViewPipelines::default());
}

fn storage(device: &RenderDevice, label: &'static str, size: u64, extra: BufferUsages) -> Buffer {
    device.create_buffer(&bevy::render::render_resource::BufferDescriptor {
        label: Some(label),
        size,
        usage: BufferUsages::STORAGE | extra,
        mapped_at_creation: false,
    })
}

pub(super) fn create_gpu_culling(
    device: &RenderDevice,
    _cache: &PipelineCache,
    _pipelines: &WorldPipelines,
) -> GpuCulling {
    GpuCulling {
        descriptors: storage(
            device,
            "world descriptors",
            u64::from(MAX_GPU_CHUNK_DESCRIPTORS) * 224,
            BufferUsages::COPY_DST,
        ),
        quad_counts: storage(
            device,
            "world model measurements",
            u64::from(MAX_GPU_CHUNK_DESCRIPTORS) * MODEL_INFO_BYTES,
            BufferUsages::empty(),
        ),
        boundary_rows: storage(
            device,
            "world canonical boundary occupancy",
            u64::from(MAX_GPU_CHUNK_DESCRIPTORS) * 96 * 4,
            BufferUsages::empty(),
        ),
        models: storage(
            device,
            "world GPU model ownership",
            u64::from(MAX_GPU_CHUNK_DESCRIPTORS) * 32,
            BufferUsages::empty(),
        ),
        count_requests: storage(
            device,
            "world count requests",
            u64::from(MAX_GPU_CHUNK_DESCRIPTORS) * 32,
            BufferUsages::COPY_DST,
        ),
        emit_requests: storage(
            device,
            "world GPU emit requests",
            u64::from(MAX_GPU_CHUNK_DESCRIPTORS) * 32,
            BufferUsages::empty(),
        ),
        emit_dispatch: storage(
            device,
            "world GPU emit dispatch",
            32,
            BufferUsages::INDIRECT,
        ),
        dummy_geometry: storage(
            device,
            "world count-only geometry binding",
            GENERATED_QUAD_SIZE,
            BufferUsages::empty(),
        ),
        count_bind_groups: Vec::new(),
        build_bind_groups: Vec::new(),
        draw_layers: Vec::new(),
        draw_capacity: 0,
        render_segment_bind_groups: Vec::new(),
    }
}

pub(super) fn write_cull_descriptor(
    queue: &RenderQueue,
    buffer: &Buffer,
    chunk: &ResidentChunk,
    chunks: &HashMap<ChunkId, ResidentChunk>,
    id: ChunkId,
) {
    let mut bytes = world_matrix_bytes(chunk.world_from_chunk);
    for value in chunk
        .coordinate
        .map(|value| value as i32)
        .into_iter()
        .chain([0])
    {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    // Draws and metadata.z are GPU-owned. Never overwrite a committed model
    // with a CPU-side guess when canonical inputs or neighbor bindings change.
    assert_eq!(bytes.len(), 80);
    let base = u64::from(chunk.gpu_slot) * 224;
    queue.write_buffer(buffer, base, &bytes);
    let metadata = [1_u32, chunk.material_seed]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    queue.write_buffer(buffer, base + 160, &metadata);
    queue.write_buffer(buffer, base + 172, &chunk.generation.to_le_bytes());
    bytes.clear();
    for value in [
        (chunk.svo.offset / 16) as u32,
        (chunk.svo.size / 16) as u32,
        chunk.svo.segment as u32,
        chunk.incarnation,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for face in 0..8 {
        let slot = CHUNK_NEIGHBOR_OFFSETS
            .get(face)
            .and_then(|offset| neighbor_id(id, *offset))
            .and_then(|id| chunks.get(&id))
            .map_or(u32::MAX, |neighbor| neighbor.gpu_slot);
        bytes.extend_from_slice(&slot.to_le_bytes());
    }
    assert_eq!(bytes.len(), 48);
    queue.write_buffer(buffer, base + 176, &bytes);
}

pub(super) fn write_chunk_transform(queue: &RenderQueue, buffer: &Buffer, chunk: &ResidentChunk) {
    queue.write_buffer(
        buffer,
        u64::from(chunk.gpu_slot) * 224,
        &world_matrix_bytes(chunk.world_from_chunk),
    );
}

pub(super) fn clear_gpu_draw(queue: &RenderQueue, culling: &GpuCulling, slot: u32) {
    // Page ownership remains in models until the queue-ordered reclaim pass.
    queue.write_buffer(&culling.descriptors, u64::from(slot) * 224, &[0; 224]);
}

fn upload_requests(queue: &RenderQueue, buffer: &Buffer, requests: &[[u32; 8]]) {
    let bytes = requests
        .iter()
        .flat_map(|request| request.iter().flat_map(|value| value.to_le_bytes()))
        .collect::<Vec<_>>();
    queue.write_buffer(buffer, 0, &bytes);
}

fn build_group(
    device: &RenderDevice,
    cache: &PipelineCache,
    pipelines: &WorldPipelines,
    culling: &GpuCulling,
    svo: &Buffer,
    geometry: &Buffer,
    requests: &Buffer,
    svo_segment: usize,
    geometry_segment: usize,
    pages: &Buffer,
) -> BindGroup {
    let bytes = [
        svo_segment as u32,
        geometry_segment as u32,
        MAX_GPU_CHUNK_DESCRIPTORS,
        0,
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect::<Vec<_>>();
    let uniform = device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("world model pass"),
        contents: &bytes,
        usage: BufferUsages::UNIFORM,
    });
    device.create_bind_group(
        Some("world model pass"),
        &cache.get_bind_group_layout(&pipelines.compute_layout),
        &BindGroupEntries::sequential((
            svo.as_entire_binding(),
            geometry.as_entire_binding(),
            culling.descriptors.as_entire_binding(),
            culling.quad_counts.as_entire_binding(),
            culling.boundary_rows.as_entire_binding(),
            uniform.as_entire_binding(),
            requests.as_entire_binding(),
            culling.models.as_entire_binding(),
            pages.as_entire_binding(),
        )),
    )
}

fn sync_bind_groups(
    device: &RenderDevice,
    cache: &PipelineCache,
    pipelines: &WorldPipelines,
    residency: &mut WorldResidency,
) {
    let svo = &residency.svo_buffers;
    let geometry = residency.geometry_arena.as_ref().unwrap();
    let culling = residency.gpu_culling.as_mut().unwrap();
    while culling.count_bind_groups.len() < svo.len() {
        let segment = culling.count_bind_groups.len();
        let group = build_group(
            device,
            cache,
            pipelines,
            culling,
            &svo[segment],
            &culling.dummy_geometry,
            &culling.count_requests,
            segment,
            0,
            &geometry.pages,
        );
        culling.count_bind_groups.push(group);
    }
    while culling.render_segment_bind_groups.len() < geometry.segment_count() {
        let segment = culling.render_segment_bind_groups.len();
        culling
            .render_segment_bind_groups
            .push(device.create_bind_group(
                Some("world committed geometry"),
                &cache.get_bind_group_layout(&pipelines.render_layout),
                &BindGroupEntries::sequential((
                    geometry.segment_buffer(segment).as_entire_binding(),
                    culling.descriptors.as_entire_binding(),
                )),
            ));
    }
    assert!(geometry.segment_count() <= MAX_GPU_GEOMETRY_SEGMENTS as usize);
    if culling.build_bind_groups.len() != svo.len()
        || culling
            .build_bind_groups
            .iter()
            .any(|row| row.len() != geometry.segment_count())
    {
        culling.build_bind_groups = (0..svo.len())
            .map(|s| {
                (0..geometry.segment_count())
                    .map(|g| {
                        build_group(
                            device,
                            cache,
                            pipelines,
                            culling,
                            &svo[s],
                            geometry.segment_buffer(g),
                            &culling.emit_requests,
                            s,
                            g,
                            &geometry.pages,
                        )
                    })
                    .collect()
            })
            .collect();
    }
    let capacity = residency.next_gpu_slot.max(64).next_power_of_two();
    if capacity > culling.draw_capacity {
        culling.draw_layers = (0..GEOMETRY_LOD_COUNT)
            .map(|lod| {
                let visible_draws = storage(
                    device,
                    "world LOD indirect draws",
                    u64::from(capacity) * u64::from(MAX_GPU_GEOMETRY_SEGMENTS) * 16,
                    BufferUsages::INDIRECT,
                );
                let segment_counts = storage(
                    device,
                    "world LOD segment counts",
                    u64::from(MAX_GPU_GEOMETRY_SEGMENTS) * 4,
                    BufferUsages::INDIRECT | BufferUsages::COPY_DST,
                );
                let bytes = [lod as u32, capacity, residency.next_gpu_slot, 0]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>();
                let uniform = device.create_buffer_with_data(&BufferInitDescriptor {
                    label: Some("world LOD draw parameters"),
                    contents: &bytes,
                    usage: BufferUsages::UNIFORM,
                });
                let bind_group = device.create_bind_group(
                    Some("world LOD culling"),
                    &cache.get_bind_group_layout(&pipelines.cull_layout),
                    &BindGroupEntries::sequential((
                        culling.descriptors.as_entire_binding(),
                        visible_draws.as_entire_binding(),
                        segment_counts.as_entire_binding(),
                        uniform.as_entire_binding(),
                    )),
                );
                GpuDrawLayer {
                    visible_draws,
                    segment_counts,
                    bind_group,
                }
            })
            .collect();
        culling.draw_capacity = capacity;
    }
}

fn selection_group(
    device: &RenderDevice,
    cache: &PipelineCache,
    pipelines: &WorldPipelines,
    residency: &WorldResidency,
    views: &[&ExtractedView],
    tolerance: f32,
) -> BindGroup {
    let mut bytes = Vec::with_capacity(views.len() * 80);
    for view in views {
        let matrix = view
            .clip_from_world
            .unwrap_or_else(|| view.clip_from_view * view.world_from_view.to_matrix().inverse());
        bytes.extend(world_matrix_bytes(matrix.to_cols_array()));
        for value in view.viewport.to_array() {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    let views_buffer = device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("world active model views"),
        contents: &bytes,
        usage: BufferUsages::STORAGE,
    });
    let geometry = residency.geometry_arena.as_ref().unwrap();
    // Include dispatch padding so every possible emit-request lane is explicitly
    // invalidated before generation; uninitialized tail requests must never run.
    let bytes = [
        residency.next_gpu_slot.div_ceil(64) * 64,
        geometry.slab_count,
        views.len() as u32,
        tolerance.to_bits(),
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect::<Vec<_>>();
    let uniform = device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("world GPU model selection parameters"),
        contents: &bytes,
        usage: BufferUsages::UNIFORM,
    });
    let culling = residency.gpu_culling.as_ref().unwrap();
    device.create_bind_group(
        Some("world GPU model selection"),
        &cache.get_bind_group_layout(&pipelines.select_layout),
        &BindGroupEntries::sequential((
            culling.descriptors.as_entire_binding(),
            culling.quad_counts.as_entire_binding(),
            culling.models.as_entire_binding(),
            culling.emit_requests.as_entire_binding(),
            geometry.pages.as_entire_binding(),
            views_buffer.as_entire_binding(),
            uniform.as_entire_binding(),
            culling.emit_dispatch.as_entire_binding(),
        )),
    )
}

pub(super) fn world_compute(
    view: ViewQuery<(&ExtractedView, &ExtractedCamera, &ViewUniformOffset)>,
    view_bind_group: Res<WorldViewBindGroup>,
    views: Query<&ExtractedView, bevy::prelude::With<WorldViewPipeline>>,
    extracted: Res<ExtractedChunks>,
    pipelines: Res<WorldPipelines>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    settings: Res<WorldRenderSettings>,
    mut residency: ResMut<WorldResidency>,
    mut context: RenderContext,
) {
    if residency.next_gpu_slot == 0 {
        return;
    }
    // Clear before pipeline readiness checks: no stale view commands may draw
    // while a shader is compiling/reloading. Reclaim also runs for an empty world.
    if let Some(culling) = &residency.gpu_culling {
        for layer in &culling.draw_layers {
            context
                .command_encoder()
                .clear_buffer(&layer.segment_counts, 0, None);
        }
    }
    let (view, _, view_offset) = view.into_inner();
    let mut active_views = views.iter().collect::<Vec<_>>();
    if active_views.is_empty() {
        active_views.push(view);
    }
    let (
        Some(boundary),
        Some(reset),
        Some(count),
        Some(gather),
        Some(mesh),
        Some(commit),
        Some(reclaim),
        Some(reclassify),
        Some(select),
        Some(finalize_dispatch),
        Some(cull),
    ) = (
        cache.get_compute_pipeline(pipelines.prepare_boundary),
        cache.get_compute_pipeline(pipelines.reset_geometry),
        cache.get_compute_pipeline(pipelines.count_models),
        cache.get_compute_pipeline(pipelines.gather_counts),
        cache.get_compute_pipeline(pipelines.compute),
        cache.get_compute_pipeline(pipelines.finalize_builds),
        cache.get_compute_pipeline(pipelines.reclaim_models),
        cache.get_compute_pipeline(pipelines.reclassify_slabs),
        cache.get_compute_pipeline(pipelines.select_models),
        cache.get_compute_pipeline(pipelines.finalize_dispatch),
        cache.get_compute_pipeline(pipelines.cull),
    )
    else {
        return;
    };
    let started = Instant::now();
    let first_view = residency.last_compute_epoch != Some(extracted.epoch);
    if first_view {
        residency.last_compute_epoch = Some(extracted.epoch);
        let dirty = std::mem::take(&mut residency.geometry_dirty);
        let mut requests = Vec::new();
        for id in dirty {
            if !residency.chunks.contains_key(&id) {
                continue;
            }
            residency.next_generation = residency
                .next_generation
                .checked_add(1)
                .expect("geometry generation exhausted");
            let generation = residency.next_generation;
            let chunk = residency.chunks.get_mut(&id).unwrap();
            chunk.generation = generation;
            requests.push([chunk.gpu_slot, u32::MAX, 0, 0, 0, 0, 0, 0]);
            write_cull_descriptor(
                &queue,
                &residency.gpu_culling.as_ref().unwrap().descriptors,
                &residency.chunks[&id],
                &residency.chunks,
                id,
            );
        }
        sync_bind_groups(&device, &cache, &pipelines, &mut residency);
        let group = selection_group(
            &device,
            &cache,
            &pipelines,
            &residency,
            &active_views,
            settings.geometry_lod_pixel_error,
        );
        let slots = residency.next_gpu_slot.div_ceil(64);
        let slabs = residency
            .geometry_arena
            .as_ref()
            .unwrap()
            .slab_count
            .div_ceil(64);
        for (pipeline, groups) in [(reclaim, slots), (reclassify, slabs)] {
            let mut pass = context
                .command_encoder()
                .begin_compute_pass(&ComputePassDescriptor {
                    label: Some("world queue-ordered page reclamation"),
                    timestamp_writes: None,
                });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(groups, 1, 1);
        }
        if !requests.is_empty() {
            let size = requests.len() as u32;
            requests.resize(size.div_ceil(64) as usize * 64, [u32::MAX; 8]);
            upload_requests(
                &queue,
                &residency.gpu_culling.as_ref().unwrap().count_requests,
                &requests,
            );
            let (x, y, z) = geometry_build_dispatch(size);
            // Every neighbor boundary is ready before any model is counted.
            for (pipeline, dimensions) in [
                (boundary, (96, y, z)),
                (reset, (size.div_ceil(64), 1, 1)),
                (count, (x, y, z)),
                (gather, (size.div_ceil(64), 1, 1)),
            ] {
                let mut pass =
                    context
                        .command_encoder()
                        .begin_compute_pass(&ComputePassDescriptor {
                            label: Some("world GPU model measurement"),
                            timestamp_writes: None,
                        });
                pass.set_pipeline(pipeline);
                for binding in &residency.gpu_culling.as_ref().unwrap().count_bind_groups {
                    pass.set_bind_group(0, binding, &[]);
                    pass.dispatch_workgroups(dimensions.0, dimensions.1, dimensions.2);
                }
            }
        }
        {
            let mut pass = context
                .command_encoder()
                .begin_compute_pass(&ComputePassDescriptor {
                    label: Some("world GPU LOD and page allocation"),
                    timestamp_writes: None,
                });
            pass.set_pipeline(select);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(slots, 1, 1);
            pass.set_pipeline(finalize_dispatch);
            pass.dispatch_workgroups(1, 1, 1);
        }
        // Compact GPU requests drive indirect work. An unchanged scene launches
        // no mesher workgroups, without asking the CPU whether work is pending.
        for (pipeline, offset) in [(reset, 16), (mesh, 4), (commit, 16)] {
            let mut pass = context
                .command_encoder()
                .begin_compute_pass(&ComputePassDescriptor {
                    label: Some("world paged geometry generation and commit"),
                    timestamp_writes: None,
                });
            pass.set_pipeline(pipeline);
            for row in &residency.gpu_culling.as_ref().unwrap().build_bind_groups {
                for binding in row {
                    pass.set_bind_group(0, binding, &[]);
                    pass.dispatch_workgroups_indirect(
                        &residency.gpu_culling.as_ref().unwrap().emit_dispatch,
                        offset,
                    );
                }
            }
        }
        if residency.pending_status.is_none() {
            residency.pending_status = Some(read_gpu_status(
                &mut context,
                &device,
                &residency.geometry_arena.as_ref().unwrap().pages,
            ));
        }
    }
    let culling = residency.gpu_culling.as_ref().unwrap();
    for layer in &culling.draw_layers {
        let mut pass = context
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor {
                label: Some("world LOD frustum culling"),
                timestamp_writes: None,
            });
        pass.set_pipeline(cull);
        pass.set_bind_group(0, &view_bind_group.0, &[view_offset.offset]);
        pass.set_bind_group(1, &layer.bind_group, &[]);
        pass.dispatch_workgroups(residency.next_gpu_slot.div_ceil(64), 1, 1);
    }
    residency.stats.compute_cpu_micros += started.elapsed().as_micros() as u64;
    if extracted
        .epoch
        .wrapping_sub(residency.stats.last_report_epoch)
        >= 120
    {
        bevy::log::debug!(
            "world GPU paging: resident={}, segments={}, reserved_bytes={}, allocations={}, frees={}, commits={}, measurements={}, compute_cpu_us={}",
            residency.chunks.len(),
            residency.geometry_arena.as_ref().unwrap().segment_count(),
            residency.geometry_arena.as_ref().unwrap().reserved_bytes(),
            residency.gpu_activity[0],
            residency.gpu_activity[1],
            residency.gpu_activity[2],
            residency.gpu_activity[3],
            residency.stats.compute_cpu_micros
        );
        residency.stats = WorldRenderStats {
            last_report_epoch: extracted.epoch,
            ..Default::default()
        };
    }
}
