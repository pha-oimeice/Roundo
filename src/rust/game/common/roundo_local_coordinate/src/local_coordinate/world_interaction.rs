//! App-local incarnation and bounded no-connection physics demand management.
//! Registration is real, but it does not certify generation/collider readiness.
use super::{
    LocalCoordinate, LocalCoordinateIdentity, PhysicsDemandBudget, PhysicsDemandRegion,
    VoxelEditBudget,
};
use crate::LocalCoordinateId;
use bevy::{
    ecs::{
        lifecycle::HookContext,
        system::SystemParam,
        world::{DeferredWorld, WorldId},
    },
    prelude::*,
};
use std::collections::{HashMap, HashSet};

/// Exact coordinate installation, not an editable target or transferable authority.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CoordinateIncarnation {
    world: WorldId,
    entity: Entity,
    identity: LocalCoordinateId,
    generation: u64,
}
impl CoordinateIncarnation {
    pub const fn identity(self) -> LocalCoordinateId {
        self.identity
    }
    pub const fn entity(self) -> Entity {
        self.entity
    }
}
/// One App-local demand. IDs are never reused, including after explicit removal.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PhysicsDemandId {
    world: WorldId,
    sequence: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorldInteractionError {
    Unavailable,
    InvalidCoordinate,
    InvalidDemand,
    InvalidRegion,
    CapacityExceeded,
    IdExhausted,
}
/// Current canonical chunks and derived collider sources, never a cached gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhysicsRegionReadiness {
    NotReady,
    Ready,
}
/// Explicit, already-validated App-wide limits. Per-application limits are separate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorldInteractionRuntimeBudgets {
    edits: VoxelEditBudget,
    physics: PhysicsDemandBudget,
}
impl WorldInteractionRuntimeBudgets {
    pub const fn new(edits: VoxelEditBudget, physics: PhysicsDemandBudget) -> Self {
        Self { edits, physics }
    }
    pub const fn voxel_edits(self) -> VoxelEditBudget {
        self.edits
    }
    pub const fn physics_demands(self) -> PhysicsDemandBudget {
        self.physics
    }
}
#[derive(Clone, Copy)]
struct Demand {
    coordinate: CoordinateIncarnation,
    region: PhysicsDemandRegion,
}
#[derive(Resource)]
pub(super) struct Runtime {
    world: WorldId,
    budgets: WorldInteractionRuntimeBudgets,
    next_coordinate: Option<u64>,
    next_demand: Option<u64>,
    demands: HashMap<PhysicsDemandId, Demand>,
    by_coordinate: HashMap<Entity, HashSet<PhysicsDemandId>>,
}
fn allocate(next: &mut Option<u64>) -> Result<u64, WorldInteractionError> {
    let sequence = next.ok_or(WorldInteractionError::IdExhausted)?;
    *next = sequence.checked_add(1);
    Ok(sequence)
}
impl Runtime {
    pub(super) fn physics_budget(&self) -> PhysicsDemandBudget {
        self.budgets.physics
    }
    pub(super) fn regions(
        &self,
        entity: Entity,
        coordinate: &LocalCoordinate,
        identity: Option<&LocalCoordinateIdentity>,
    ) -> Vec<PhysicsDemandRegion> {
        let Some(identity) = identity else {
            return Vec::new();
        };
        self.by_coordinate
            .get(&entity)
            .into_iter()
            .flatten()
            .filter_map(|id| {
                let demand = self.demands.get(id)?;
                validate_installation(self.world, demand.coordinate, coordinate, identity).ok()?;
                Some(demand.region)
            })
            .collect()
    }
    fn remove(&mut self, id: PhysicsDemandId) -> Result<(), WorldInteractionError> {
        let entry = self
            .demands
            .remove(&id)
            .ok_or(WorldInteractionError::InvalidDemand)?;
        if let Some(ids) = self.by_coordinate.get_mut(&entry.coordinate.entity) {
            ids.remove(&id);
            if ids.is_empty() {
                self.by_coordinate.remove(&entry.coordinate.entity);
            }
        }
        Ok(())
    }
}

