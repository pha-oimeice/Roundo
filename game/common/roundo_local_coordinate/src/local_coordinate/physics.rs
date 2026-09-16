//! Incremental derivation of Avian child colliders from voxel chunks.
//!
//! Collider meshes contain only exposed voxel faces. A chunk's mesh therefore
//! depends on its own content revision and the six adjacent chunk revisions.
//! Rebuild work is queued and bounded per Bevy update; collider state may lag
//! voxel mutations while that queue drains.

use crate::local_coordinate::data::{CHUNK_EDGE_LENGTH, Chunk, LocalCoordinate};
use crate::{LocalCoordinateId, LocalCoordinateIdentity};
use avian3d::{math::Vector, prelude::Collider};
use bevy::prelude::{
    App, ChildOf, Commands, Component, DetectChanges, Entity, IVec3, IntoScheduleConfigs, Plugin,
    Query, Res, Resource, SystemSet, Transform, Update, Without,
};
use std::collections::{HashMap, HashSet, VecDeque};

const MAX_COLLIDER_CHUNKS_PER_UPDATE: usize = 32;

/// Derives one chunk-local child collider for each relevant nonempty chunk.
///
/// Systems run in [`Update`] and process at most 32 queued chunks per coordinate
/// per update. Empty, removed, or no-longer-relevant chunks have their collider
/// entities despawned. Generated colliders are children of the coordinate owner,
/// so the owner's transform and rigid body define their world-space placement.
pub struct LocalCoordinatePhysicsPlugin;

impl Plugin for LocalCoordinatePhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalCoordinatePhysicsInterests>()
            .add_systems(
                Update,
                sync_local_coordinate_colliders.in_set(LocalCoordinatePhysicsSet::Sync),
            )
            .add_systems(
                Update,
                cleanup_removed_local_coordinate_colliders.in_set(LocalCoordinatePhysicsSet::Sync),
            );
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub(crate) enum LocalCoordinatePhysicsSet {
    Sync,
}

/// Optional exact set of chunks for which physics should be materialized.
///
/// The default is unrestricted. After [`Self::replace`], only listed chunks are
/// retained; changing the set schedules reconciliation rather than rebuilding
/// every collider synchronously.
#[derive(Default, Resource)]
pub(crate) struct LocalCoordinatePhysicsInterests {
    restricted: bool,
    chunks: HashMap<LocalCoordinateId, HashSet<IVec3>>,
}

impl LocalCoordinatePhysicsInterests {
    /// Returns whether this resource already contains the same restricted set.
    pub fn matches(&self, chunks: &HashMap<LocalCoordinateId, HashSet<IVec3>>) -> bool {
        self.restricted && self.chunks == *chunks
    }

    /// Replaces unrestricted/default behavior with an exact chunk-interest set.
    ///
    /// The ECS synchronization system observes the resource change and applies
    /// additions/removals subject to its per-update queue budget.
    pub fn replace(&mut self, chunks: HashMap<LocalCoordinateId, HashSet<IVec3>>) {
        self.restricted = true;
        self.chunks = chunks;
    }

    fn contains(&self, id: Option<LocalCoordinateId>, position: IVec3) -> bool {
        !self.restricted
            || id.is_some_and(|id| {
                let chunks = self.chunks.get(&id);
                chunks.is_some_and(|chunks| chunks.contains(&position))
            })
    }
}

/// Derived collider entities and deduplicated rebuild queue for one coordinate.
#[derive(Component, Default)]
struct LocalCoordinatePhysicsState {
    chunks: HashMap<IVec3, ChunkColliderState>,
    pending_chunks: VecDeque<IVec3>,
    pending_set: HashSet<IVec3>,
}

