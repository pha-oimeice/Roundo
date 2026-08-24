use crate::local_coordinate::{
    base::rebuild_dirty_chunk_triangles,
    data::{CHUNK_EDGE_LENGTH, Chunk, LocalCoordinate},
};
use avian3d::{math::Vector, prelude::Collider};
use bevy::prelude::{
    App, ChildOf, Commands, Component, Entity, IVec3, IntoScheduleConfigs, Plugin, Query,
    Transform, Update, Without,
};
use std::collections::HashMap;

/// Materializes one chunk-local child collider for each non-empty chunk.
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

/// Private output state owned by one local-coordinate rigid body.
#[derive(Component, Default)]
struct LocalCoordinatePhysicsState {
    geometry_revision: u64,
    chunks: HashMap<IVec3, ChunkColliderState>,
}

struct ChunkColliderState {
    entity: Entity,
    geometry_revision: u64,
}

/// Identifies one child collider by its chunk-local position.
#[derive(Component)]
struct LocalCoordinateChunkCollider {
    position: IVec3,
}

fn sync_local_coordinate_colliders(
    mut commands: Commands,
    mut local_coordinates: Query<(
        Entity,
        &LocalCoordinate,
        Option<&mut LocalCoordinatePhysicsState>,
    )>,
) {
    for (owner, local_coordinate, physics_state) in &mut local_coordinates {
        if physics_state
            .as_ref()
            .is_some_and(|state| state.geometry_revision == local_coordinate.geometry_revision)
        {
            continue;
        }

        if let Some(mut physics_state) = physics_state {
            sync_physics_state(owner, local_coordinate, &mut physics_state, &mut commands);
        } else {
            let mut physics_state = LocalCoordinatePhysicsState::default();
            sync_physics_state(owner, local_coordinate, &mut physics_state, &mut commands);
            commands.entity(owner).insert(physics_state);
        }
    }
}

fn sync_physics_state(
    owner: Entity,
    local_coordinate: &LocalCoordinate,
    state: &mut LocalCoordinatePhysicsState,
    commands: &mut Commands,
) {
    let stale_positions = state
        .chunks
        .keys()
        .filter(|position| !local_coordinate.chunks.contains_key(*position))
        .copied()
        .collect::<Vec<_>>();
    for position in stale_positions {
        remove_chunk_collider(position, state, commands);
    }

    for (position, chunk) in &local_coordinate.chunks {
        if state
            .chunks
            .get(position)
            .is_some_and(|chunk_state| chunk_state.geometry_revision == chunk.geometry_revision)
        {
            continue;
        }

        let Some(collider) = chunk_collider(chunk) else {
            remove_chunk_collider(*position, state, commands);
            continue;
        };

        if let Some(chunk_state) = state.chunks.get_mut(position) {
            commands.entity(chunk_state.entity).try_insert(collider);
            chunk_state.geometry_revision = chunk.geometry_revision;
            continue;
        }

        let entity = commands
            .spawn((
                LocalCoordinateChunkCollider {
                    position: *position,
                },
                collider,
                Transform::from_translation((*position * CHUNK_EDGE_LENGTH).as_vec3()),
                ChildOf(owner),
            ))
            .id();
        state.chunks.insert(
            *position,
            ChunkColliderState {
                entity,
                geometry_revision: chunk.geometry_revision,
            },
        );
    }

    state.geometry_revision = local_coordinate.geometry_revision;
}

fn remove_chunk_collider(
    position: IVec3,
    state: &mut LocalCoordinatePhysicsState,
    commands: &mut Commands,
) {
    if let Some(chunk_state) = state.chunks.remove(&position) {
        commands.entity(chunk_state.entity).try_despawn();
    }
}

fn cleanup_removed_local_coordinate_colliders(
    mut commands: Commands,
    stale_entities: Query<(Entity, &LocalCoordinatePhysicsState), Without<LocalCoordinate>>,
) {
    for (owner, physics_state) in &stale_entities {
        for chunk_state in physics_state.chunks.values() {
            commands.entity(chunk_state.entity).try_despawn();
        }
        commands
            .entity(owner)
            .remove::<LocalCoordinatePhysicsState>();
    }
}

