use crate::local_coordinate::data::{
    CHUNK_EDGE_LENGTH, Chunk, EMPTY_VOXEL_ID, LocalCoordinate, LocalCoordinateChunkView,
    PositionedAtomicVoxel,
};
use bevy::prelude::{IVec3, Vec3};

impl LocalCoordinate {
    /// Reads one voxel in local-coordinate space without exposing Chunk storage.
    pub fn voxel(&self, position: IVec3) -> Option<crate::AtomicVoxel> {
        let (chunk_position, local_position) = split_position(position);
        let chunk = self.chunks.get(&chunk_position)?;
        chunk.voxel(local_position)
    }

    /// Observes renderable loaded Chunks without exposing the backing map or Chunk layout.
    pub fn loaded_chunk_views(&self) -> impl Iterator<Item = LocalCoordinateChunkView<'_>> {
        self.chunks.iter().filter_map(|(position, chunk)| {
            chunk.compressed_svo().map(|svo| LocalCoordinateChunkView {
                position: *position,
                revision: chunk.content_revision,
                svo,
            })
        })
    }

    /// Applies one authoritative voxel delta.
    pub fn apply_voxel(&mut self, voxel: PositionedAtomicVoxel) -> bool {
        let changed = self.apply_voxel_without_center_of_mass(voxel);
        if changed {
            self.refresh_center_of_mass();
        }
        changed
    }

    fn apply_voxel_without_center_of_mass(&mut self, voxel: PositionedAtomicVoxel) -> bool {
        let (chunk_position, local_position) = split_position(voxel.position);
        let previous_count = self
            .chunks
            .get(&chunk_position)
            .map_or(0, |chunk| chunk.solid_count);
        let changed = if voxel.voxel != EMPTY_VOXEL_ID {
            self.mark_chunk_loaded(chunk_position);
            self.chunks
                .get_mut(&chunk_position)
                .expect("loaded chunk must exist")
                .set_voxel(local_position, voxel.voxel)
        } else {
            let Some(chunk) = self.chunks.get_mut(&chunk_position) else {
                return false;
            };
            chunk.set_voxel(local_position, voxel.voxel)
        };

        if !changed {
            return false;
        }

        let current_count = self.chunks[&chunk_position].solid_count;
        self.adjust_chunk_weight(chunk_position, previous_count, current_count);
        self.changed_chunks.insert(chunk_position);
        self.physics_dirty_chunks.insert(chunk_position);
        self.content_revision = self.content_revision.wrapping_add(1);
        true
    }

    /// Applies several authoritative voxel updates and rebuilds aggregate state once.
    pub fn apply_voxels(
        &mut self,
        voxels: impl IntoIterator<Item = PositionedAtomicVoxel>,
    ) -> bool {
        let mut changed = false;

        for voxel in voxels {
            changed |= self.apply_voxel_without_center_of_mass(voxel);
        }
        if changed {
            self.refresh_center_of_mass();
        }

        changed
    }

    /// Builds a complete finite coordinate and treats its outer chunk shell as loaded air.
    pub fn from_voxels(voxels: impl IntoIterator<Item = PositionedAtomicVoxel>) -> Self {
        let mut local_coordinate = Self::default();
        local_coordinate.apply_voxels(voxels);
        local_coordinate
    }

    /// Marks one chunk's primitive data as loaded, even when the chunk is empty.
    pub fn mark_chunk_loaded(&mut self, chunk_position: IVec3) -> bool {
        if self.chunks.contains_key(&chunk_position) {
            return false;
        }

        self.chunks.insert(chunk_position, Chunk::default());
        self.changed_chunks.insert(chunk_position);
        self.physics_dirty_chunks.insert(chunk_position);
        self.content_revision = self.content_revision.wrapping_add(1);
        true
    }

    /// Removes one complete chunk from this local coordinate.
    pub fn remove_chunk(&mut self, chunk_position: IVec3) -> bool {
        let Some(removed) = self.chunks.remove(&chunk_position) else {
            return false;
        };

        self.adjust_chunk_weight(chunk_position, removed.solid_count, 0);
        self.changed_chunks.insert(chunk_position);
        self.physics_dirty_chunks.insert(chunk_position);
        self.content_revision = self.content_revision.wrapping_add(1);
        self.refresh_center_of_mass();
        true
    }

    pub(crate) fn replace_chunk(&mut self, chunk_position: IVec3, mut chunk: Chunk) {
        let current_chunk = self.chunks.get(&chunk_position);
        let next_chunk_revision =
            current_chunk.map_or(1, |current| current.content_revision.wrapping_add(1));
        let previous_count = current_chunk.map_or(0, |chunk| chunk.solid_count);
        self.adjust_chunk_weight(chunk_position, previous_count, chunk.solid_count);
        chunk.content_revision = next_chunk_revision;
        self.chunks.insert(chunk_position, chunk);
        self.changed_chunks.insert(chunk_position);
        self.physics_dirty_chunks.insert(chunk_position);
        self.content_revision = self.content_revision.wrapping_add(1);
        self.refresh_center_of_mass();
    }

    // The common Chunk volume factor cancels in the weighted average. Keep
    // integer voxel counts and f64 moments instead of rescanning all Chunks
    // after every installation, removal, or individual voxel edit.
    fn adjust_chunk_weight(&mut self, position: IVec3, previous: usize, current: usize) {
        let delta = current as f64 - previous as f64;
        let center = (position.as_dvec3() + 0.5) * f64::from(CHUNK_EDGE_LENGTH);
        for (moment, value) in self
            .weighted_chunk_centers
            .iter_mut()
            .zip(center.to_array())
        {
            *moment += value * delta;
        }
        self.total_solid_voxels = self.total_solid_voxels - previous as u64 + current as u64;
    }

    fn refresh_center_of_mass(&mut self) {
        if self.total_solid_voxels == 0 {
            self.weighted_chunk_centers = [0.0; 3];
            self.center_of_mass = Vec3::ZERO;
        } else {
            self.center_of_mass = Vec3::from_array(
                self.weighted_chunk_centers
                    .map(|moment| (moment / self.total_solid_voxels as f64) as f32),
            );
        }
    }
}

#[cfg(test)]
mod tests;

fn split_position(position: IVec3) -> (IVec3, IVec3) {
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

    (chunk_position, local_position)
}
