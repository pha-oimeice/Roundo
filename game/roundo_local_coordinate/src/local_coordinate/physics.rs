use crate::local_coordinate::{base::rebuild_dirty_chunk_triangles, data::LocalCoordinate};
use avian3d::{math::Vector, prelude::Collider};
use bevy::prelude::{
    App, Commands, Component, Entity, IntoScheduleConfigs, Plugin, Query, Update, With, Without,
};

/// Builds the server-side collider from shared chunk triangle caches.
pub struct LocalCoordinatePhysicsPlugin;

impl Plugin for LocalCoordinatePhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            sync_local_coordinate_colliders.after(rebuild_dirty_chunk_triangles),
        )
        .add_systems(Update, cleanup_removed_local_coordinate_colliders);
    }
}

/// Private revision marker owned by the physics output path.
#[derive(Component)]
struct LocalCoordinatePhysicsState {
    geometry_revision: u64,
}

fn sync_local_coordinate_colliders(
    mut commands: Commands,
    local_coordinates: Query<(
        Entity,
        &LocalCoordinate,
        Option<&LocalCoordinatePhysicsState>,
    )>,
) {
    for (entity, local_coordinate, physics_state) in &local_coordinates {
        if physics_state
            .is_some_and(|state| state.geometry_revision == local_coordinate.geometry_revision)
        {
            continue;
        }

        if let Some((vertices, indices)) = collider_triangles(local_coordinate) {
            commands
                .entity(entity)
                .insert(Collider::trimesh(vertices, indices));
        } else {
            commands.entity(entity).remove::<Collider>();
        }

        commands.entity(entity).insert(LocalCoordinatePhysicsState {
            geometry_revision: local_coordinate.geometry_revision,
        });
    }
}

fn cleanup_removed_local_coordinate_colliders(
    mut commands: Commands,
    stale_entities: Query<Entity, (With<LocalCoordinatePhysicsState>, Without<LocalCoordinate>)>,
) {
    for entity in &stale_entities {
        commands
            .entity(entity)
            .remove::<Collider>()
            .remove::<LocalCoordinatePhysicsState>();
    }
}

fn collider_triangles(local_coordinate: &LocalCoordinate) -> Option<(Vec<Vector>, Vec<[u32; 3]>)> {
    if local_coordinate.triangles.is_empty() {
        return None;
    }

    let mut vertices = Vec::with_capacity(local_coordinate.triangles.len() * 3);
    let mut indices = Vec::with_capacity(local_coordinate.triangles.len());

    for triangle in &local_coordinate.triangles {
        let first_index = u32::try_from(vertices.len()).ok()?;
        for vertex in triangle.vertices {
            vertices.push(Vector::new(vertex.x, vertex.y, vertex.z));
        }
        indices.push([first_index, first_index + 1, first_index + 2]);
    }

    Some((vertices, indices))
}
