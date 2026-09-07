use crate::local_coordinate::data::{
    CHUNK_EDGE_LENGTH, Chunk, LocalCoordinate, PositionedAtomicVoxel, SOLID_VOXEL_ID,
};
use bevy::prelude::{IVec3, Vec3};

impl LocalCoordinate {
    /// Applies one authoritative voxel delta.
    pub fn apply_voxel(&mut self, voxel: PositionedAtomicVoxel) -> bool {
        let changed = self.apply_voxel_without_center_of_mass(voxel);
        if changed {
            self.rebuild_center_of_mass();
        }
        changed
    }

    fn apply_voxel_without_center_of_mass(&mut self, voxel: PositionedAtomicVoxel) -> bool {
        let (chunk_position, local_position) = split_position(voxel.position);
        let changed = if voxel.voxel == SOLID_VOXEL_ID {
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
            self.rebuild_center_of_mass();
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
        let Some(_) = self.chunks.remove(&chunk_position) else {
            return false;
        };

        self.changed_chunks.insert(chunk_position);
        self.physics_dirty_chunks.insert(chunk_position);
        self.content_revision = self.content_revision.wrapping_add(1);
        self.rebuild_center_of_mass();
        true
    }

    pub(crate) fn replace_chunk(&mut self, chunk_position: IVec3, mut chunk: Chunk) {
        let next_chunk_revision = self
            .chunks
            .get(&chunk_position)
            .map_or(1, |current| current.content_revision.wrapping_add(1));
        chunk.content_revision = next_chunk_revision;
        self.chunks.insert(chunk_position, chunk);
        self.changed_chunks.insert(chunk_position);
        self.physics_dirty_chunks.insert(chunk_position);
        self.content_revision = self.content_revision.wrapping_add(1);
        self.rebuild_center_of_mass();
    }

    fn rebuild_center_of_mass(&mut self) {
        let chunk_volume = f64::from(CHUNK_EDGE_LENGTH).powi(3);
        let mut weighted_center = [0.0_f64; 3];
        let mut total_fill = 0.0_f64;

        for (chunk_position, chunk) in &self.chunks {
            let fill = chunk.solid_count as f64 / chunk_volume;
            let center = (chunk_position.as_dvec3() + 0.5) * f64::from(CHUNK_EDGE_LENGTH);
            weighted_center[0] += center.x * fill;
            weighted_center[1] += center.y * fill;
            weighted_center[2] += center.z * fill;
            total_fill += fill;
        }

        self.center_of_mass = if total_fill > 0.0 {
            Vec3::new(
                (weighted_center[0] / total_fill) as f32,
                (weighted_center[1] / total_fill) as f32,
                (weighted_center[2] / total_fill) as f32,
            )
        } else {
            Vec3::ZERO
        };
    }
}

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
