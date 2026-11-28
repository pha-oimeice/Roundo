//! Bounded local PCG work shared by render and collider interests.
use super::*;
use bevy::prelude::{Entity, Insert, On};

impl Plugin for LocalCoordinateServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((LocalCoordinateBasePlugin, LocalCoordinatePhysicsPlugin))
            .init_resource::<crate::VirtualChunkEnvironmentMap>()
            .insert_resource(GeneratedLocalCoordinates(
                self.generated_coordinates.clone(),
            ))
            .init_resource::<LocalCoordinateServerWorld>()
            .add_observer(invalidate_installed_coordinate)
            .configure_sets(
                Update,
                (
                    LocalCoordinateServerSet::Prepare,
                    LocalCoordinateSet::RebuildIndex,
                    LocalCoordinateServerSet::Commit,
                    LocalCoordinatePhysicsSet::Sync,
                )
                    .chain(),
            )
            .add_systems(Startup, spawn_generated_local_coordinates)
            .add_systems(
                Update,
                (prepare_generated_chunks, commit_generated_chunks)
                    .chain()
                    .in_set(LocalCoordinateServerSet::Prepare),
            )
            .add_systems(
                Update,
                super::super::base::clear_chunk_changes.in_set(LocalCoordinateServerSet::Commit),
            );
    }
}

#[derive(Resource)]
struct GeneratedLocalCoordinates(Vec<PcgLocalCoordinate>);

#[derive(Default)]
pub(super) struct GenerationPlan {
    desired: HashSet<ChunkCoordinate>,
    physics: HashSet<ChunkCoordinate>,
    view: Option<PcgRenderView>,
    pending_physics: VecDeque<ChunkCoordinate>,
    pending_render: VecDeque<ChunkCoordinate>,
    evicting: VecDeque<ChunkCoordinate>,
}

impl GenerationPlan {
    fn reconcile(
        &mut self,
        physics: HashSet<ChunkCoordinate>,
        view: Option<PcgRenderView>,
        loaded: &HashMap<ChunkCoordinate, GeneratedChunk>,
    ) {
        if self.physics == physics && self.view == view {
            return;
        }
        self.view = view;
        let desired = physics
            .iter()
            .copied()
            .chain(view.into_iter().flat_map(PcgRenderView::positions))
            .collect::<HashSet<_>>();
        if self.physics == physics && self.desired == desired {
            return;
        }
        let (mut pending_physics, mut pending_render): (Vec<_>, Vec<_>) = desired
            .iter()
            .copied()
            .filter(|position| !loaded.contains_key(position))
            .partition(|position| physics.contains(position));
        pending_physics.sort_unstable();
        if let Some(view) = view {
            pending_render.sort_unstable_by(|&a, &b| {
                view.distance_squared(a)
                    .total_cmp(&view.distance_squared(b))
                    .then(a.cmp(&b))
            });
        }
        let mut evicting = loaded
            .keys()
            .filter(|position| !desired.contains(*position))
            .copied()
            .collect::<Vec<_>>();
        evicting.sort_unstable();
        self.desired = desired;
        self.physics = physics;
        self.pending_physics = pending_physics.into();
        self.pending_render = pending_render.into();
        self.evicting = evicting.into();
    }

    fn retry(&mut self, position: ChunkCoordinate) {
        if self.physics.contains(&position) {
            self.pending_physics.push_front(position);
        } else {
            self.pending_render.push_front(position);
        }
    }
}

fn invalidate_installed_coordinate(
    event: On<Insert<LocalCoordinate>>,
    coordinates: Query<&PcgLocalCoordinate>,
    mut world: ResMut<LocalCoordinateServerWorld>,
) {
    if let Ok(generated) = coordinates.get(event.entity) {
        world.loaded_chunks.remove(&generated.id);
        world.generation_plans.remove(&generated.id);
        world
            .generated_revisions
            .retain(|id, _| id.local_coordinate_id != generated.id);
    }
}

// Creates one static authoritative coordinate root per configured generator.
fn spawn_generated_local_coordinates(
    mut commands: Commands,
    coordinates: Res<GeneratedLocalCoordinates>,
) {
    for coordinate in &coordinates.0 {
        commands.spawn((
            LocalCoordinate::default(),
            LocalCoordinateIdentity(coordinate.id),
            coordinate.clone(),
            RigidBody::Static,
            LocalCoordinateTransform::default(),
        ));
    }
}