/// Installs management only: no coordinate, Life, Body, application or demand is
/// created and no existing interest/anchor is replaced. Readiness requires
/// independently installed generation/physics capabilities and current sources.
pub struct WorldInteractionPlugin {
    budgets: WorldInteractionRuntimeBudgets,
}
impl WorldInteractionPlugin {
    pub const fn new(budgets: WorldInteractionRuntimeBudgets) -> Self {
        Self { budgets }
    }
}
impl Plugin for WorldInteractionPlugin {
    fn build(&self, app: &mut App) {
        super::voxel_edit::install(app, self.budgets.voxel_edits());
        let world = app.world().id();
        app.insert_resource(Runtime {
            world,
            budgets: self.budgets,
            next_coordinate: Some(1),
            next_demand: Some(1),
            demands: HashMap::new(),
            by_coordinate: HashMap::new(),
        })
        .add_observer(cleanup_coordinate_demands)
        .add_observer(cleanup_coordinate_identity_demands);
    }
}
/// Replacement, including re-insertion of an extracted component, invalidates
/// installation evidence. This hook deliberately needs no plugin or resource.
pub(super) fn clear_incarnation(mut world: DeferredWorld, context: HookContext) {
    if let Some(mut coordinate) = world.get_mut::<LocalCoordinate>(context.entity) {
        coordinate.incarnation = None;
    }
}
fn cleanup_coordinate_demands(
    event: On<Discard<LocalCoordinate>>,
    runtime: Option<ResMut<Runtime>>,
) {
    cleanup_demands_for_entity(event.entity, runtime);
}
fn cleanup_coordinate_identity_demands(
    event: On<Discard<LocalCoordinateIdentity>>,
    runtime: Option<ResMut<Runtime>>,
) {
    cleanup_demands_for_entity(event.entity, runtime);
}
fn cleanup_demands_for_entity(entity: Entity, runtime: Option<ResMut<Runtime>>) {
    let Some(mut runtime) = runtime else {
        return;
    };
    if let Some(ids) = runtime.by_coordinate.remove(&entity) {
        for id in ids {
            runtime.demands.remove(&id);
        }
    }
}

/// Read-only installation/budget evidence for composition preflight. It neither
/// binds coordinates nor registers demands or certifies collider readiness.
#[derive(SystemParam)]
pub struct LocalPhysicsDemandFacts<'w, 's> {
    runtime: Option<Res<'w, Runtime>>,
    coordinates: Query<'w, 's, (&'static LocalCoordinate, &'static LocalCoordinateIdentity)>,
}
impl LocalPhysicsDemandFacts<'_, '_> {
    pub fn budgets(&self) -> Result<WorldInteractionRuntimeBudgets, WorldInteractionError> {
        Ok(self
            .runtime
            .as_ref()
            .ok_or(WorldInteractionError::Unavailable)?
            .budgets)
    }
    pub fn validate_coordinate(
        &self,
        token: CoordinateIncarnation,
    ) -> Result<(), WorldInteractionError> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(WorldInteractionError::Unavailable)?;
        let (coordinate, identity) = self
            .coordinates
            .get(token.entity)
            .map_err(|_| WorldInteractionError::InvalidCoordinate)?;
        validate_installation(runtime.world, token, coordinate, identity)
    }
}
pub(super) fn validate_installation(
    world: WorldId,
    token: CoordinateIncarnation,
    coordinate: &LocalCoordinate,
    identity: &LocalCoordinateIdentity,
) -> Result<(), WorldInteractionError> {
    if token.world != world || coordinate.incarnation != Some(token) || identity.0 != token.identity
    {
        return Err(WorldInteractionError::InvalidCoordinate);
    }
    Ok(())
}

