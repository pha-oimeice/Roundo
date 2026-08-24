use crate::local_coordinate::{
    base::rebuild_dirty_chunk_triangles,
    data::{CHUNK_EDGE_LENGTH, Chunk, LocalCoordinate},
    transform::LocalCoordinateTransform,
};
use bevy::prelude::{
    App, Entity, IVec3, IntoScheduleConfigs, Plugin, Query, ResMut, Resource, Update,
};
use roundo_rendering::{
    RenderMaterial, RenderMesh, RenderObject, RenderObjectId, RenderObjects, RenderTransform,
    issue_render_object, remove_render_object, update_render_object_mesh,
    update_render_object_transform,
};
use std::collections::HashMap;

/// Derives render-object instructions from local-coordinate primitive data.
pub struct LocalCoordinateMeshPlugin;

impl Plugin for LocalCoordinateMeshPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalCoordinateMeshRegistry>()
            .add_systems(
                Update,
                sync_local_coordinate_meshes.after(rebuild_dirty_chunk_triangles),
            );
    }
}

#[derive(Resource, Default)]
struct LocalCoordinateMeshRegistry(HashMap<Entity, LocalCoordinateMeshState>);

#[derive(Default)]
struct LocalCoordinateMeshState {
    initialized: bool,
    geometry_revision: u64,
    transform: LocalCoordinateTransform,
    chunks: HashMap<IVec3, ChunkMeshState>,
}

