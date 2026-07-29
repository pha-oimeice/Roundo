use crate::local_coordinate::{base::rebuild_dirty_chunk_triangles, data::LocalCoordinate};
use bevy::{
    asset::RenderAssetUsages,
    mesh::Indices,
    prelude::{
        App, Assets, Color, Commands, Component, Entity, Handle, IntoScheduleConfigs, Mesh, Mesh3d,
        MeshMaterial3d, Plugin, Query, ResMut, StandardMaterial, Update, With, Without,
    },
    render::render_resource::PrimitiveTopology,
};

/// Builds client-side render meshes from shared chunk triangle caches.
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

/// Private render asset state owned by the client output path.
#[derive(Component)]
struct LocalCoordinateRenderState {
    geometry_revision: u64,
    mesh_handle: Option<Handle<Mesh>>,
    material_handle: Option<Handle<StandardMaterial>>,
}

fn sync_local_coordinate_meshes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    local_coordinates: Query<(
        Entity,
        &LocalCoordinate,
        Option<&LocalCoordinateRenderState>,
    )>,
) {
    for (entity, local_coordinate, render_state) in &local_coordinates {
        if render_state
            .is_some_and(|state| state.geometry_revision == local_coordinate.geometry_revision)
        {
            continue;
        }

        let Some(mesh) = render_mesh(local_coordinate) else {
            commands
                .entity(entity)
                .remove::<Mesh3d>()
                .remove::<MeshMaterial3d<StandardMaterial>>()
                .insert(LocalCoordinateRenderState {
                    geometry_revision: local_coordinate.geometry_revision,
                    mesh_handle: None,
                    material_handle: None,
                });
            continue;
        };

        let mesh_handle = if let Some(mesh_handle) =
            render_state.and_then(|state| state.mesh_handle.as_ref())
            && let Some(mut existing_mesh) = meshes.get_mut(mesh_handle)
        {
            *existing_mesh = mesh;
            mesh_handle.clone()
        } else {
            meshes.add(mesh)
        };
        let material_handle = render_state
            .and_then(|state| state.material_handle.as_ref())
            .filter(|handle| materials.get(*handle).is_some())
            .cloned()
            .unwrap_or_else(|| {
                materials.add(StandardMaterial {
                    base_color: Color::WHITE,
                    perceptual_roughness: 1.0,
                    unlit: true,
                    ..Default::default()
                })
            });

        commands.entity(entity).insert((
            Mesh3d(mesh_handle.clone()),
            MeshMaterial3d(material_handle.clone()),
            LocalCoordinateRenderState {
                geometry_revision: local_coordinate.geometry_revision,
                mesh_handle: Some(mesh_handle),
                material_handle: Some(material_handle),
            },
        ));
    }
}

fn cleanup_removed_local_coordinate_meshes(
    mut commands: Commands,
    stale_entities: Query<Entity, (With<LocalCoordinateRenderState>, Without<LocalCoordinate>)>,
) {
    for entity in &stale_entities {
        commands
            .entity(entity)
            .remove::<Mesh3d>()
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .remove::<LocalCoordinateRenderState>();
    }
}

fn render_mesh(local_coordinate: &LocalCoordinate) -> Option<Mesh> {
    if local_coordinate.triangles.is_empty() {
        return None;
    }

    let vertex_count = local_coordinate.triangles.len() * 3;
    let mut positions = Vec::with_capacity(vertex_count);
    let mut normals = Vec::with_capacity(vertex_count);
    let mut colors = Vec::with_capacity(vertex_count);
    let mut indices = Vec::with_capacity(vertex_count);

    for triangle in &local_coordinate.triangles {
        let first_index = u32::try_from(positions.len()).ok()?;
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
    Some(mesh)
}