struct ChunkColliderState {
    entity: Entity,
    source: ChunkColliderSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ChunkColliderSource {
    content_revision: u64,
    neighbor_revisions: [Option<u64>; 6],
}

/// Identifies one child collider by its chunk-local position.
#[derive(Component)]
struct LocalCoordinateChunkCollider {
    position: IVec3,
}

const FACE_OFFSETS: [IVec3; 6] = [
    IVec3::X,
    IVec3::NEG_X,
    IVec3::Y,
    IVec3::NEG_Y,
    IVec3::Z,
    IVec3::NEG_Z,
];

// Consumes dirty chunks, includes face neighbors, then drains bounded rebuild work.
fn sync_local_coordinate_colliders(
    mut commands: Commands,
    interests: Res<LocalCoordinatePhysicsInterests>,
    mut local_coordinates: Query<(
        Entity,
        Option<&LocalCoordinateIdentity>,
        &mut LocalCoordinate,
        Option<&mut LocalCoordinatePhysicsState>,
    )>,
) {
    for (owner, identity, mut local_coordinate, physics_state) in &mut local_coordinates {
        let identity = identity.map(|identity| identity.0);
        let changed = std::mem::take(&mut local_coordinate.physics_dirty_chunks);
        if let Some(mut physics_state) = physics_state {
            if interests.is_changed() {
                for position in physics_state.chunks.keys().copied().collect::<Vec<_>>() {
                    enqueue_collider_chunk(&mut physics_state, position);
                }
                if let Some(id) = identity
                    && let Some(desired) = interests.chunks.get(&id)
                {
                    for position in desired.iter().copied() {
                        enqueue_collider_chunk(&mut physics_state, position);
                    }
                }
            }
            for position in changed.into_iter().flat_map(|position| {
                std::iter::once(position).chain(FACE_OFFSETS.map(|offset| position + offset))
            }) {
                if interests.contains(identity, position)
                    || physics_state.chunks.contains_key(&position)
                {
                    enqueue_collider_chunk(&mut physics_state, position);
                }
            }
            let affected = take_pending_collider_chunks(&mut physics_state);
            sync_physics_state(
                owner,
                identity,
                &interests,
                &local_coordinate,
                affected,
                &mut physics_state,
                &mut commands,
            );
        } else {
            let mut physics_state = LocalCoordinatePhysicsState::default();
            for position in local_coordinate.chunks.keys().copied() {
                if interests.contains(identity, position) {
                    enqueue_collider_chunk(&mut physics_state, position);
                }
            }
            let affected = take_pending_collider_chunks(&mut physics_state);
            sync_physics_state(
                owner,
                identity,
                &interests,
                &local_coordinate,
                affected,
                &mut physics_state,
                &mut commands,
            );
            commands.entity(owner).insert(physics_state);
        }
    }
}

fn enqueue_collider_chunk(state: &mut LocalCoordinatePhysicsState, position: IVec3) {
    if state.pending_set.insert(position) {
        state.pending_chunks.push_back(position);
    }
}

fn take_pending_collider_chunks(state: &mut LocalCoordinatePhysicsState) -> Vec<IVec3> {
    let count = state
        .pending_chunks
        .len()
        .min(MAX_COLLIDER_CHUNKS_PER_UPDATE);
    let mut chunks = Vec::with_capacity(count);
    for _ in 0..count {
        let position = state
            .pending_chunks
            .pop_front()
            .expect("bounded collider queue length was checked");
        let removed = state.pending_set.remove(&position);
        debug_assert!(removed, "queued collider Chunk must be in the pending set");
        chunks.push(position);
    }
    chunks
}

fn sync_physics_state(
    owner: Entity,
    identity: Option<LocalCoordinateId>,
    interests: &LocalCoordinatePhysicsInterests,
    local_coordinate: &LocalCoordinate,
    affected: impl IntoIterator<Item = IVec3>,
    state: &mut LocalCoordinatePhysicsState,
    commands: &mut Commands,
) {
    for position in affected {
        if !interests.contains(identity, position) {
            remove_chunk_collider(position, state, commands);
            continue;
        }
        let Some(chunk) = local_coordinate.chunks.get(&position) else {
            remove_chunk_collider(position, state, commands);
            continue;
        };
        let source = collider_source(local_coordinate, position, chunk);
        let existing_chunk = state.chunks.get(&position);
        if existing_chunk.is_some_and(|chunk_state| chunk_state.source == source) {
            continue;
        }

        let Some(collider) = chunk_collider(local_coordinate, position, chunk) else {
            remove_chunk_collider(position, state, commands);
            continue;
        };

        if let Some(chunk_state) = state.chunks.get_mut(&position) {
            commands.entity(chunk_state.entity).try_insert(collider);
            chunk_state.source = source;
            continue;
        }

        let entity = commands
            .spawn((
                LocalCoordinateChunkCollider { position },
                collider,
                Transform::from_translation((position * CHUNK_EDGE_LENGTH).as_vec3()),
                ChildOf(owner),
            ))
            .id();
        state
            .chunks
            .insert(position, ChunkColliderState { entity, source });
    }
}

fn collider_source(
    local_coordinate: &LocalCoordinate,
    position: IVec3,
    chunk: &Chunk,
) -> ChunkColliderSource {
    ChunkColliderSource {
        content_revision: chunk.content_revision,
        neighbor_revisions: FACE_OFFSETS.map(|offset| {
            let neighbor = local_coordinate.chunks.get(&(position + offset));
            neighbor.map(|neighbor| neighbor.content_revision)
        }),
    }
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

fn chunk_collider(
    local_coordinate: &LocalCoordinate,
    chunk_position: IVec3,
    chunk: &Chunk,
) -> Option<Collider> {
    let (vertices, indices) = chunk_collider_mesh(local_coordinate, chunk_position, chunk);
    (!indices.is_empty()).then(|| Collider::trimesh(vertices, indices))
}

// Emits four independent vertices and two triangles for each exposed voxel face.
fn chunk_collider_mesh(
    local_coordinate: &LocalCoordinate,
    chunk_position: IVec3,
    chunk: &Chunk,
) -> (Vec<Vector>, Vec<[u32; 3]>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let chunk_origin = chunk_position * CHUNK_EDGE_LENGTH;

    for z in 0..CHUNK_EDGE_LENGTH {
        for y in 0..CHUNK_EDGE_LENGTH {
            for x in 0..CHUNK_EDGE_LENGTH {
                let local_position = IVec3::new(x, y, z);
                if !chunk.is_solid(local_position) {
                    continue;
                }

                for (face, offset) in FACE_OFFSETS.into_iter().enumerate() {
                    let neighbor_position = chunk_origin + local_position + offset;
                    if local_coordinate_is_solid(local_coordinate, neighbor_position) {
                        continue;
                    }
                    push_face(&mut vertices, &mut indices, local_position, face);
                }
            }
        }
    }

    (vertices, indices)
}

// Euclidean division keeps chunk-local coordinates nonnegative across the origin.
fn local_coordinate_is_solid(local_coordinate: &LocalCoordinate, position: IVec3) -> bool {
    let chunk_position = IVec3::new(
        position.x.div_euclid(CHUNK_EDGE_LENGTH),
        position.y.div_euclid(CHUNK_EDGE_LENGTH),
        position.z.div_euclid(CHUNK_EDGE_LENGTH),
    );
    let local_position = IVec3::new(
        position.x.rem_euclid(CHUNK_EDGE_LENGTH),
        position.y.rem_euclid(CHUNK_EDGE_LENGTH),
        position.z.rem_euclid(CHUNK_EDGE_LENGTH),
    );
    let chunk = local_coordinate.chunks.get(&chunk_position);
    chunk.is_some_and(|candidate| candidate.is_solid(local_position))
}

fn push_face(
    vertices: &mut Vec<Vector>,
    indices: &mut Vec<[u32; 3]>,
    position: IVec3,
    face: usize,
) {
    let x = position.x as f32;
    let y = position.y as f32;
    let z = position.z as f32;
    let corners = match face {
        0 => [
            [x + 1.0, y, z],
            [x + 1.0, y + 1.0, z],
            [x + 1.0, y + 1.0, z + 1.0],
            [x + 1.0, y, z + 1.0],
        ],
        1 => [
            [x, y, z],
            [x, y, z + 1.0],
            [x, y + 1.0, z + 1.0],
            [x, y + 1.0, z],
        ],
        2 => [
            [x, y + 1.0, z],
            [x, y + 1.0, z + 1.0],
            [x + 1.0, y + 1.0, z + 1.0],
            [x + 1.0, y + 1.0, z],
        ],
        3 => [
            [x, y, z],
            [x + 1.0, y, z],
            [x + 1.0, y, z + 1.0],
            [x, y, z + 1.0],
        ],
        4 => [
            [x, y, z + 1.0],
            [x + 1.0, y, z + 1.0],
            [x + 1.0, y + 1.0, z + 1.0],
            [x, y + 1.0, z + 1.0],
        ],
        5 => [
            [x, y, z],
            [x, y + 1.0, z],
            [x + 1.0, y + 1.0, z],
            [x + 1.0, y, z],
        ],
        _ => unreachable!("a cube has exactly six faces"),
    };
    let first = u32::try_from(vertices.len()).expect("one chunk collider fits in u32 indices");
    vertices.extend(corners.map(|corner| Vector::new(corner[0], corner[1], corner[2])));
    indices.extend([[first, first + 1, first + 2], [first, first + 2, first + 3]]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_coordinate::data::{PositionedAtomicVoxel, SOLID_VOXEL_ID};
    use avian3d::prelude::{ColliderHierarchyPlugin, ColliderOf, RigidBody};
    use bevy::prelude::App;

    fn solid(position: IVec3) -> PositionedAtomicVoxel {
        PositionedAtomicVoxel {
            position,
            voxel: SOLID_VOXEL_ID,
        }
    }

    #[test]
    fn collider_derivation_has_a_hard_per_update_budget() {
        let mut state = LocalCoordinatePhysicsState::default();
        for x in 0..(MAX_COLLIDER_CHUNKS_PER_UPDATE as i32 + 10) {
            enqueue_collider_chunk(&mut state, IVec3::new(x, 0, 0));
        }
        let admitted = take_pending_collider_chunks(&mut state);
        assert_eq!(admitted.len(), MAX_COLLIDER_CHUNKS_PER_UPDATE);
        assert_eq!(state.pending_chunks.len(), 10);
    }

    #[test]
    fn each_non_empty_chunk_gets_a_local_child_collider() {
        let mut app = physics_app();
        let first_position = IVec3::new(2, -1, 3);
        let second_position = IVec3::new(7, 0, -4);
        let coordinate = LocalCoordinate::from_voxels([
            solid(first_position * CHUNK_EDGE_LENGTH),
            solid(second_position * CHUNK_EDGE_LENGTH),
        ]);
        let owner = app.world_mut().spawn((coordinate, RigidBody::Static)).id();

        app.update();

        let state = app
            .world()
            .get::<LocalCoordinatePhysicsState>(owner)
            .unwrap();
        assert_eq!(state.chunks.len(), 2);
        for position in [first_position, second_position] {
            let collider_entity = state.chunks[&position].entity;
            assert!(app.world().get::<Collider>(collider_entity).is_some());
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
        }
    }

    #[test]
    fn updating_one_chunk_only_rebuilds_its_collider() {
        let mut app = physics_app();
        let second_position = IVec3::new(3, 0, 0);
        let owner = app
            .world_mut()
            .spawn(LocalCoordinate::from_voxels([
                solid(IVec3::ZERO),
                solid(second_position * CHUNK_EDGE_LENGTH),
            ]))
            .id();
        app.update();

        let first_entity = app
            .world()
            .get::<LocalCoordinatePhysicsState>(owner)
            .unwrap()
            .chunks[&IVec3::ZERO]
            .entity;
        let second_entity = app
            .world()
            .get::<LocalCoordinatePhysicsState>(owner)
            .unwrap()
            .chunks[&second_position]
            .entity;
        let first_tick = collider_change_tick(&app, first_entity);
        let second_tick = collider_change_tick(&app, second_entity);

        app.world_mut()
            .get_mut::<LocalCoordinate>(owner)
            .unwrap()
            .apply_voxel(solid(second_position * CHUNK_EDGE_LENGTH + IVec3::X));
        app.update();

        assert_eq!(collider_change_tick(&app, first_entity), first_tick);
        assert_ne!(collider_change_tick(&app, second_entity), second_tick);
    }

    #[test]
    fn adjacent_chunks_cull_their_shared_collider_faces() {
        let coordinate = LocalCoordinate::from_voxels([
            solid(IVec3::new(CHUNK_EDGE_LENGTH - 1, 0, 0)),
            solid(IVec3::new(CHUNK_EDGE_LENGTH, 0, 0)),
        ]);
        let (_, first_indices) =
            chunk_collider_mesh(&coordinate, IVec3::ZERO, &coordinate.chunks[&IVec3::ZERO]);
        assert_eq!(first_indices.len(), 10);
    }

    #[test]
    fn removing_a_chunk_despawns_only_its_collider() {
        let mut app = physics_app();
        let owner = app
            .world_mut()
            .spawn(LocalCoordinate::from_voxels([
                solid(IVec3::ZERO),
                solid(IVec3::new(CHUNK_EDGE_LENGTH, 0, 0)),
            ]))
            .id();
        app.update();
        let state = app
            .world()
            .get::<LocalCoordinatePhysicsState>(owner)
            .unwrap();
        let retained = state.chunks[&IVec3::ZERO].entity;
        let removed = state.chunks[&IVec3::X].entity;

        app.world_mut()
            .get_mut::<LocalCoordinate>(owner)
            .unwrap()
            .remove_chunk(IVec3::X);
        app.update();

        assert!(app.world().entities().contains(retained));
        assert!(!app.world().entities().contains(removed));
    }

    #[test]
    fn avian_attaches_chunk_colliders_to_the_owner_rigid_body() {
        let mut app = App::new();
        app.add_plugins((ColliderHierarchyPlugin, LocalCoordinatePhysicsPlugin));
        let owner = app
            .world_mut()
            .spawn((
                LocalCoordinate::from_voxels([solid(IVec3::ZERO)]),
                RigidBody::Static,
            ))
            .id();
        app.update();
        let collider = app
            .world()
            .get::<LocalCoordinatePhysicsState>(owner)
            .unwrap()
            .chunks[&IVec3::ZERO]
            .entity;
        assert_eq!(app.world().get::<ColliderOf>(collider).unwrap().body, owner);
    }

    #[test]
    fn removing_local_coordinate_state_despawns_its_chunk_colliders() {
        let mut app = physics_app();
        let owner = app
            .world_mut()
            .spawn(LocalCoordinate::from_voxels([solid(IVec3::ZERO)]))
            .id();
        app.update();
        let collider = app
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
        assert!(!app.world().entities().contains(collider));
    }

    fn physics_app() -> App {
        let mut app = App::new();
        app.add_plugins(LocalCoordinatePhysicsPlugin);
        app
    }

    fn collider_change_tick(app: &App, entity: Entity) -> bevy::ecs::change_detection::Tick {
        app.world()
            .entity(entity)
            .get_change_ticks::<Collider>()
            .unwrap()
            .changed
    }
}
