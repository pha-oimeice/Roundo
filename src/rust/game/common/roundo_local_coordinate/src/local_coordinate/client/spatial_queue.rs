//! Approximate near-first work selection in Chunk space, independent of arrival order.
//!
//! Only occupancy is indexed. A moving loading origin never changes stored keys
//! or triggers reprioritization. Lookup probes a local stencil at successively
//! coarser dyadic cells, then descends an occupied cell toward the current origin.
//! This deliberately trades exact Euclidean ordering for bounded spatial lookup.

use crate::{ChunkCoordinate, ChunkId, LocalCoordinateId};
use std::collections::{HashMap, hash_map::Entry};

/// Latest work per Chunk, with no stored distance or insertion-order priority.
pub(super) struct SpatialChunkQueue<T> {
    coordinates: HashMap<LocalCoordinateId, CoordinateQueue<T>>,
    len: usize,
    next_origin: usize,
}

impl<T> Default for SpatialChunkQueue<T> {
    fn default() -> Self {
        Self {
            coordinates: HashMap::new(),
            len: 0,
            next_origin: 0,
        }
    }
}

impl<T> SpatialChunkQueue<T> {
    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn clear(&mut self) {
        self.coordinates.clear();
        self.len = 0;
        self.next_origin = 0;
    }

    pub(super) fn get(&self, id: ChunkId) -> Option<&T> {
        self.coordinates
            .get(&id.local_coordinate_id)?
            .chunks
            .get(&id.coordinate)
    }

    /// Replacement coalesces one identity without changing spatial occupancy.
    pub(super) fn insert(&mut self, id: ChunkId, value: T) {
        let coordinate = self.coordinates.entry(id.local_coordinate_id).or_default();
        if coordinate.insert(id.coordinate, value) {
            self.len += 1;
        }
    }

    pub(super) fn remove(&mut self, id: ChunkId) -> Option<T> {
        let coordinate = self.coordinates.get_mut(&id.local_coordinate_id)?;
        let value = coordinate.remove(id.coordinate)?;
        self.len -= 1;
        if coordinate.chunks.is_empty() {
            self.coordinates.remove(&id.local_coordinate_id);
        }
        Some(value)
    }

    pub(super) fn remove_coordinate(&mut self, id: LocalCoordinateId) {
        if let Some(coordinate) = self.coordinates.remove(&id) {
            self.len -= coordinate.chunks.len();
        }
    }

    /// Selects near the current Chunk origins, never a cached player position.
    ///
    /// All origins are probed at a finer level before any coarser level. A
    /// rotating origin cursor avoids privileging one of several loading anchors.
    /// Missing origins do not fabricate an origin at zero; remaining work can
    /// still drain (notably unloads after the last anchor disappears).
    pub(super) fn pop_near(
        &mut self,
        origins: &[(LocalCoordinateId, ChunkCoordinate)],
    ) -> Option<(ChunkId, T)> {
        if self.len == 0 {
            return None;
        }
        for level in 0..i64::BITS {
            for offset in 0..origins.len() {
                let index = (self.next_origin + offset) % origins.len();
                let (id, origin) = origins[index];
                let Some(coordinate) = self.coordinates.get(&id) else {
                    continue;
                };
                if let Some(position) = coordinate.find_near(level, origin) {
                    let key = ChunkId {
                        local_coordinate_id: id,
                        coordinate: position,
                    };
                    self.next_origin = (index + 1) % origins.len();
                    return self.remove(key).map(|value| (key, value));
                }
            }
        }
        let (&id, coordinate) = self.coordinates.iter().next()?;
        let &position = coordinate.chunks.keys().next()?;
        let key = ChunkId {
            local_coordinate_id: id,
            coordinate: position,
        };
        self.remove(key).map(|value| (key, value))
    }
}

