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
    App, ChildOf, Commands, Component, DetectChanges, DetectChangesMut, Entity, IVec3,
    IntoScheduleConfigs, Plugin, Query, Res, Resource, SystemSet, Transform, Update, Without,
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
pub struct LocalCoordinatePhysicsInterests {
    restricted: bool,
    chunks: HashMap<LocalCoordinateId, HashSet<IVec3>>,
}

impl LocalCoordinatePhysicsInterests {
    /// Creates an exact empty interest set. Clients install this before their
    /// first streamed prediction anchor so rendering-only chunks never obtain a
    /// transient collider during startup.
    pub fn restricted_empty() -> Self {
        Self {
            restricted: true,
            chunks: HashMap::new(),
        }
    }

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
pub(super) struct LocalCoordinatePhysicsState {
    installation: Option<super::CoordinateIncarnation>,
    sources: HashMap<IVec3, ChunkColliderSource>,
    chunks: HashMap<IVec3, ChunkColliderState>,
    pending_chunks: VecDeque<IVec3>,
    pending_set: HashSet<IVec3>,
}

impl LocalCoordinatePhysicsState {
    pub(super) fn region_ready(
        &self,
        coordinate: &LocalCoordinate,
        region: super::PhysicsDemandRegion,
        colliders: &Query<(), bevy::prelude::With<Collider>>,
    ) -> bool {
        self.installation == coordinate.incarnation
            && region.positions().all(|position| {
                let Some(chunk) = coordinate.chunks.get(&position) else {
                    return false;
                };
                self.sources.get(&position) == Some(&collider_source(coordinate, position, chunk))
                    && self
                        .chunks
                        .get(&position)
                        .is_none_or(|state| colliders.contains(state.entity))
            })
    }
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
    demands: Option<Res<super::world_interaction::Runtime>>,
    mut local_coordinates: Query<(
        Entity,
        Option<&LocalCoordinateIdentity>,
        &mut LocalCoordinate,
        Option<&mut LocalCoordinatePhysicsState>,
    )>,
) {
    let mut local_budget = demands
        .as_ref()
        .map(|runtime| runtime.physics_budget().derive_per_tick())
        .unwrap_or(MAX_COLLIDER_CHUNKS_PER_UPDATE);
    for (owner, identity, mut local_coordinate, physics_state) in &mut local_coordinates {
        let requested = demands
            .as_ref()
            .map(|runtime| runtime.regions(owner, &local_coordinate, identity))
            .unwrap_or_default()
            .into_iter()
            .flat_map(super::PhysicsDemandRegion::positions)
            .collect::<HashSet<_>>();
        let identity = identity.map(|identity| identity.0);
        let changed = std::mem::take(
            &mut local_coordinate
                .bypass_change_detection()
                .physics_dirty_chunks,
        );
        if let Some(mut physics_state) = physics_state {
            if physics_state.installation != local_coordinate.incarnation {
                for state in physics_state.chunks.values() {
                    commands.entity(state.entity).try_despawn();
                }
                *physics_state = LocalCoordinatePhysicsState::default();
                physics_state.installation = local_coordinate.incarnation;
                for position in local_coordinate.chunks.keys().copied() {
                    if interests.contains(identity, position) || requested.contains(&position) {
                        enqueue_collider_chunk(&mut physics_state, position);
                    }
                }
            }
            for position in requested.iter().copied() {
                if local_coordinate.chunks.get(&position).is_some_and(|chunk| {
                    physics_state.sources.get(&position)
                        != Some(&collider_source(&local_coordinate, position, chunk))
                }) {
                    enqueue_collider_chunk(&mut physics_state, position);
                }
            }
            if interests.is_changed()
                || demands.as_ref().is_some_and(|runtime| runtime.is_changed())
            {
                for position in physics_state.sources.keys().copied().collect::<Vec<_>>() {
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
                    || requested.contains(&position)
                    || physics_state.sources.contains_key(&position)
                {
                    enqueue_collider_chunk(&mut physics_state, position);
                }
            }
            let affected = take_pending_collider_chunks_with_budget(
                &mut physics_state,
                if requested.is_empty() {
                    MAX_COLLIDER_CHUNKS_PER_UPDATE
                } else {
                    local_budget
                },
            );
            if !requested.is_empty() {
                local_budget = local_budget.saturating_sub(affected.len());
            }
            sync_physics_state(
                owner,
                identity,
                &interests,
                &requested,
                &local_coordinate,
                affected,
                &mut physics_state,
                &mut commands,
            );
        } else {
            let mut physics_state = LocalCoordinatePhysicsState {
                installation: local_coordinate.incarnation,
                ..Default::default()
            };
            for position in local_coordinate.chunks.keys().copied() {
                if interests.contains(identity, position) || requested.contains(&position) {
                    enqueue_collider_chunk(&mut physics_state, position);
                }
            }
            let affected = take_pending_collider_chunks_with_budget(
                &mut physics_state,
                if requested.is_empty() {
                    MAX_COLLIDER_CHUNKS_PER_UPDATE
                } else {
                    local_budget
                },
            );
            if !requested.is_empty() {
                local_budget = local_budget.saturating_sub(affected.len());
            }
            sync_physics_state(
                owner,
                identity,
                &interests,
                &requested,
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

#[cfg(test)]
fn take_pending_collider_chunks(state: &mut LocalCoordinatePhysicsState) -> Vec<IVec3> {
    take_pending_collider_chunks_with_budget(state, MAX_COLLIDER_CHUNKS_PER_UPDATE)
}
fn take_pending_collider_chunks_with_budget(
    state: &mut LocalCoordinatePhysicsState,
    budget: usize,
) -> Vec<IVec3> {
    let count = state
        .pending_chunks
        .len()
        .min(budget)
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
    requested: &HashSet<IVec3>,
    local_coordinate: &LocalCoordinate,
    affected: impl IntoIterator<Item = IVec3>,
    state: &mut LocalCoordinatePhysicsState,
    commands: &mut Commands,
) {
    for position in affected {
        if !interests.contains(identity, position) && !requested.contains(&position) {
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
            state.sources.insert(position, source);
            continue;
        }

        let Some(collider) = chunk_collider(local_coordinate, position, chunk) else {
            remove_chunk_collider(position, state, commands);
            state.sources.insert(position, source);
            continue;
        };
        state.sources.insert(position, source);

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
    state.sources.remove(&position);
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
mod tests;
