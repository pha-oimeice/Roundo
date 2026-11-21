//! Eager loading and one-time compilation checks for every world shader.

use super::*;
use bevy::{
    asset::{AssetId, DependencyLoadState, LoadState, RecursiveDependencyLoadState},
    prelude::{AssetServer, Handle},
    render::render_resource::{CachedPipelineState, PipelineDescriptor},
    shader::{Shader, ShaderCacheError},
};

pub(super) fn register_world_shaders(app: &mut App) {
    embedded_asset!(app, "shaders/world_cull_lod.wesl");
    embedded_asset!(app, "shaders/world_greedy_emit.wesl");
    embedded_asset!(app, "shaders/world_model_select.wesl");
    embedded_asset!(app, "shaders/world_surface_proxy.wesl");
    embedded_asset!(app, "shaders/world_vertex.wesl");
    embedded_asset!(app, "shaders/world_fragment.wesl");
}

#[derive(Resource)]
pub(super) struct WorldShaders {
    pub(super) cull_lod: Handle<Shader>,
    pub(super) greedy_emit: Handle<Shader>,
    pub(super) model_select: Handle<Shader>,
    pub(super) surface_proxy: Handle<Shader>,
    pub(super) vertex: Handle<Shader>,
    pub(super) fragment: Handle<Shader>,
}

impl WorldShaders {
    pub(super) fn load(server: &AssetServer) -> Self {
        bevy::log::info!("starting world shader compilation checks");
        Self {
            cull_lod: load_embedded_asset!(server, "shaders/world_cull_lod.wesl"),
            greedy_emit: load_embedded_asset!(server, "shaders/world_greedy_emit.wesl"),
            model_select: load_embedded_asset!(server, "shaders/world_model_select.wesl"),
            surface_proxy: load_embedded_asset!(server, "shaders/world_surface_proxy.wesl"),
            vertex: load_embedded_asset!(server, "shaders/world_vertex.wesl"),
            fragment: load_embedded_asset!(server, "shaders/world_fragment.wesl"),
        }
    }

    fn ids(&self) -> HashSet<AssetId<Shader>> {
        [
            &self.cull_lod,
            &self.greedy_emit,
            &self.model_select,
            &self.surface_proxy,
            &self.vertex,
            &self.fragment,
        ]
        .into_iter()
        .map(Handle::id)
        .collect()
    }
}

#[derive(Resource, Default)]
pub(super) struct WorldShaderInitialization {
    ready: bool,
}

pub(super) fn world_shaders_ready(initialization: Res<WorldShaderInitialization>) -> bool {
    initialization.ready
}

/// Asset loading and GPU compilation are asynchronous. Keep initialization pending
/// until all registered shaders and all their queued pipeline entry points pass;
/// neither missing world chunks nor an inactive 3D camera may bypass this check.
pub(super) fn check_world_shader_initialization(
    shaders: Res<WorldShaders>,
    server: Res<AssetServer>,
    mut cache: ResMut<PipelineCache>,
    mut initialization: ResMut<WorldShaderInitialization>,
) {
    if initialization.ready {
        return;
    }
    let required = shaders.ids();
    for id in &required {
        let Some((load, dependencies, recursive)) = server.get_load_states(*id) else {
            continue;
        };
        let failure = match (load, dependencies, recursive) {
            (LoadState::Failed(error), _, _)
            | (_, DependencyLoadState::Failed(error), _)
            | (_, _, RecursiveDependencyLoadState::Failed(error)) => Some(error),
            _ => None,
        };
        if let Some(error) = failure {
            let path = server.get_path(*id);
            bevy::log::error!("world shader loading failed: {path:?}: {error}");
            panic!("world shader loading failed: {path:?}: {error}");
        }
    }

    // Use the real pipeline compiler, including imports, shader definitions,
    // entry points and binding layouts. No separate syntax-only parser or probe
    // pipeline with invented render-target settings is used.
    cache.process_queue();
    let mut compiled = HashSet::new();
    let mut pending = false;
    let mut pipeline_count = 0;
    for pipeline in cache.pipelines() {
        let (ids, label) = match &pipeline.descriptor {
            PipelineDescriptor::ComputePipelineDescriptor(descriptor) => {
                (vec![descriptor.shader.id()], &descriptor.label)
            }
            PipelineDescriptor::RenderPipelineDescriptor(descriptor) => {
                let mut ids = vec![descriptor.vertex.shader.id()];
                if let Some(fragment) = &descriptor.fragment {
                    ids.push(fragment.shader.id());
                }
                (ids, &descriptor.label)
            }
            PipelineDescriptor::MeshPipelineDescriptor(_) => continue,
        };
        if !ids.iter().any(|id| required.contains(id)) {
            continue;
        }
        pipeline_count += 1;
        match &pipeline.state {
            CachedPipelineState::Ok(_) => compiled.extend(ids),
            CachedPipelineState::Queued | CachedPipelineState::Creating(_) => pending = true,
            CachedPipelineState::Err(
                ShaderCacheError::ShaderNotLoaded(_)
                | ShaderCacheError::ShaderImportNotYetAvailable,
            ) => pending = true,
            CachedPipelineState::Err(error) => {
                let paths = ids
                    .iter()
                    .map(|id| server.get_path(*id))
                    .collect::<Vec<_>>();
                bevy::log::error!(
                    "world shader compilation failed: pipeline={label:?}, shaders={paths:?}: {error}"
                );
                panic!(
                    "world shader compilation failed: pipeline={label:?}, shaders={paths:?}: {error}"
                );
            }
        }
    }
    if !pending && required.is_subset(&compiled) {
        initialization.ready = true;
        bevy::log::info!(
            "world shader initialization passed: shaders={}, pipelines={pipeline_count}",
            required.len()
        );
    }
}