fn prepare_generated_chunks(
    mut world: ResMut<LocalCoordinateServerWorld>,
    mut physics_interests: ResMut<LocalCoordinatePhysicsInterests>,
    demands: Option<Res<super::super::world_interaction::Runtime>>,
    mut coordinates: Query<(
        Entity,
        &PcgLocalCoordinate,
        &mut LocalCoordinate,
        Option<&LocalCoordinateIdentity>,
        Option<&PcgRenderView>,
    )>,
) {
    let mut active = HashSet::new();
    let mut desired_physics = HashMap::new();
    let mut budget = MAX_CHUNKS_GENERATED_PER_TICK
        .min(MAX_CHUNK_GENERATION_JOBS_IN_FLIGHT.saturating_sub(world.generation_jobs.len()));
    let mut physics_budget = demands.as_ref().map_or(budget, |demands| {
        demands.physics_budget().generate_per_tick()
    });
    let worker = world.generation_worker.clone();
    for (entity, generated, mut coordinate, identity, view) in &mut coordinates {
        active.insert(generated.id);
        let requested = demands
            .as_ref()
            .map(|runtime| runtime.regions(entity, &coordinate, identity))
            .unwrap_or_default()
            .into_iter()
            .flat_map(crate::PhysicsDemandRegion::positions)
            .collect::<HashSet<_>>();
        let physics = requested
            .iter()
            .map(|position| position.to_array().map(i64::from))
            .collect::<HashSet<_>>();
        desired_physics.insert(generated.id, requested);
        let view = view.map(|view| match identity {
            Some(identity) if identity.0 == generated.id => {
                view.for_installation(&coordinate, identity.0)
            }
            _ => PcgRenderView::default(),
        });
        let LocalCoordinateServerWorld {
            loaded_chunks,
            generation_plans,
            generation_jobs,
            generated_revisions,
            ..
        } = &mut *world;
        let loaded = loaded_chunks.entry(generated.id).or_default();
        let plan = generation_plans.entry(generated.id).or_default();
        plan.reconcile(physics, view, loaded);
        for position in plan
            .evicting
            .drain(..plan.evicting.len().min(MAX_CHUNKS_EVICTED_PER_TICK))
        {
            let id = ChunkId {
                local_coordinate_id: generated.id,
                coordinate: position,
            };
            let local = super::super::pcg::local_chunk_position(position)
                .expect("loaded chunk has a local position");
            if coordinate
                .chunks
                .get(&local)
                .map(|chunk| chunk.content_revision)
                != generated_revisions.get(&id).copied()
            {
                continue;
            }
            loaded.remove(&position);
            generated_revisions.remove(&id);
            remove_generated_chunk(&mut coordinate, position);
        }
        while budget > 0 {
            let physics = physics_budget > 0 && !plan.pending_physics.is_empty();
            let pending = if physics {
                &mut plan.pending_physics
            } else {
                &mut plan.pending_render
            };
            let Some(position) = pending.pop_front() else {
                break;
            };
            if loaded.contains_key(&position) {
                continue;
            }
            let id = ChunkId {
                local_coordinate_id: generated.id,
                coordinate: position,
            };
            if !generation_jobs.insert(id) {
                continue;
            }
            if worker
                .submit(ChunkGenerationJob {
                    id,
                    generator: generated.generator.clone(),
                    installation: coordinate.incarnation,
                })
                .is_err()
            {
                generation_jobs.remove(&id);
                pending.push_front(position);
                log::error!("local chunk generation worker is closed");
                break;
            }
            budget -= 1;
            if physics {
                physics_budget -= 1;
            }
        }
    }
    world.loaded_chunks.retain(|id, _| active.contains(id));
    world.generation_plans.retain(|id, _| active.contains(id));
    world
        .generated_revisions
        .retain(|id, _| active.contains(&id.local_coordinate_id));
    if !physics_interests.matches(&desired_physics) {
        physics_interests.replace(desired_physics);
    }
}
// Publishes only results that still correspond to a pending and desired chunk.
fn commit_generated_chunks(
    voxels: Option<Res<crate::AtomicVoxelRegistry>>,
    mut world: ResMut<LocalCoordinateServerWorld>,
    mut local_coordinates: Query<(&PcgLocalCoordinate, &mut LocalCoordinate)>,
) {
    let worker = world.generation_worker.clone();
    for _ in 0..MAX_CHUNKS_GENERATED_PER_TICK {
        let Some(result) = worker.try_receive() else {
            break;
        };
        let id = result.id;
        let was_pending = world.generation_jobs.remove(&id);
        let generation_plan = world.generation_plans.get(&id.local_coordinate_id);
        let still_desired =
            generation_plan.is_some_and(|plan| plan.desired.contains(&id.coordinate));
        let loaded_chunks = world.loaded_chunks.get(&id.local_coordinate_id);
        let already_loaded =
            loaded_chunks.is_some_and(|chunks| chunks.contains_key(&id.coordinate));
        if !was_pending || !still_desired || already_loaded {
            continue;
        }
        let Some((_, mut local_coordinate)) = local_coordinates
            .iter_mut()
            .find(|(generated, _)| generated.id == id.local_coordinate_id)
        else {
            continue;
        };
        let builtin;
        let voxels = if let Some(voxels) = voxels.as_deref() {
            voxels
        } else {
            builtin = crate::AtomicVoxelRegistry::builtin();
            &builtin
        };
        if result.installation != local_coordinate.incarnation {
            if let Some(plan) = world.generation_plans.get_mut(&id.local_coordinate_id) {
                plan.retry(id.coordinate);
            }
            continue;
        }
        if !replace_generated_chunk(&mut local_coordinate, id.coordinate, &result.chunk, voxels) {
            continue;
        }
        let position = super::super::pcg::local_chunk_position(id.coordinate)
            .expect("installed generated chunk fits local coordinates");
        world
            .generated_revisions
            .insert(id, local_coordinate.chunks[&position].content_revision);
        world
            .loaded_chunks
            .entry(id.local_coordinate_id)
            .or_default()
            .insert(id.coordinate, result.chunk);
    }
}
