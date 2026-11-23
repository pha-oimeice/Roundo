//! Per-view pipeline preparation and drawing of committed resident geometry.

use super::*;

pub(super) fn world_primitive_state() -> PrimitiveState {
    PrimitiveState {
        topology: PrimitiveTopology::TriangleList,
        strip_index_format: None,
        front_face: FrontFace::Ccw,
        cull_mode: Some(Face::Back),
        unclipped_depth: false,
        polygon_mode: PolygonMode::Fill,
        conservative: false,
    }
}

pub(super) fn prepare_view_pipelines(
    mut commands: Commands,
    views: Query<(
        Entity,
        &ViewTarget,
        &Msaa,
        bevy::prelude::Has<bevy::prelude::Camera3d>,
    )>,
    pipelines: Res<WorldPipelines>,
    pipeline_cache: Res<PipelineCache>,
    mut view_pipelines: ResMut<WorldViewPipelines>,
) {
    // Prewarm from actual targets, including the menu's 2D camera. Waiting for
    // an active 3D camera would defer vertex/fragment validation until game entry.
    for (entity, target, msaa, is_3d) in &views {
        let key = (target.main_texture_format(), msaa.samples());
        let id = *view_pipelines.0.entry(key).or_insert_with(|| {
            pipeline_cache.queue_render_pipeline(RenderPipelineDescriptor {
                label: Some(Cow::Borrowed("world generated triangles")),
                layout: vec![
                    pipelines.view_layout.clone(),
                    pipelines.render_layout.clone(),
                ],
                vertex: VertexState {
                    shader: pipelines.vertex_shader.clone(),
                    entry_point: Some(Cow::Borrowed("vertex")),
                    buffers: Vec::new(),
                    ..Default::default()
                },
                primitive: world_primitive_state(),
                depth_stencil: Some(DepthStencilState {
                    format: TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(CompareFunction::Greater),
                    stencil: StencilState::default(),
                    bias: Default::default(),
                }),
                multisample: MultisampleState {
                    count: msaa.samples(),
                    ..Default::default()
                },
                fragment: Some(FragmentState {
                    shader: pipelines.fragment_shader.clone(),
                    entry_point: Some(Cow::Borrowed("fragment")),
                    targets: vec![Some(ColorTargetState {
                        format: target.main_texture_format(),
                        blend: Some(BlendState::REPLACE),
                        write_mask: ColorWrites::ALL,
                    })],
                    ..Default::default()
                }),
                ..Default::default()
            })
        });
        if is_3d {
            commands.entity(entity).insert(WorldViewPipeline(id));
        }
    }
}

pub(super) fn prepare_view_bind_group(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    view_uniforms: Res<ViewUniforms>,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<WorldPipelines>,
) {
    let Some(binding) = view_uniforms.uniforms.binding() else {
        return;
    };
    commands.insert_resource(WorldViewBindGroup(render_device.create_bind_group(
        Some("world view bind group"),
        &pipeline_cache.get_bind_group_layout(&pipelines.view_layout),
        &BindGroupEntries::single(binding),
    )));
}

pub(super) fn world_draw(
    view: ViewQuery<(
        &ViewTarget,
        &ViewDepthStencilTexture,
        &ViewUniformOffset,
        &WorldViewPipeline,
        &ExtractedCamera,
    )>,
    view_bind_group: Res<WorldViewBindGroup>,
    pipelines: Res<PipelineCache>,
    residency: Res<WorldResidency>,
    mut context: RenderContext,
) {
    if residency.chunks.is_empty() {
        return;
    }
    let (target, depth, view_offset, pipeline_id, camera) = view.into_inner();
    let Some(pipeline) = pipelines.get_render_pipeline(pipeline_id.0) else {
        return;
    };
    let color_attachments = [Some(target.get_color_attachment())];
    let mut pass = context.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("world generated triangles"),
        color_attachments: &color_attachments,
        depth_stencil_attachment: Some(depth.get_attachment(StoreOp::Store)),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_render_pipeline(pipeline);
    pass.set_bind_group(0, &view_bind_group.0, &[view_offset.offset]);
    let culling = residency
        .gpu_culling
        .as_ref()
        .expect("GPU culling exists while resident Chunks are drawable");
    if let Some(viewport) = &camera.viewport {
        pass.set_camera_viewport(viewport);
    }
    for layer in &culling.draw_layers {
        for (segment, bind_group) in culling.render_segment_bind_groups.iter().enumerate() {
            pass.set_bind_group(1, bind_group, &[]);
            pass.wgpu_pass().multi_draw_indirect_count(
                &layer.visible_draws,
                segment as u64 * u64::from(culling.draw_capacity) * 16,
                &layer.segment_counts,
                segment as u64 * 4,
                culling.draw_capacity,
            );
        }
    }
}