fn chunk_collider(chunk: &Chunk) -> Option<Collider> {
    if chunk.triangles.is_empty() {
        return None;
    }

    let mut vertices = Vec::with_capacity(chunk.triangles.len() * 3);
    let mut indices = Vec::with_capacity(chunk.triangles.len());

    for triangle in &chunk.triangles {
        let first_index = u32::try_from(vertices.len()).ok()?;
        for vertex in triangle.vertices {
            vertices.push(Vector::new(vertex.x, vertex.y, vertex.z));
        }
        indices.push([first_index, first_index + 1, first_index + 2]);
    }

    Some(Collider::trimesh(vertices, indices))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_coordinate::data::{SOLID_VOXEL_ID, VoxelTriangle};
    use avian3d::prelude::{ColliderHierarchyPlugin, ColliderOf, RigidBody};
    use bevy::prelude::{App, Vec3};

    #[test]
    fn each_non_empty_chunk_gets_a_local_child_collider() {
        let mut app = physics_app();
        let first_position = IVec3::new(2, -1, 3);
        let second_position = IVec3::new(7, 0, -4);
        let owner = app
            .world_mut()
            .spawn((
                LocalCoordinate {
                    chunks: HashMap::from([
                        (first_position, test_chunk(1)),
                        (second_position, test_chunk(1)),
                        (IVec3::ONE, Chunk::default()),
                    ]),
                    geometry_revision: 1,
                    ..Default::default()
                },
                RigidBody::Static,
            ))
            .id();

        app.update();

        let state = app
            .world()
            .get::<LocalCoordinatePhysicsState>(owner)
            .unwrap();
        assert_eq!(state.chunks.len(), 2);
        assert!(app.world().get::<Collider>(owner).is_none());
        assert_eq!(
            *app.world().get::<RigidBody>(owner).unwrap(),
            RigidBody::Static
        );

        for position in [first_position, second_position] {
            let collider_entity = state.chunks[&position].entity;
            assert!(app.world().get::<Collider>(collider_entity).is_some());
            assert!(app.world().get::<RigidBody>(collider_entity).is_none());
            assert_eq!(
                app.world()
                    .get::<ChildOf>(collider_entity)
                    .unwrap()
                    .parent(),
                owner
            );
            assert_eq!(
                app.world()
                    .get::<Transform>(collider_entity)
                    .unwrap()
                    .translation,
                (position * CHUNK_EDGE_LENGTH).as_vec3()
            );
            assert_eq!(
                app.world()
                    .get::<LocalCoordinateChunkCollider>(collider_entity)
                    .unwrap()
                    .position,
                position
            );
        }
    }

    #[test]
    fn updating_one_chunk_only_rebuilds_its_collider() {
        let mut app = physics_app();
        let first_position = IVec3::ZERO;
        let second_position = IVec3::new(3, 0, 0);
        let owner = app
            .world_mut()
            .spawn(LocalCoordinate {
                chunks: HashMap::from([
                    (first_position, test_chunk(1)),
                    (second_position, test_chunk(1)),
                ]),
                geometry_revision: 1,
                ..Default::default()
            })
            .id();
        app.update();

        let (first_entity, second_entity) =
            collider_entities(&app, owner, first_position, second_position);
        let first_tick = collider_change_tick(&app, first_entity);
        let second_tick = collider_change_tick(&app, second_entity);

        {
            let mut local_coordinate = app.world_mut().get_mut::<LocalCoordinate>(owner).unwrap();
            let second_chunk = local_coordinate.chunks.get_mut(&second_position).unwrap();
            second_chunk.triangles.push(test_triangle(1.0));
            second_chunk.geometry_revision = 2;
            local_coordinate.geometry_revision = 2;
        }
        app.update();

        assert_eq!(
            collider_entities(&app, owner, first_position, second_position),
            (first_entity, second_entity)
        );
        assert_eq!(collider_change_tick(&app, first_entity), first_tick);
        assert_ne!(collider_change_tick(&app, second_entity), second_tick);
    }

    #[test]
    fn removing_a_chunk_despawns_only_its_collider() {
        let mut app = physics_app();
        let retained_position = IVec3::ZERO;
        let removed_position = IVec3::X;
        let owner = app
            .world_mut()
            .spawn(LocalCoordinate {
                chunks: HashMap::from([
                    (retained_position, test_chunk(1)),
                    (removed_position, test_chunk(1)),
                ]),
                geometry_revision: 1,
                ..Default::default()
            })
            .id();
        app.update();

        let (retained_entity, removed_entity) =
            collider_entities(&app, owner, retained_position, removed_position);
        {
            let mut local_coordinate = app.world_mut().get_mut::<LocalCoordinate>(owner).unwrap();
            local_coordinate.chunks.remove(&removed_position);
            local_coordinate.geometry_revision = 2;
        }
        app.update();

        let state = app
            .world()
            .get::<LocalCoordinatePhysicsState>(owner)
            .unwrap();
        assert_eq!(state.chunks.len(), 1);
        assert_eq!(state.chunks[&retained_position].entity, retained_entity);
        assert!(app.world().entities().contains(retained_entity));
        assert!(!app.world().entities().contains(removed_entity));
    }

    #[test]
    fn avian_attaches_chunk_colliders_to_the_owner_rigid_body() {
        let mut app = App::new();
        app.add_plugins((ColliderHierarchyPlugin, LocalCoordinatePhysicsPlugin));
        let owner = app
            .world_mut()
            .spawn((
                LocalCoordinate {
                    chunks: HashMap::from([(IVec3::ZERO, test_chunk(1))]),
                    geometry_revision: 1,
                    ..Default::default()
                },
                RigidBody::Static,
            ))
            .id();

        app.update();

        let collider_entity = app
            .world()
            .get::<LocalCoordinatePhysicsState>(owner)
            .unwrap()
            .chunks[&IVec3::ZERO]
            .entity;
        assert_eq!(
            app.world().get::<ColliderOf>(collider_entity).unwrap().body,
            owner
        );
    }

    #[test]
    fn removing_local_coordinate_state_despawns_its_chunk_colliders() {
        let mut app = physics_app();
        let owner = app
            .world_mut()
            .spawn(LocalCoordinate {
                chunks: HashMap::from([(IVec3::ZERO, test_chunk(1))]),
                geometry_revision: 1,
                ..Default::default()
            })
            .id();
        app.update();
        let collider_entity = app
            .world()
            .get::<LocalCoordinatePhysicsState>(owner)
            .unwrap()
            .chunks[&IVec3::ZERO]
            .entity;

        app.world_mut()
            .entity_mut(owner)
            .remove::<LocalCoordinate>();
        app.update();

        assert!(
            app.world()
                .get::<LocalCoordinatePhysicsState>(owner)
                .is_none()
        );
        assert!(!app.world().entities().contains(collider_entity));
    }

    fn physics_app() -> App {
        let mut app = App::new();
        app.add_plugins(LocalCoordinatePhysicsPlugin);
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

    fn collider_entities(
        app: &App,
        owner: Entity,
        first_position: IVec3,
        second_position: IVec3,
    ) -> (Entity, Entity) {
        let state = app
            .world()
            .get::<LocalCoordinatePhysicsState>(owner)
            .unwrap();
        (
            state.chunks[&first_position].entity,
            state.chunks[&second_position].entity,
        )
    }

    fn collider_change_tick(app: &App, entity: Entity) -> bevy::ecs::change_detection::Tick {
        app.world()
            .entity(entity)
            .get_change_ticks::<Collider>()
            .unwrap()
            .changed
    }
}
