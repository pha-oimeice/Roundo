use crate::local_coordinate::data::{AtomicVoxel, CHUNK_EDGE_LENGTH, LocalCoordinate};
use bevy::prelude::IVec3;

impl LocalCoordinate {
    /// Applies a voxel delta and marks only the changed chunk and touched boundaries dirty.
    pub fn apply_voxel(&mut self, voxel: AtomicVoxel) -> bool {
        let (chunk_position, local_position) = split_position(voxel.position);
        let changed = if voxel.data.is_solid() {
            self.chunks
                .entry(chunk_position)
                .or_default()
                .set_voxel(local_position, voxel.data)
        } else {
            let Some(chunk) = self.chunks.get_mut(&chunk_position) else {
                return false;
            };
            chunk.set_voxel(local_position, voxel.data)
        };

        if !changed {
            return false;
        }

        if voxel.data.is_solid() {
            self.assign_color(voxel.position);
        } else {
            self.voxel_colors.remove(&voxel.position);
        }

        if self
            .chunks
            .get(&chunk_position)
            .is_some_and(|chunk| chunk.is_empty())
        {
            self.chunks.remove(&chunk_position);
        }

        self.mark_dirty(chunk_position, local_position);
        true
    }

    /// Applies several updates while coalescing derived work through the dirty-chunk set.
    pub fn apply_voxels(&mut self, voxels: impl IntoIterator<Item = AtomicVoxel>) -> bool {
        let mut changed = false;

        for voxel in voxels {
            changed |= self.apply_voxel(voxel);
        }

        changed
    }

    /// Builds a coordinate from an authoritative voxel list.
    pub fn from_voxels(voxels: impl IntoIterator<Item = AtomicVoxel>) -> Self {
        let mut local_coordinate = Self::default();
        local_coordinate.apply_voxels(voxels);
        local_coordinate
    }

    pub(crate) fn rebuild_dirty_chunks(&mut self) -> bool {
        if self.dirty_chunks.is_empty() {
            return false;
        }

        let mut dirty_positions = std::mem::take(&mut self.dirty_chunks)
            .into_iter()
            .collect::<Vec<_>>();
        dirty_positions.sort_by_key(|position| (position.x, position.y, position.z));

        let occupied_positions = self.occupied_positions();
        let (chunks, voxel_colors) = (&mut self.chunks, &self.voxel_colors);

        for chunk_position in dirty_positions {
            let Some(mut chunk) = chunks.remove(&chunk_position) else {
                continue;
            };
            chunk.rebuild_triangles(
                chunk_position,
                |position| occupied_positions.contains(&position),
                |position| voxel_colors.get(&position).copied().unwrap_or([1.0; 4]),
            );
            chunks.insert(chunk_position, chunk);
        }

        self.triangles.clear();
        let mut chunk_positions = self.chunks.keys().copied().collect::<Vec<_>>();
        chunk_positions.sort_by_key(|position| (position.x, position.y, position.z));
        for chunk_position in chunk_positions {
            self.triangles
                .extend_from_slice(&self.chunks[&chunk_position].triangles);
        }
        true
    }

    fn occupied_positions(&self) -> std::collections::HashSet<IVec3> {
        let mut occupied_positions = std::collections::HashSet::new();

        for (chunk_position, chunk) in &self.chunks {
            let chunk_origin = *chunk_position * CHUNK_EDGE_LENGTH;
            for z in 0..CHUNK_EDGE_LENGTH {
                for y in 0..CHUNK_EDGE_LENGTH {
                    for x in 0..CHUNK_EDGE_LENGTH {
                        let local_position = IVec3::new(x, y, z);
                        if chunk.is_solid(local_position) {
                            occupied_positions.insert(chunk_origin + local_position);
                        }
                    }
                }
            }
        }

        occupied_positions
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

        for axis in 0..3 {
            let unit = match axis {
                0 => IVec3::X,
                1 => IVec3::Y,
                _ => IVec3::Z,
            };
            let coordinate = local_position[axis];

            if coordinate == 0 {
                self.mark_existing_chunk_dirty(chunk_position - unit);
            }
            if coordinate == CHUNK_EDGE_LENGTH - 1 {
                self.mark_existing_chunk_dirty(chunk_position + unit);
            }
        }
    }

    fn mark_existing_chunk_dirty(&mut self, chunk_position: IVec3) {
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