#[derive(Default)]
struct ChunkMeshState {
    initialized: bool,
    geometry_revision: u64,
    lods: HashMap<ChunkMeshLod, RenderObjectId>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ChunkMeshLod(u8);

struct GeneratedChunkMesh {
    lod: ChunkMeshLod,
    mesh: RenderMesh,
}

fn sync_local_coordinate_meshes(
    mut registry: ResMut<LocalCoordinateMeshRegistry>,
    mut render_objects: ResMut<RenderObjects>,
    local_coordinates: Query<(Entity, &LocalCoordinate, &LocalCoordinateTransform)>,
) {
    let stale_owners = registry
        .0
        .keys()
        .filter(|owner| local_coordinates.get(**owner).is_err())
        .copied()
        .collect::<Vec<_>>();
    for owner in stale_owners {
        let state = registry
            .0
            .remove(&owner)
            .expect("collected local-coordinate mesh owner must exist");
        remove_mesh_state(state, &mut render_objects);
    }

    for (owner, local_coordinate, local_transform) in &local_coordinates {
        let state = registry.0.entry(owner).or_default();
        let transform_changed = state.transform != *local_transform;
        if state.initialized
            && state.geometry_revision == local_coordinate.geometry_revision
            && !transform_changed
        {
            continue;
        }

        sync_mesh_state(
            owner,
            local_coordinate,
            *local_transform,
            state,
            &mut render_objects,
        );
    }
}

fn sync_mesh_state(
    owner: Entity,
    local_coordinate: &LocalCoordinate,
    local_transform: LocalCoordinateTransform,
    state: &mut LocalCoordinateMeshState,
    render_objects: &mut RenderObjects,
) {
    let stale_chunks = state
        .chunks
        .keys()
        .filter(|position| !local_coordinate.chunks.contains_key(*position))
        .copied()
        .collect::<Vec<_>>();
    for position in stale_chunks {
        if let Some(chunk_state) = state.chunks.remove(&position) {
            remove_chunk_meshes(chunk_state, render_objects);
        }
    }

    for (position, chunk) in &local_coordinate.chunks {
        let chunk_state = state.chunks.entry(*position).or_default();
        if !chunk_state.initialized || chunk_state.geometry_revision != chunk.geometry_revision {
            sync_chunk_meshes(
                owner,
                *position,
                chunk,
                local_transform,
                chunk_state,
                render_objects,
            );
        } else if state.transform != local_transform {
            let transform = chunk_render_transform(local_transform, *position);
            for id in chunk_state.lods.values().copied() {
                update_render_object_transform(render_objects, id, transform);
            }
        }
    }

    state.geometry_revision = local_coordinate.geometry_revision;
    state.transform = local_transform;
    state.initialized = true;
}

fn sync_chunk_meshes(
    owner: Entity,
    position: IVec3,
    chunk: &Chunk,
    local_transform: LocalCoordinateTransform,
    state: &mut ChunkMeshState,
    render_objects: &mut RenderObjects,
) {
    let generated = generate_chunk_meshes(chunk);
    let generated_lods = generated.iter().map(|mesh| mesh.lod).collect::<Vec<_>>();
    let stale_lods = state
        .lods
        .keys()
        .filter(|lod| !generated_lods.contains(lod))
        .copied()
        .collect::<Vec<_>>();
    for lod in stale_lods {
        if let Some(id) = state.lods.remove(&lod) {
            remove_render_object(render_objects, id);
        }
    }

    let transform = chunk_render_transform(local_transform, position);
    for generated_mesh in generated {
        if let Some(id) = state.lods.get(&generated_mesh.lod).copied() {
            update_render_object_mesh(render_objects, id, generated_mesh.mesh);
            update_render_object_transform(render_objects, id, transform);
            continue;
        }

        let id = issue_render_object(
            render_objects,
            RenderObject::new(generated_mesh.mesh, chunk_material(), transform).with_name(format!(
                "Local Coordinate {} Chunk [{}, {}, {}] LOD {}",
                owner.index(),
                position.x,
                position.y,
                position.z,
                generated_mesh.lod.0,
            )),
        );
        state.lods.insert(generated_mesh.lod, id);
    }
    state.geometry_revision = chunk.geometry_revision;
    state.initialized = true;
}

fn remove_mesh_state(state: LocalCoordinateMeshState, render_objects: &mut RenderObjects) {
    for chunk_state in state.chunks.into_values() {
        remove_chunk_meshes(chunk_state, render_objects);
    }
}

fn remove_chunk_meshes(state: ChunkMeshState, render_objects: &mut RenderObjects) {
    for id in state.lods.into_values() {
        remove_render_object(render_objects, id);
    }
}

fn chunk_render_transform(
    local_transform: LocalCoordinateTransform,
    position: IVec3,
) -> RenderTransform {
    let coordinate_transform = RenderTransform::from_parts(
        local_transform.translation,
        local_transform.rotation,
        local_transform.scale,
    );
    let chunk_transform =
        RenderTransform::from_translation((position * CHUNK_EDGE_LENGTH).as_vec3());
    coordinate_transform.compose(chunk_transform)
}

fn chunk_material() -> RenderMaterial {
    RenderMaterial::unlit([1.0; 4]).with_perceptual_roughness(1.0)
}

/// The output is keyed by LOD so an SVO algorithm can return several meshes for one chunk.
/// The current lossless surface extraction has one exact LOD.
fn generate_chunk_meshes(chunk: &Chunk) -> Vec<GeneratedChunkMesh> {
    if chunk.triangles.is_empty() {
        return Vec::new();
    }
    vec![GeneratedChunkMesh {
        lod: ChunkMeshLod(0),
        mesh: chunk_mesh(chunk),
    }]
}

fn chunk_mesh(chunk: &Chunk) -> RenderMesh {
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

    RenderMesh::triangle_list(positions, normals, colors, indices)
        .expect("local-coordinate triangle generation emits valid mesh data")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_coordinate::data::{SOLID_VOXEL_ID, VoxelTriangle};
    use bevy::prelude::{App, Vec3};
    use roundo_rendering::{
        RenderObjectPlugin, render_object, render_object_mesh, render_object_transform,
    };

    #[test]
    fn updating_one_chunk_preserves_every_render_object_id() {
        let mut app = mesh_app();
        let first_position = IVec3::new(2, -1, 3);
        let second_position = IVec3::new(7, 0, -4);
        let owner = app
            .world_mut()
            .spawn((
                LocalCoordinate {
                    chunks: HashMap::from([
                        (first_position, test_chunk(1)),
                        (second_position, test_chunk(1)),
                    ]),
                    geometry_revision: 1,
                    ..Default::default()
                },
                LocalCoordinateTransform::from_translation(Vec3::new(1_000.0, 2_000.0, 3_000.0)),
            ))
            .id();

        app.update();
        let (first_id, second_id) = render_ids(&app, owner, first_position, second_position);
        assert_eq!(
            render_object_transform(app.world().resource(), first_id)
                .unwrap()
                .translation,
            Vec3::new(1_032.0, 1_984.0, 3_048.0)
        );

        {
            let mut local_coordinate = app
                .world_mut()
                .get_mut::<LocalCoordinate>(owner)
                .expect("test local coordinate exists");
            let chunk = local_coordinate
                .chunks
                .get_mut(&second_position)
                .expect("second test chunk exists");
            assert!(chunk.set_voxel(IVec3::new(1, 0, 0), SOLID_VOXEL_ID));
            chunk.triangles.push(test_triangle(1.0));
            chunk.geometry_revision = 2;
            local_coordinate.geometry_revision = 2;
        }

        app.update();
        let (updated_first_id, updated_second_id) =
            render_ids(&app, owner, first_position, second_position);
        assert_eq!(updated_first_id, first_id);
        assert_eq!(updated_second_id, second_id);
        let objects = app.world().resource::<RenderObjects>();
        assert_eq!(
            render_object_mesh(objects, first_id)
                .unwrap()
                .vertex_count(),
            3
        );
        assert_eq!(
            render_object_mesh(objects, second_id)
                .unwrap()
                .vertex_count(),
            6
        );
        app.world_mut()
            .get_mut::<LocalCoordinateTransform>(owner)
            .unwrap()
            .translation += Vec3::Y;
        app.update();
        let (moved_first_id, moved_second_id) =
            render_ids(&app, owner, first_position, second_position);
        assert_eq!((moved_first_id, moved_second_id), (first_id, second_id));
        assert_eq!(
            render_object_transform(app.world().resource(), first_id)
                .unwrap()
                .translation,
            Vec3::new(1_032.0, 1_985.0, 3_048.0)
        );
    }

    #[test]
    fn despawning_an_owner_revokes_its_render_object_ids() {
        let mut app = mesh_app();
        let owner = app
            .world_mut()
            .spawn(LocalCoordinate {
                chunks: HashMap::from([(IVec3::ZERO, test_chunk(1))]),
                geometry_revision: 1,
                ..Default::default()
            })
            .id();
        app.update();
        let id = app.world().resource::<LocalCoordinateMeshRegistry>().0[&owner].chunks
            [&IVec3::ZERO]
            .lods[&ChunkMeshLod(0)];

        app.world_mut().despawn(owner);
        app.update();

        assert!(render_object(app.world().resource(), id).is_none());
    }

    fn mesh_app() -> App {
        let mut app = App::new();
        app.add_plugins((RenderObjectPlugin, LocalCoordinateMeshPlugin));
        app
    }

    fn test_chunk(geometry_revision: u64) -> Chunk {
        let mut chunk = Chunk::default();
        assert!(chunk.set_voxel(IVec3::ZERO, SOLID_VOXEL_ID));
        chunk.triangles.push(test_triangle(0.0));
        chunk.geometry_revision = geometry_revision;
        chunk
    }

    fn test_triangle(offset: f32) -> VoxelTriangle {
        VoxelTriangle {
            vertices: [
                Vec3::new(offset, 0.0, 0.0),
                Vec3::new(offset + 1.0, 0.0, 0.0),
                Vec3::new(offset, 1.0, 0.0),
            ],
            normal: Vec3::Z,
            color: [1.0; 4],
        }
    }

    fn render_ids(
        app: &App,
        owner: Entity,
        first_position: IVec3,
        second_position: IVec3,
    ) -> (RenderObjectId, RenderObjectId) {
        let registry = app.world().resource::<LocalCoordinateMeshRegistry>();
        (
            registry.0[&owner].chunks[&first_position].lods[&ChunkMeshLod(0)],
            registry.0[&owner].chunks[&second_position].lods[&ChunkMeshLod(0)],
        )
    }
}
