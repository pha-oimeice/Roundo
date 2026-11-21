//! Authoritative environment storage and sampling, independent of spatial indexes.

use super::{
    attributes::{CompleteEnvironmentDefaults, SparseEnvironmentOverride, validate_environment},
    virtual_chunk::{VirtualChunkCoordinate, virtual_chunk_coordinate_for_position},
};
use bevy::prelude::{Resource, Vec3};
use std::collections::HashMap;

/// Independent authoritative environment storage; it deliberately does not
/// consult the derived, potentially stale [`super::VirtualChunkIndex`].
#[derive(Resource, Clone, Debug)]
pub struct VirtualChunkEnvironmentMap {
    default: CompleteEnvironmentDefaults,
    overrides: HashMap<VirtualChunkCoordinate, SparseEnvironmentOverride>,
    revisions: HashMap<VirtualChunkCoordinate, u64>,
}

impl Default for VirtualChunkEnvironmentMap {
    fn default() -> Self {
        Self {
            default: CompleteEnvironmentDefaults::default(),
            overrides: HashMap::new(),
            revisions: HashMap::new(),
        }
    }
}

impl VirtualChunkEnvironmentMap {
    pub fn new(default: CompleteEnvironmentDefaults) -> Option<Self> {
        validate_environment(
            default.gravity,
            default.static_friction,
            default.kinetic_friction,
        )
        .then_some(Self {
            default,
            ..Default::default()
        })
    }
    pub fn defaults(&self) -> CompleteEnvironmentDefaults {
        self.default
    }
    pub fn sample(&self, position: Vec3) -> Option<(CompleteEnvironmentDefaults, u64)> {
        let coordinate = virtual_chunk_coordinate_for_position(position)?;
        let override_ = self.overrides.get(&coordinate).copied().unwrap_or_default();
        Some((
            CompleteEnvironmentDefaults {
                gravity: override_.gravity.unwrap_or(self.default.gravity),
                static_friction: override_
                    .static_friction
                    .unwrap_or(self.default.static_friction),
                kinetic_friction: override_
                    .kinetic_friction
                    .unwrap_or(self.default.kinetic_friction),
            },
            self.revisions.get(&coordinate).copied().unwrap_or(0),
        ))
    }
    /// Returns the sparse stored values and monotonically advanced revision.
    /// A missing override still has a revision after it has been removed.
    pub fn override_at(
        &self,
        coordinate: VirtualChunkCoordinate,
    ) -> (SparseEnvironmentOverride, u64) {
        (
            self.overrides.get(&coordinate).copied().unwrap_or_default(),
            self.revisions.get(&coordinate).copied().unwrap_or(0),
        )
    }

    pub fn set_override(
        &mut self,
        coordinate: VirtualChunkCoordinate,
        mut override_: SparseEnvironmentOverride,
    ) -> bool {
        let candidate = CompleteEnvironmentDefaults {
            gravity: override_.gravity.unwrap_or(self.default.gravity),
            static_friction: override_
                .static_friction
                .unwrap_or(self.default.static_friction),
            kinetic_friction: override_
                .kinetic_friction
                .unwrap_or(self.default.kinetic_friction),
        };
        if !validate_environment(
            candidate.gravity,
            candidate.static_friction,
            candidate.kinetic_friction,
        ) {
            return false;
        }
        if override_.gravity == Some(self.default.gravity) {
            override_.gravity = None;
        }
        if override_.static_friction == Some(self.default.static_friction) {
            override_.static_friction = None;
        }
        if override_.kinetic_friction == Some(self.default.kinetic_friction) {
            override_.kinetic_friction = None;
        }
        if override_ == SparseEnvironmentOverride::default() {
            self.overrides.remove(&coordinate);
        } else {
            self.overrides.insert(coordinate, override_);
        }
        *self.revisions.entry(coordinate).or_default() += 1;
        true
    }
}
