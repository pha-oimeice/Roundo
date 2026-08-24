use crate::local_coordinate::data::{
    CHUNK_EDGE_LENGTH, Chunk, LocalCoordinate, PositionedAtomicVoxel, SOLID_VOXEL_ID,
};
use bevy::prelude::{IVec3, Vec3};

impl LocalCoordinate {
    /// Applies a voxel delta and marks every affected chunk mesh dirty.
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

        if voxel.voxel == SOLID_VOXEL_ID {
            self.assign_color(voxel.position);
        } else {
            self.voxel_colors.remove(&voxel.position);
        }

        self.mark_dirty(chunk_position, local_position);
        self.changed_chunks.insert(chunk_position);
        true
    }

    /// Applies several updates while coalescing derived work through the dirty-chunk set.
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
        self.dirty_chunks.insert(chunk_position);
        self.changed_chunks.insert(chunk_position);
        true
    }

    /// Removes one complete chunk from this local coordinate.
    pub fn remove_chunk(&mut self, chunk_position: IVec3) -> bool {
        let Some(_) = self.chunks.remove(&chunk_position) else {
            return false;
        };

        self.voxel_colors
            .retain(|position, _| split_position(*position).0 != chunk_position);
        self.dirty_chunks.insert(chunk_position);
        self.mark_loaded_neighbors_dirty(chunk_position);
        self.changed_chunks.insert(chunk_position);
        self.rebuild_center_of_mass();
        true
    }

    pub(crate) fn replace_chunk(&mut self, chunk_position: IVec3, chunk: Chunk) {
        self.remove_chunk(chunk_position);
        self.chunks.insert(chunk_position, chunk);
        self.dirty_chunks.insert(chunk_position);
        self.mark_loaded_neighbors_dirty(chunk_position);
        self.changed_chunks.insert(chunk_position);
        let chunk_origin = chunk_position * CHUNK_EDGE_LENGTH;
        for z in 0..CHUNK_EDGE_LENGTH {
            for y in 0..CHUNK_EDGE_LENGTH {
                for x in 0..CHUNK_EDGE_LENGTH {
                    let local_position = IVec3::new(x, y, z);
                    if self.chunks[&chunk_position].is_solid(local_position) {
                        self.assign_color(chunk_origin + local_position);
                    }
                }
            }
        }
        self.rebuild_center_of_mass();
    }

    pub(crate) fn rebuild_dirty_chunks(&mut self) -> bool {
        self.rebuild_dirty_chunks_with_limit(usize::MAX) > 0
    }

    pub(crate) fn rebuild_dirty_chunks_with_limit(&mut self, limit: usize) -> usize {
        if self.dirty_chunks.is_empty() || limit == 0 {
            return 0;
        }

        let mut dirty_positions = self.dirty_chunks.iter().copied().collect::<Vec<_>>();
        dirty_positions.sort_by_key(|position| (position.x, position.y, position.z));
        dirty_positions.truncate(limit);
        for position in &dirty_positions {
            self.dirty_chunks.remove(position);
        }
        if dirty_positions.is_empty() {
            return 0;
        }
        let processed_count = dirty_positions.len();

        let next_revision = self.geometry_revision.wrapping_add(1);
        let (chunks, voxel_colors) = (&mut self.chunks, &self.voxel_colors);

        for chunk_position in dirty_positions {
            let Some(mut chunk) = chunks.remove(&chunk_position) else {
                continue;
            };
            chunk.rebuild_triangles(
                chunk_position,
                |position| voxel_colors.get(&position).copied().unwrap_or([1.0; 4]),
                |position| {
                    let (neighbor_chunk_position, neighbor_local_position) =
                        split_position(position);
                    chunks
                        .get(&neighbor_chunk_position)
                        .is_some_and(|neighbor| neighbor.is_solid(neighbor_local_position))
                },
            );
            chunk.geometry_revision = next_revision;
            chunks.insert(chunk_position, chunk);
        }

        self.geometry_revision = next_revision;
        processed_count
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

    fn assign_color(&mut self, voxel_position: IVec3) {
        if self.voxel_colors.contains_key(&voxel_position) {
            return;
        }

        const MAX_UNIQUE_RGB_COLORS: u128 = 1_u128 << 72;
        assert!(
            self.next_color < MAX_UNIQUE_RGB_COLORS,
            "local coordinate exhausted its unique RGB color space"
        );

        let color_id = self.next_color;
        self.next_color += 1;
        self.voxel_colors
            .insert(voxel_position, unique_color(color_id));
    }

    fn mark_dirty(&mut self, chunk_position: IVec3, local_position: IVec3) {
        self.dirty_chunks.insert(chunk_position);

        if local_position.x == 0 {
            self.mark_solid_neighbor_dirty(
                chunk_position + IVec3::NEG_X,
                IVec3::new(CHUNK_EDGE_LENGTH - 1, local_position.y, local_position.z),
            );
        }
        if local_position.x == CHUNK_EDGE_LENGTH - 1 {
            self.mark_solid_neighbor_dirty(
                chunk_position + IVec3::X,
                IVec3::new(0, local_position.y, local_position.z),
            );
        }
        if local_position.y == 0 {
            self.mark_solid_neighbor_dirty(
                chunk_position + IVec3::NEG_Y,
                IVec3::new(local_position.x, CHUNK_EDGE_LENGTH - 1, local_position.z),
            );
        }
        if local_position.y == CHUNK_EDGE_LENGTH - 1 {
            self.mark_solid_neighbor_dirty(
                chunk_position + IVec3::Y,
                IVec3::new(local_position.x, 0, local_position.z),
            );
        }
        if local_position.z == 0 {
            self.mark_solid_neighbor_dirty(
                chunk_position + IVec3::NEG_Z,
                IVec3::new(local_position.x, local_position.y, CHUNK_EDGE_LENGTH - 1),
            );
        }
        if local_position.z == CHUNK_EDGE_LENGTH - 1 {
            self.mark_solid_neighbor_dirty(
                chunk_position + IVec3::Z,
                IVec3::new(local_position.x, local_position.y, 0),
            );
        }
    }

    fn mark_solid_neighbor_dirty(&mut self, chunk_position: IVec3, local_position: IVec3) {
        if self
            .chunks
            .get(&chunk_position)
            .is_some_and(|chunk| chunk.is_solid(local_position))
        {
            self.dirty_chunks.insert(chunk_position);
        }
    }

    fn mark_loaded_neighbors_dirty(&mut self, chunk_position: IVec3) {
        for offset in [
            IVec3::X,
            IVec3::NEG_X,
            IVec3::Y,
            IVec3::NEG_Y,
            IVec3::Z,
            IVec3::NEG_Z,
        ] {
            self.mark_loaded_chunk_dirty(chunk_position + offset);
        }
    }

    fn mark_loaded_chunk_dirty(&mut self, chunk_position: IVec3) {
        if self.chunks.contains_key(&chunk_position) {
            self.dirty_chunks.insert(chunk_position);
        }
    }
}

fn unique_color(color_id: u128) -> [f32; 4] {
    const RGB_MASK: u128 = (1_u128 << 24) - 1;
    const RGB_ID_MASK: u128 = (1_u128 << 72) - 1;
    const RGB_SCALE: f32 = 16_777_216.0;
    const COLOR_PERMUTATION: u128 = 0x9e37_79b9_7f4a_7c15;
    const COLOR_OFFSET: u128 = 0x4cf5_ad43_2745_937f;

    // Odd multiplication and addition are bijective modulo 2^72, so RGB stays unique.
    let shuffled_id = color_id
        .wrapping_mul(COLOR_PERMUTATION)
        .wrapping_add(COLOR_OFFSET)
        & RGB_ID_MASK;

    [
        (shuffled_id & RGB_MASK) as f32 / RGB_SCALE,
        ((shuffled_id >> 24) & RGB_MASK) as f32 / RGB_SCALE,
        ((shuffled_id >> 48) & RGB_MASK) as f32 / RGB_SCALE,
        1.0,
    ]
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