struct CoordinateQueue<T> {
    chunks: HashMap<ChunkCoordinate, T>,
    // Level zero is `chunks`; each following level doubles the cell edge.
    // Counts are occupied leaf cells, not priorities or duplicated payloads.
    levels: Vec<HashMap<ChunkCoordinate, usize>>,
}

impl<T> Default for CoordinateQueue<T> {
    fn default() -> Self {
        Self {
            chunks: HashMap::new(),
            levels: (1..i64::BITS).map(|_| HashMap::new()).collect(),
        }
    }
}

impl<T> CoordinateQueue<T> {
    fn insert(&mut self, position: ChunkCoordinate, value: T) -> bool {
        match self.chunks.entry(position) {
            Entry::Occupied(mut entry) => {
                entry.insert(value);
                false
            }
            Entry::Vacant(entry) => {
                entry.insert(value);
                let mut cell = position;
                for level in &mut self.levels {
                    cell = cell.map(|axis| axis.div_euclid(2));
                    *level.entry(cell).or_default() += 1;
                }
                true
            }
        }
    }

    fn remove(&mut self, position: ChunkCoordinate) -> Option<T> {
        let value = self.chunks.remove(&position)?;
        let mut cell = position;
        for level in &mut self.levels {
            cell = cell.map(|axis| axis.div_euclid(2));
            let mut entry = match level.entry(cell) {
                Entry::Occupied(entry) => entry,
                Entry::Vacant(_) => unreachable!("occupied Chunk retains its spatial ancestors"),
            };
            *entry.get_mut() -= 1;
            if *entry.get() == 0 {
                entry.remove();
            }
        }
        Some(value)
    }

    fn occupied(&self, level: u32, cell: ChunkCoordinate) -> bool {
        if level == 0 {
            self.chunks.contains_key(&cell)
        } else {
            self.levels[level as usize - 1].contains_key(&cell)
        }
    }

    fn find_near(&self, level: u32, origin: ChunkCoordinate) -> Option<ChunkCoordinate> {
        // Arithmetic shift is Euclidean division by 2^level, including negatives.
        let home = origin.map(|axis| axis >> level);
        if self.occupied(level, home) {
            return Some(self.descend(level, home, origin));
        }
        // Geometric stencil: faces, then edges, then corners. No Chunk-to-Chunk
        // distance comparisons and no scan through the pending payload set.
        for squared_offset in 1..=3 {
            for x in -1_i64..=1 {
                for y in -1_i64..=1 {
                    for z in -1_i64..=1 {
                        if x * x + y * y + z * z != squared_offset {
                            continue;
                        }
                        let Some(cell) = home[0].checked_add(x).and_then(|x| {
                            Some([x, home[1].checked_add(y)?, home[2].checked_add(z)?])
                        }) else {
                            continue;
                        };
                        if self.occupied(level, cell) {
                            return Some(self.descend(level, cell, origin));
                        }
                    }
                }
            }
        }
        None
    }

    fn descend(
        &self,
        mut level: u32,
        mut cell: ChunkCoordinate,
        origin: ChunkCoordinate,
    ) -> ChunkCoordinate {
        while level > 0 {
            // Widen before computing the midpoint: level 63 covers signed i64
            // extrema, and its mathematical bounds need one extra bit.
            let preferred = (0..3).fold(0_u8, |octant, axis| {
                let midpoint = (i128::from(cell[axis]) << level) + (1_i128 << (level - 1));
                octant | (u8::from(i128::from(origin[axis]) >= midpoint) << axis)
            });
            cell = (0..8_u8)
                .find_map(|offset| {
                    let octant = preferred ^ offset;
                    let mut child = [0; 3];
                    for axis in 0..3 {
                        child[axis] = i64::try_from(
                            i128::from(cell[axis]) * 2 + i128::from((octant >> axis) & 1),
                        )
                        .ok()?;
                    }
                    self.occupied(level - 1, child).then_some(child)
                })
                .expect("occupied spatial cell retains an occupied child");
            level -= 1;
        }
        cell
    }
}
