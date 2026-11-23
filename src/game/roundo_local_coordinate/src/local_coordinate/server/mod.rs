//! Procedural coordinates driven by physics demands in the same World.
mod generation;
mod systems;

use crate::local_coordinate::{
    attributes::LocalCoordinateTransform,
    base::{LocalCoordinateBasePlugin, LocalCoordinateSet},
    data::LocalCoordinate,
    pcg::{remove_generated_chunk, replace_generated_chunk},
    physics::{
        LocalCoordinatePhysicsInterests, LocalCoordinatePhysicsPlugin, LocalCoordinatePhysicsSet,
    },
};
use crate::{
    ChunkCoordinate, ChunkGenerator, ChunkId, GeneratedChunk, LocalCoordinateId,
    LocalCoordinateIdentity, SuperflatGenerator,
};
use avian3d::prelude::RigidBody;
use bevy::prelude::{
    App, Commands, Component, IntoScheduleConfigs, Plugin, Query, Res, ResMut, Resource, Startup,
    SystemSet, Update,
};
use generation::{ChunkGenerationJob, ChunkGenerationWorker};
use roundo_contracts::SceneId;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};
use systems::GenerationPlan;

pub const DEFAULT_PCG_LOCAL_COORDINATE_ID: LocalCoordinateId = LocalCoordinateId(1);
const MAX_CHUNKS_GENERATED_PER_TICK: usize = 16;
const MAX_CHUNK_GENERATION_JOBS_IN_FLIGHT: usize = 32;
const MAX_CHUNKS_EVICTED_PER_TICK: usize = 32;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum LocalCoordinateServerSet {
    Prepare,
    Commit,
}

/// Installs generated coordinates, bounded local generation.
///
#[derive(Clone)]
pub struct LocalCoordinateServerPlugin {
    generated_coordinates: Vec<PcgLocalCoordinate>,
}

impl LocalCoordinateServerPlugin {
    /// Creates a plugin with one scene-S1 superflat coordinate at height zero.
    pub fn new() -> Self {
        Self::with_generator(SuperflatGenerator::default())
    }

    /// Creates a plugin whose only generated coordinate uses `generator`.
    pub fn with_generator(generator: impl ChunkGenerator + Send + Sync + 'static) -> Self {
        Self {
            generated_coordinates: vec![PcgLocalCoordinate::new(
                DEFAULT_PCG_LOCAL_COORDINATE_ID,
                generator,
            )],
        }
    }

    /// Adds or replaces a generated coordinate configuration by identity.
    ///
    /// Replacement preserves the existing coordinate's scene selection; a new
    /// entry defaults to [`SceneId::S1`]. This only configures future app setup.
    pub fn with_generated_coordinate(
        mut self,
        local_coordinate_id: LocalCoordinateId,
        generator: impl ChunkGenerator + Send + Sync + 'static,
    ) -> Self {
        if let Some(existing) = self
            .generated_coordinates
            .iter_mut()
            .find(|coordinate| coordinate.id == local_coordinate_id)
        {
            existing.generator = Arc::new(generator);
        } else {
            self.generated_coordinates
                .push(PcgLocalCoordinate::new(local_coordinate_id, generator));
        }
        self
    }
}

impl Default for LocalCoordinateServerPlugin {
    fn default() -> Self {
        Self::new()
    }
}

type WorldChunkGenerator = Arc<dyn ChunkGenerator + Send + Sync>;

/// Marks an authoritative coordinate backed by deterministic procedural generation.
#[derive(Component, Clone)]
pub struct PcgLocalCoordinate {
    pub id: LocalCoordinateId,
    generator: WorldChunkGenerator,
    pub scene_id: SceneId,
}

impl PcgLocalCoordinate {
    /// Creates a generated coordinate in [`SceneId::S1`].
    pub fn new(
        id: LocalCoordinateId,
        generator: impl ChunkGenerator + Send + Sync + 'static,
    ) -> Self {
        Self {
            id,
            generator: Arc::new(generator),
            scene_id: SceneId::S1,
        }
    }

    /// Assigns the scene whose observers may request this coordinate.
    pub const fn in_scene(mut self, scene_id: SceneId) -> Self {
        self.scene_id = scene_id;
        self
    }
}

/// Local procedural generation state. Edited chunks are never evicted.
#[derive(Resource)]
pub struct LocalCoordinateServerWorld {
    loaded_chunks: HashMap<LocalCoordinateId, HashMap<ChunkCoordinate, GeneratedChunk>>,
    generation_plans: HashMap<LocalCoordinateId, GenerationPlan>,
    generation_jobs: HashSet<ChunkId>,
    generation_worker: ChunkGenerationWorker,
    generated_revisions: HashMap<ChunkId, u64>,
}

impl Default for LocalCoordinateServerWorld {
    fn default() -> Self {
        Self {
            loaded_chunks: HashMap::new(),
            generation_plans: HashMap::new(),
            generation_jobs: HashSet::new(),
            generation_worker: ChunkGenerationWorker::spawn(),
            generated_revisions: HashMap::new(),
        }
    }
}

impl LocalCoordinateServerWorld {
    /// Borrows a retained generated chunk, or returns `None` if not loaded.
    pub fn chunk(
        &self,
        local_coordinate_id: LocalCoordinateId,
        coordinate: ChunkCoordinate,
    ) -> Option<&GeneratedChunk> {
        let chunks = self.loaded_chunks.get(&local_coordinate_id)?;
        return chunks.get(&coordinate);
    }

    /// Returns the number of retained generated chunks for one coordinate.
    pub fn loaded_chunk_count(&self, local_coordinate_id: LocalCoordinateId) -> usize {
        let chunks = self.loaded_chunks.get(&local_coordinate_id);
        chunks.map_or(0, HashMap::len)
    }

    /// Returns the number of coordinate IDs with generation bookkeeping entries.
    ///
    /// An entry may currently contain zero loaded chunks.
    pub fn coordinate_count(&self) -> usize {
        self.loaded_chunks.len()
    }
}
