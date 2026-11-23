//! Validated budgets for precise edits and local physics-demand seams.
//! These data values alone install no queues, demands or world systems.

use bevy::prelude::IVec3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorldInteractionBudgetError {
    ZeroLimit,
    Overflow,
    InvertedRegion,
    RegionTooLarge,
}

/// The capacity includes pending edits and unclaimed completions together.
/// A completion must be claimed explicitly; pending cancellation also retains
/// its result slot until claim. It cannot become an
/// unbounded log or be silently evicted to admit another edit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VoxelEditBudget {
    capacity: usize,
    edits_per_tick: usize,
}

impl VoxelEditBudget {
    pub fn new(
        capacity: usize,
        edits_per_tick: usize,
    ) -> Result<Self, WorldInteractionBudgetError> {
        if capacity == 0 || edits_per_tick == 0 {
            return Err(WorldInteractionBudgetError::ZeroLimit);
        }
        Ok(Self {
            capacity,
            edits_per_tick,
        })
    }
    pub const fn capacity(self) -> usize {
        self.capacity
    }
    pub const fn edits_per_tick(self) -> usize {
        self.edits_per_tick
    }
}

/// Finite local demand and derivation work limits, independent of connection
/// interests. No unlimited or implicit-default setting is representable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicsDemandBudget {
    demands: usize,
    chunks_per_demand: usize,
    generate_per_tick: usize,
    derive_per_tick: usize,
}

impl PhysicsDemandBudget {
    pub fn new(
        demands: usize,
        chunks_per_demand: usize,
        generate_per_tick: usize,
        derive_per_tick: usize,
    ) -> Result<Self, WorldInteractionBudgetError> {
        if [
            demands,
            chunks_per_demand,
            generate_per_tick,
            derive_per_tick,
        ]
        .contains(&0)
        {
            return Err(WorldInteractionBudgetError::ZeroLimit);
        }
        demands
            .checked_mul(chunks_per_demand)
            .ok_or(WorldInteractionBudgetError::Overflow)?;
        Ok(Self {
            demands,
            chunks_per_demand,
            generate_per_tick,
            derive_per_tick,
        })
    }
    pub const fn demands(self) -> usize {
        self.demands
    }
    pub const fn chunks_per_demand(self) -> usize {
        self.chunks_per_demand
    }
    pub const fn generate_per_tick(self) -> usize {
        self.generate_per_tick
    }
    pub const fn derive_per_tick(self) -> usize {
        self.derive_per_tick
    }

    /// Validates an inclusive finite chunk AABB without enumerating/allocating it.
    /// Coordinates are chunk positions, not voxel or world-space positions.
    pub fn region(
        self,
        minimum: IVec3,
        maximum: IVec3,
    ) -> Result<PhysicsDemandRegion, WorldInteractionBudgetError> {
        let mut chunks = 1usize;
        for (min, max) in minimum.to_array().into_iter().zip(maximum.to_array()) {
            if min > max {
                return Err(WorldInteractionBudgetError::InvertedRegion);
            }
            let extent = usize::try_from(i64::from(max) - i64::from(min) + 1)
                .map_err(|_| WorldInteractionBudgetError::Overflow)?;
            chunks = chunks
                .checked_mul(extent)
                .ok_or(WorldInteractionBudgetError::Overflow)?;
        }
        if chunks > self.chunks_per_demand {
            return Err(WorldInteractionBudgetError::RegionTooLarge);
        }
        Ok(PhysicsDemandRegion {
            minimum,
            maximum,
            chunks,
        })
    }
}

/// Validated bounds only; possession is not a demand ID or readiness proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicsDemandRegion {
    minimum: IVec3,
    maximum: IVec3,
    chunks: usize,
}

impl PhysicsDemandRegion {
    /// Finite enumeration, bounded by the validated chunks_per_demand limit.
    pub(super) fn positions(self) -> impl Iterator<Item = IVec3> {
        (self.minimum.z..=self.maximum.z).flat_map(move |z| {
            (self.minimum.y..=self.maximum.y).flat_map(move |y| {
                (self.minimum.x..=self.maximum.x).map(move |x| IVec3::new(x, y, z))
            })
        })
    }
    pub const fn minimum(self) -> IVec3 {
        self.minimum
    }
    pub const fn maximum(self) -> IVec3 {
        self.maximum
    }
    pub const fn chunk_count(self) -> usize {
        self.chunks
    }
}
