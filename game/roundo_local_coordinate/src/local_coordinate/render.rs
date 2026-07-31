use crate::local_coordinate::{
    base::rebuild_dirty_chunk_triangles,
    data::{CHUNK_EDGE_LENGTH, Chunk, LocalCoordinate},
};
use bevy::{
    asset::RenderAssetUsages,
    mesh::Indices,
    prelude::{
        App, Assets, ChildOf, Color, Commands, Component, Entity, Handle, IVec3,
        IntoScheduleConfigs, Mesh, Mesh3d, MeshMaterial3d, Plugin, Query, ResMut, StandardMaterial,
        Transform, Update, With, Without,
    },
    render::render_resource::PrimitiveTopology,
};
use std::collections::HashMap;

/// Builds one client-side mesh for each completed, visible chunk.
pub struct LocalCoordinateRenderPlugin;

impl Plugin for LocalCoordinateRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            sync_local_coordinate_meshes.after(rebuild_dirty_chunk_triangles),
        )
        .add_systems(Update, cleanup_removed_local_coordinate_meshes);
    }
}

#[derive(Component, Default)]
struct LocalCoordinateRenderState {
    geometry_revision: u64,
    material_handle: Option<Handle<StandardMaterial>>,
    chunks: HashMap<IVec3, RenderedChunk>,
}

struct RenderedChunk {
    entity: Entity,
    mesh_handle: Handle<Mesh>,
    geometry_revision: u64,
}

#[derive(Component)]
struct LocalCoordinateChunkMesh;

fn sync_local_coordinate_meshes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut local_coordinates: Query<(
        Entity,
        &LocalCoordinate,
        Option<&mut LocalCoordinateRenderState>,
    )>,
) {
    for (entity, local_coordinate, render_state) in &mut local_coordinates {
        let Some(mut render_state) = render_state else {
            let mut render_state = LocalCoordinateRenderState::default();
            sync_render_state(
                entity,
                local_coordinate,
                &mut render_state,
                &mut commands,
                &mut meshes,
                &mut materials,
            );
            commands.entity(entity).insert(render_state);
            continue;
        };

        if render_state.geometry_revision == local_coordinate.geometry_revision {
            continue;
        }
        sync_render_state(
            entity,
            local_coordinate,
            &mut render_state,
            &mut commands,
            &mut meshes,
            &mut materials,
        );
    }
}

fn sync_render_state(
    local_coordinate_entity: Entity,
    local_coordinate: &LocalCoordinate,
    render_state: &mut LocalCoordinateRenderState,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let stale_positions = render_state
        .chunks
        .keys()
        .filter(|position| {
            local_coordinate
                .chunks
                .get(*position)
                .map_or(true, |chunk| chunk.triangles.is_empty())
        })
        .copied()
        .collect::<Vec<_>>();
    for position in stale_positions {
        if let Some(rendered_chunk) = render_state.chunks.remove(&position) {
            commands.entity(rendered_chunk.entity).despawn();
            meshes.remove(rendered_chunk.mesh_handle.id());
        }
    }

    if local_coordinate
        .chunks
        .values()
        .all(|chunk| chunk.triangles.is_empty())
    {
        render_state.geometry_revision = local_coordinate.geometry_revision;
        return;
    }

    let material_handle = render_material(render_state, materials);
    for (position, chunk) in &local_coordinate.chunks {
        if chunk.triangles.is_empty()
            || render_state
                .chunks
                .get(position)
                .is_some_and(|rendered| rendered.geometry_revision == chunk.geometry_revision)
        {
            continue;
        }

        let mesh = render_chunk_mesh(chunk);
        if let Some(rendered_chunk) = render_state.chunks.get_mut(position) {
            if let Some(mut existing_mesh) = meshes.get_mut(&rendered_chunk.mesh_handle) {
                *existing_mesh = mesh;
            } else {
                let mesh_handle = meshes.add(mesh);
                commands
                    .entity(rendered_chunk.entity)
                    .insert(Mesh3d(mesh_handle.clone()));
                rendered_chunk.mesh_handle = mesh_handle;
            }
            rendered_chunk.geometry_revision = chunk.geometry_revision;
            continue;
        }

        let mesh_handle = meshes.add(mesh);
        let chunk_origin = (*position * CHUNK_EDGE_LENGTH).as_vec3();
        let chunk_entity = commands
            .spawn((
                LocalCoordinateChunkMesh,
                ChildOf(local_coordinate_entity),
                Transform::from_translation(chunk_origin),
                Mesh3d(mesh_handle.clone()),
                MeshMaterial3d(material_handle.clone()),
            ))
            .id();
        render_state.chunks.insert(
            *position,
            RenderedChunk {
                entity: chunk_entity,
                mesh_handle,
                geometry_revision: chunk.geometry_revision,
            },
        );
    }

    render_state.geometry_revision = local_coordinate.geometry_revision;
}

fn render_material(
    render_state: &mut LocalCoordinateRenderState,
    materials: &mut Assets<StandardMaterial>,
) -> Handle<StandardMaterial> {
    if let Some(material_handle) = render_state
        .material_handle
        .as_ref()
        .filter(|handle| materials.get(*handle).is_some())
    {
        return material_handle.clone();
    }

    let material_handle = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 1.0,
        unlit: true,
        ..Default::default()
    });
    render_state.material_handle = Some(material_handle.clone());
    material_handle
}

fn cleanup_removed_local_coordinate_meshes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut stale_entities: Query<
        (Entity, &mut LocalCoordinateRenderState),
        (With<LocalCoordinateRenderState>, Without<LocalCoordinate>),
    >,
) {
    for (entity, mut render_state) in &mut stale_entities {
        for (_, rendered_chunk) in render_state.chunks.drain() {
            commands.entity(rendered_chunk.entity).despawn();
            meshes.remove(rendered_chunk.mesh_handle.id());
        }
        if let Some(material_handle) = render_state.material_handle.take() {
            materials.remove(material_handle.id());
        }
        commands
            .entity(entity)
            .remove::<LocalCoordinateRenderState>();
    }
}

fn render_chunk_mesh(chunk: &Chunk) -> Mesh {
    let vertex_count = chunk.triangles.len() * 3;
    let mut positions = Vec::with_capacity(vertex_count);
    let mut normals = Vec::with_capacity(vertex_count);
    let mut colors = Vec::with_capacity(vertex_count);
    let mut indices = Vec::with_capacity(vertex_count);

    for triangle in &chunk.triangles {
        let first_index =
            u32::try_from(positions.len()).expect("one chunk mesh must fit in u32 indices");
        for vertex in triangle.vertices {
            positions.push(vertex.to_array());
            normals.push(triangle.normal.to_array());
            colors.push(triangle.color);
        }
        indices.extend([first_index, first_index + 1, first_index + 2]);
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}