/// Restricted local control plane. It exposes no World, callback, voxel storage,
/// connection policy, mutable demand collection or readiness bypass.
#[derive(SystemParam)]
pub struct LocalPhysicsDemands<'w, 's> {
    runtime: Option<ResMut<'w, Runtime>>,
    coordinates: Query<
        'w,
        's,
        (
            &'static mut LocalCoordinate,
            &'static LocalCoordinateIdentity,
            Option<&'static super::physics::LocalCoordinatePhysicsState>,
        ),
    >,
    colliders: Query<'w, 's, (), With<avian3d::prelude::Collider>>,
}
impl LocalPhysicsDemands<'_, '_> {
    pub fn budgets(&self) -> Result<WorldInteractionRuntimeBudgets, WorldInteractionError> {
        Ok(self
            .runtime
            .as_ref()
            .ok_or(WorldInteractionError::Unavailable)?
            .budgets)
    }
    /// Lazily binds an exact installation. Repeated binding is stable only while
    /// this same coordinate component and domain identity remain installed.
    pub fn bind_coordinate(
        &mut self,
        entity: Entity,
    ) -> Result<CoordinateIncarnation, WorldInteractionError> {
        let runtime = self
            .runtime
            .as_mut()
            .ok_or(WorldInteractionError::Unavailable)?;
        let (mut coordinate, identity, _) = self
            .coordinates
            .get_mut(entity)
            .map_err(|_| WorldInteractionError::InvalidCoordinate)?;
        if let Some(token) = coordinate.incarnation {
            if token.world == runtime.world
                && token.entity == entity
                && token.identity == identity.0
            {
                return Ok(token);
            }
            return Err(WorldInteractionError::InvalidCoordinate);
        }
        let token = CoordinateIncarnation {
            world: runtime.world,
            entity,
            identity: identity.0,
            generation: allocate(&mut runtime.next_coordinate)?,
        };
        coordinate.incarnation = Some(token);
        Ok(token)
    }
    pub fn validate_coordinate(
        &self,
        token: CoordinateIncarnation,
    ) -> Result<(), WorldInteractionError> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(WorldInteractionError::Unavailable)?;
        if token.world != runtime.world {
            return Err(WorldInteractionError::InvalidCoordinate);
        }
        let (coordinate, identity, _) = self
            .coordinates
            .get(token.entity)
            .map_err(|_| WorldInteractionError::InvalidCoordinate)?;
        validate_installation(runtime.world, token, coordinate, identity)
    }
    fn checked_region(
        &self,
        region: PhysicsDemandRegion,
    ) -> Result<PhysicsDemandRegion, WorldInteractionError> {
        self.budgets()?
            .physics_demands()
            .region(region.minimum(), region.maximum())
            .map_err(|_| WorldInteractionError::InvalidRegion)
    }
    pub fn register(
        &mut self,
        coordinate: CoordinateIncarnation,
        region: PhysicsDemandRegion,
    ) -> Result<PhysicsDemandId, WorldInteractionError> {
        self.validate_coordinate(coordinate)?;
        let region = self.checked_region(region)?;
        let runtime = self
            .runtime
            .as_mut()
            .ok_or(WorldInteractionError::Unavailable)?;
        if runtime.demands.len() >= runtime.budgets.physics.demands() {
            return Err(WorldInteractionError::CapacityExceeded);
        }
        let id = PhysicsDemandId {
            world: runtime.world,
            sequence: allocate(&mut runtime.next_demand)?,
        };
        runtime.demands.insert(id, Demand { coordinate, region });
        runtime
            .by_coordinate
            .entry(coordinate.entity)
            .or_default()
            .insert(id);
        Ok(id)
    }
    pub fn update(
        &mut self,
        id: PhysicsDemandId,
        region: PhysicsDemandRegion,
    ) -> Result<(), WorldInteractionError> {
        let coordinate = self
            .runtime
            .as_ref()
            .ok_or(WorldInteractionError::Unavailable)?
            .demands
            .get(&id)
            .ok_or(WorldInteractionError::InvalidDemand)?
            .coordinate;
        self.validate_coordinate(coordinate)?;
        let region = self.checked_region(region)?;
        self.runtime
            .as_mut()
            .unwrap()
            .demands
            .get_mut(&id)
            .unwrap()
            .region = region;
        Ok(())
    }
    /// Exact old IDs can be removed even after their coordinate becomes invalid;
    /// unknown/already removed/foreign IDs never target a replacement demand.
    pub fn remove(&mut self, id: PhysicsDemandId) -> Result<(), WorldInteractionError> {
        self.runtime
            .as_mut()
            .ok_or(WorldInteractionError::Unavailable)?
            .remove(id)
    }
    pub fn demand(
        &self,
        id: PhysicsDemandId,
    ) -> Result<(CoordinateIncarnation, PhysicsDemandRegion), WorldInteractionError> {
        let entry = self
            .runtime
            .as_ref()
            .ok_or(WorldInteractionError::Unavailable)?
            .demands
            .get(&id)
            .ok_or(WorldInteractionError::InvalidDemand)?;
        Ok((entry.coordinate, entry.region))
    }
    pub fn demand_count(&self) -> Result<usize, WorldInteractionError> {
        Ok(self
            .runtime
            .as_ref()
            .ok_or(WorldInteractionError::Unavailable)?
            .demands
            .len())
    }
    /// Rechecks every loaded Chunk and its collider/neighbor revisions, including
    /// empty chunks, against this installation. Missing/stale sources are blocked.
    pub fn readiness(
        &self,
        coordinate: CoordinateIncarnation,
        region: PhysicsDemandRegion,
    ) -> Result<PhysicsRegionReadiness, WorldInteractionError> {
        self.validate_coordinate(coordinate)?;
        self.checked_region(region)?;
        let (lc, _, state) = self
            .coordinates
            .get(coordinate.entity)
            .map_err(|_| WorldInteractionError::InvalidCoordinate)?;
        Ok(
            if state.is_some_and(|state| state.region_ready(lc, region, &self.colliders)) {
                PhysicsRegionReadiness::Ready
            } else {
                PhysicsRegionReadiness::NotReady
            },
        )
    }
}
