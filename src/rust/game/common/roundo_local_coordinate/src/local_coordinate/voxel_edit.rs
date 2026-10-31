//! LC-owned exact edits. Targets/IDs are observations, never application authority.
use super::{world_interaction::validate_installation, *};
use bevy::{
    ecs::{system::SystemParam, world::WorldId},
    prelude::*,
};
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VoxelEditFacts {
    pub voxel: AtomicVoxel,
    pub coordinate_revision: u64,
    pub chunk_revision: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VoxelEditTarget {
    coordinate: CoordinateIncarnation,
    position: IVec3,
    expected: VoxelEditFacts,
}
impl VoxelEditTarget {
    pub const fn coordinate(self) -> CoordinateIncarnation {
        self.coordinate
    }
    pub const fn position(self) -> IVec3 {
        self.position
    }
    pub const fn expected(self) -> VoxelEditFacts {
        self.expected
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VoxelEditId {
    world: WorldId,
    sequence: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VoxelEditError {
    Unavailable,
    TargetUnavailable,
    TargetChanged,
    InvalidVoxel,
    CapacityExceeded,
    IdExhausted,
    EpochExhausted,
    RevisionExhausted,
    InvalidEdit,
    OutOfOrder,
    WindowClosed,
    WorkBudgetExceeded,
}
/// The only host-controlled terminal decisions; world outcomes cannot be forged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VoxelEditRejection {
    NotEligible,
    Cancelled,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VoxelEditOutcome {
    Applied,
    NoChange,
    Rejected(VoxelEditRejection),
    Failed(VoxelEditError),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VoxelEditRequest {
    pub id: VoxelEditId,
    pub target: VoxelEditTarget,
    pub replacement: AtomicVoxel,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VoxelEditCompletion {
    pub request: VoxelEditRequest,
    /// Local fixed epoch, NOT an authoritative command/completion tick.
    pub completed_epoch: u64,
    pub outcome: VoxelEditOutcome,
    /// None if the original installation/loaded chunk is unavailable.
    pub before: Option<VoxelEditFacts>,
    pub after: Option<VoxelEditFacts>,
}
#[derive(SystemSet, Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VoxelEditSet {
    Begin,
    Commit,
    End,
}
#[derive(Resource)]
struct EditRuntime {
    world: WorldId,
    budget: VoxelEditBudget,
    next: Option<u64>,
    epoch: u64,
    exhausted: bool,
    open: bool,
    processed: usize,
    pending: VecDeque<VoxelEditRequest>,
    completions: HashMap<VoxelEditId, VoxelEditCompletion>,
}
impl EditRuntime {
    fn new(world: WorldId, budget: VoxelEditBudget) -> Self {
        Self {
            world,
            budget,
            next: Some(1),
            epoch: 0,
            exhausted: false,
            open: false,
            processed: 0,
            pending: VecDeque::new(),
            completions: HashMap::new(),
        }
    }
    fn begin(&mut self) {
        self.open = false;
        if self.exhausted {
            return;
        }
        let Some(epoch) = self.epoch.checked_add(1) else {
            self.exhausted = true;
            return;
        };
        self.epoch = epoch;
        self.processed = 0;
        self.open = true;
    }
    fn front(&self, id: VoxelEditId) -> Result<VoxelEditRequest, VoxelEditError> {
        if id.world != self.world || !self.pending.iter().any(|r| r.id == id) {
            return Err(VoxelEditError::InvalidEdit);
        }
        if self.pending.front().map(|r| r.id) != Some(id) {
            return Err(VoxelEditError::OutOfOrder);
        }
        if self.exhausted {
            return Err(VoxelEditError::EpochExhausted);
        }
        if !self.open {
            return Err(VoxelEditError::WindowClosed);
        }
        if self.processed >= self.budget.edits_per_tick() {
            return Err(VoxelEditError::WorkBudgetExceeded);
        }
        Ok(*self.pending.front().unwrap())
    }
    fn finish(
        &mut self,
        request: VoxelEditRequest,
        outcome: VoxelEditOutcome,
        before: Option<VoxelEditFacts>,
        after: Option<VoxelEditFacts>,
    ) -> VoxelEditCompletion {
        let result = VoxelEditCompletion {
            request,
            completed_epoch: self.epoch,
            outcome,
            before,
            after,
        };
        self.pending.pop_front();
        self.processed += 1;
        self.completions.insert(request.id, result);
        result
    }
}
pub(super) fn install(app: &mut App, budget: VoxelEditBudget) {
    app.insert_resource(EditRuntime::new(app.world().id(), budget))
        .configure_sets(
            FixedUpdate,
            (VoxelEditSet::Begin, VoxelEditSet::Commit, VoxelEditSet::End)
                .chain()
                .after(LocalCoordinateSet::ApplyCrud)
                .before(LocalCoordinateSet::RebuildIndex),
        )
        .add_systems(FixedUpdate, begin.in_set(VoxelEditSet::Begin))
        .add_systems(FixedUpdate, end.in_set(VoxelEditSet::End));
}
fn begin(mut runtime: ResMut<EditRuntime>) {
    runtime.begin();
}
fn end(mut runtime: ResMut<EditRuntime>) {
    runtime.open = false;
}
fn facts(
    world: WorldId,
    target: VoxelEditTarget,
    coordinate: &LocalCoordinate,
    identity: &LocalCoordinateIdentity,
) -> Result<VoxelEditFacts, VoxelEditError> {
    validate_installation(world, target.coordinate, coordinate, identity)
        .map_err(|_| VoxelEditError::TargetUnavailable)?;
    let chunk = coordinate
        .chunks
        .get(
            &target
                .position
                .div_euclid(IVec3::splat(data::CHUNK_EDGE_LENGTH)),
        )
        .ok_or(VoxelEditError::TargetUnavailable)?;
    Ok(VoxelEditFacts {
        voxel: coordinate.voxel(target.position).unwrap_or(EMPTY_VOXEL_ID),
        coordinate_revision: coordinate.content_revision,
        chunk_revision: chunk.content_revision,
    })
}
fn registered(
    registry: Option<&AtomicVoxelRegistry>,
    voxel: AtomicVoxel,
) -> Result<(), VoxelEditError> {
    let registry = registry.ok_or(VoxelEditError::Unavailable)?;
    if voxel == EMPTY_VOXEL_ID || registry.definition(voxel).is_some() {
        Ok(())
    } else {
        Err(VoxelEditError::InvalidVoxel)
    }
}
#[derive(SystemParam)]
pub struct LocalVoxelEdits<'w, 's> {
    runtime: Option<ResMut<'w, EditRuntime>>,
    registry: Option<Res<'w, AtomicVoxelRegistry>>,
    coordinates: Query<
        'w,
        's,
        (
            &'static mut LocalCoordinate,
            &'static LocalCoordinateIdentity,
        ),
    >,
}
impl LocalVoxelEdits<'_, '_> {
    pub fn observe(
        &self,
        coordinate: CoordinateIncarnation,
        position: IVec3,
    ) -> Result<VoxelEditTarget, VoxelEditError> {
        let runtime = self.runtime.as_ref().ok_or(VoxelEditError::Unavailable)?;
        let (lc, identity) = self
            .coordinates
            .get(coordinate.entity())
            .map_err(|_| VoxelEditError::TargetUnavailable)?;
        let mut target = VoxelEditTarget {
            coordinate,
            position,
            expected: VoxelEditFacts {
                voxel: EMPTY_VOXEL_ID,
                coordinate_revision: 0,
                chunk_revision: 0,
            },
        };
        target.expected = facts(runtime.world, target, lc, identity)?;
        Ok(target)
    }
    pub fn submit(
        &mut self,
        target: VoxelEditTarget,
        replacement: AtomicVoxel,
    ) -> Result<VoxelEditId, VoxelEditError> {
        let runtime = self.runtime.as_mut().ok_or(VoxelEditError::Unavailable)?;
        let (lc, identity) = self
            .coordinates
            .get(target.coordinate.entity())
            .map_err(|_| VoxelEditError::TargetUnavailable)?;
        if facts(runtime.world, target, lc, identity)? != target.expected {
            return Err(VoxelEditError::TargetChanged);
        }
        registered(self.registry.as_deref(), replacement)?;
        if runtime.exhausted {
            return Err(VoxelEditError::EpochExhausted);
        }
        if runtime.pending.len() + runtime.completions.len() >= runtime.budget.capacity() {
            return Err(VoxelEditError::CapacityExceeded);
        }
        let sequence = runtime.next.ok_or(VoxelEditError::IdExhausted)?;
        runtime.next = sequence.checked_add(1);
        let id = VoxelEditId {
            world: runtime.world,
            sequence,
        };
        runtime.pending.push_back(VoxelEditRequest {
            id,
            target,
            replacement,
        });
        Ok(id)
    }
    /// Execution work admission, separate from Life Command acceptance.
    pub fn can_submit_and_commit(&self) -> bool {
        self.runtime.as_ref().is_some_and(|runtime| {
            runtime.open
                && !runtime.exhausted
                && runtime.pending.is_empty()
                && runtime.processed < runtime.budget.edits_per_tick()
                && runtime.pending.len() + runtime.completions.len() < runtime.budget.capacity()
        })
    }
    pub fn pending_front(&self) -> Result<Option<VoxelEditRequest>, VoxelEditError> {
        Ok(self
            .runtime
            .as_ref()
            .ok_or(VoxelEditError::Unavailable)?
            .pending
            .front()
            .copied())
    }
    pub fn completion(&self, id: VoxelEditId) -> Result<VoxelEditCompletion, VoxelEditError> {
        self.runtime
            .as_ref()
            .ok_or(VoxelEditError::Unavailable)?
            .completions
            .get(&id)
            .copied()
            .ok_or(VoxelEditError::InvalidEdit)
    }
    pub fn take_completion(
        &mut self,
        id: VoxelEditId,
    ) -> Result<VoxelEditCompletion, VoxelEditError> {
        self.runtime
            .as_mut()
            .ok_or(VoxelEditError::Unavailable)?
            .completions
            .remove(&id)
            .ok_or(VoxelEditError::InvalidEdit)
    }
    /// Trusted host MUST recheck live route/status/permission in this same call's
    /// system immediately beforehand. Command retention is not edit authority.
    pub fn commit_front(&mut self, id: VoxelEditId) -> Result<VoxelEditCompletion, VoxelEditError> {
        let runtime = self.runtime.as_mut().ok_or(VoxelEditError::Unavailable)?;
        let request = runtime.front(id)?;
        let mut before = None;
        let mut after = None;
        let outcome = (|| {
            let (mut lc, identity) = self
                .coordinates
                .get_mut(request.target.coordinate.entity())
                .map_err(|_| VoxelEditError::TargetUnavailable)?;
            let actual = facts(runtime.world, request.target, &lc, identity)?;
            before = Some(actual);
            after = Some(actual);
            if actual != request.target.expected {
                return Err(VoxelEditError::TargetChanged);
            }
            registered(self.registry.as_deref(), request.replacement)?;
            if actual.voxel == request.replacement {
                return Ok(VoxelEditOutcome::NoChange);
            }
            actual
                .coordinate_revision
                .checked_add(1)
                .ok_or(VoxelEditError::RevisionExhausted)?;
            actual
                .chunk_revision
                .checked_add(1)
                .ok_or(VoxelEditError::RevisionExhausted)?;
            if lc.apply_voxel(PositionedAtomicVoxel {
                position: request.target.position,
                voxel: request.replacement,
            }) {
                after = Some(facts(runtime.world, request.target, &lc, identity)?);
                Ok(VoxelEditOutcome::Applied)
            } else {
                Ok(VoxelEditOutcome::NoChange)
            }
        })()
        .unwrap_or_else(VoxelEditOutcome::Failed);
        Ok(runtime.finish(request, outcome, before, after))
    }
    pub fn reject_front(
        &mut self,
        id: VoxelEditId,
        reason: VoxelEditRejection,
    ) -> Result<VoxelEditCompletion, VoxelEditError> {
        let runtime = self.runtime.as_mut().ok_or(VoxelEditError::Unavailable)?;
        let request = runtime.front(id)?;
        let actual = self
            .coordinates
            .get(request.target.coordinate.entity())
            .ok()
            .and_then(|(lc, identity)| facts(runtime.world, request.target, lc, identity).ok());
        Ok(runtime.finish(request, VoxelEditOutcome::Rejected(reason), actual, actual))
    }
}
