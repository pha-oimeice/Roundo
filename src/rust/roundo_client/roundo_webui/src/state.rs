//! Native-independent document transactions. Logical authority stays in the manager.

use crate::registry::MAX_PREPARED_UI_CANDIDATES;
use crate::{
    ClientInteractionMode, PendingUiDescriptor, RecoverySurface, UiBounds, UiCommandSource,
    UiInstanceId, UiLifecycleError, UiLifecycleManager, UiOpenTarget, UiRootReplacement,
};
use roundo_scene::SceneKind;
use roundo_toolbox::request_response_pipe::{RequestCall, ResponseSender};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);

/// Process-local identity of one real opening (or its claimable blank candidate).
/// Only its issuing lifetime may use it; commit/terminal completion ends binding
/// eligibility. Claim preserves identity. This token has no wire representation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct UiOpeningId {
    owner: u64,
    pending: u64,
}
impl UiOpeningId {
    /// Returns the manager-issued pending/future instance identity, interpreted
    /// only within the manager paired with this opening's issuing lifetime.
    /// It does not prove that the instance is live or committed, grant command
    /// authority, or extend the lifetime of the opening or its physical view.
    /// Cancellation, failure, or destruction leaves the numeric identity
    /// unchanged; it must not be used to bypass normal admission checks.
    /// This lookup performs no I/O and does not mutate any state.
    pub fn instance_id(self) -> UiInstanceId {
        UiInstanceId::from_host_id(self.pending)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct UiDocumentProof {
    pub(crate) navigation_id: u64,
    pub(crate) start_revision: u64,
    pub(crate) document_identity: u64,
    pub(crate) source_revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct UiActionId(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct HandoffId {
    owner: u64,
    generation: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RecoveryId {
    owner: u64,
    generation: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct UiPresentation {
    pub(crate) bounds: UiBounds,
    pub(crate) interaction_mode: ClientInteractionMode,
    pub(crate) visible: bool,
    pub(crate) focused: bool,
    /// This visible in-game view must hand keyboard focus to the host before
    /// its presentation can be acknowledged, even when its mode is unchanged.
    pub(crate) host_focused: bool,
}
struct PresentationRequest {
    desired: UiPresentation,
    action_id: UiActionId,
    confirmed: Option<bool>,
    expected_document: Option<UiDocumentProof>,
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum HandoffTarget {
    View(UiOpeningId),
    Recovery(RecoveryId),
    World,
}
struct HandoffGroup {
    members: BTreeSet<u64>,
    cause: HandoffCause,
    target: Option<HandoffTarget>,
    notified: bool,
    world_presented: bool,
}
struct RecoveryEpisode {
    snapshot: RecoverySurface,
    id: RecoveryId,
    notified: bool,
    presented: bool,
}

pub(crate) struct RecoveryActivity {
    pub(crate) exists: bool,
    pub(crate) scripts: Option<usize>,
    pub(crate) navigation_pending: Option<bool>,
    pub(crate) create_requested: bool,
    pub(crate) close_requested: bool,
    pub(crate) close_failed: bool,
}

pub(crate) struct NativeActivity {
    pub(crate) parent_available: bool,
    pub(crate) callback_depth: Option<usize>,
    pub(crate) scripts: BTreeMap<UiOpeningId, Option<usize>>,
    pub(crate) navigation_pending: BTreeMap<UiOpeningId, Option<bool>>,
    pub(crate) recovery: RecoveryActivity,
    /// Ordinary blockers after exact deferred-response filtering.
    pub(crate) commands_pending: bool,
}

pub(crate) enum NativeFact {
    NativeFault {
        message: String,
    },
    PresentationResult {
        opening: UiOpeningId,
        action_id: UiActionId,
        result: Result<bool, String>,
    },
    RecoveryPresented {
        recovery: RecoveryId,
    },
    WorldPresented {
        handoff: HandoffId,
    },
    CreateResult {
        opening: UiOpeningId,
        action_id: UiActionId,
        created_at: Option<Instant>,
        result: Result<(), String>,
    },
    BlankFinished {
        opening: UiOpeningId,
    },
    LoadResult {
        opening: UiOpeningId,
        action_id: UiActionId,
        result: Result<(), String>,
    },
    NavigationStarted {
        opening: UiOpeningId,
        target: Option<UiOpenTarget>,
        navigation_id: u64,
        start_revision: u64,
    },
    DocumentInstalled {
        opening: UiOpeningId,
        proof: UiDocumentProof,
        target: Option<UiOpenTarget>,
    },
    SourceQualificationChanged {
        opening: UiOpeningId,
        proof: UiDocumentProof,
        target: Option<UiOpenTarget>,
    },
    NavigationCompleted {
        opening: UiOpeningId,
        navigation_id: u64,
        start_revision: u64,
        result: Result<(), String>,
    },
    Handshake {
        opening: UiOpeningId,
        proof: UiDocumentProof,
    },
    ActivationSkipped {
        opening: UiOpeningId,
        expected_document: UiDocumentProof,
    },
    DestroyingDispatched {
        opening: UiOpeningId,
        action_id: UiActionId,
    },
    CloseResult {
        opening: UiOpeningId,
        action_id: UiActionId,
        result: Result<(), String>,
    },
}

pub(crate) enum UiAction {
    ReportFatalNativeFailure {
        message: String,
    },
    ApplyPresentation {
        opening: UiOpeningId,
        action_id: UiActionId,
        presentation: UiPresentation,
        expected_document: Option<UiDocumentProof>,
    },
    RecoveryHandoffNeeded {
        recovery: RecoveryId,
    },
    WorldHandoffNeeded {
        handoff: HandoffId,
    },
    CreateBlank {
        opening: UiOpeningId,
        action_id: UiActionId,
        descriptor: PendingUiDescriptor,
    },
    LoadDocument {
        opening: UiOpeningId,
        action_id: UiActionId,
        target: UiOpenTarget,
    },
    SuspendPrepared {
        opening: UiOpeningId,
    },
    ResumePrepared {
        opening: UiOpeningId,
    },
    ActivateCommittedAndReleaseQueued {
        opening: UiOpeningId,
        instance: UiInstanceId,
        expected_document: UiDocumentProof,
    },
    SettleOpenResponse {
        opening: UiOpeningId,
        sender: ResponseSender<Value>,
        value: Value,
    },
    BeginDestroying {
        opening: UiOpeningId,
        action_id: UiActionId,
    },
    CloseAndDropView {
        opening: UiOpeningId,
        action_id: UiActionId,
    },
    ReportFatalCreateFailure {
        opening: UiOpeningId,
        message: String,
    },
}

/// Logical removal records its handoff cause without granting disposal permission.
#[derive(Clone, Copy, Eq, PartialEq)]
enum HandoffCause {
    Ordinary,
    RootReplacement(UiOpeningId),
    World,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Phase {
    Queued,
    Creating,
    AwaitBlank,
    Prepared,
    Loading,
    Committed,
    Retained(HandoffId),
    Destroying,
    Disposing,
}
struct DeferredResponse {
    sender: ResponseSender<Value>,
    success: Value,
}
struct InstalledDocument {
    proof: UiDocumentProof,
    target: Option<UiOpenTarget>,
}
struct Record {
    descriptor: PendingUiDescriptor,
    root_replacement: bool,
    presentation: Option<PresentationRequest>,
    phase: Phase,
    outstanding: Option<UiActionId>,
    created_at: Option<Instant>,
    blank_finished: bool,
    load_accepted: bool,
    // The admitted native Start, not the immutable logical Open target.
    navigation: Option<(u64, u64, Option<UiOpenTarget>)>,
    installed: Option<InstalledDocument>,
    document_high_water: u64,
    source_high_water: u64,
    navigation_result: Option<Result<(), String>>,
    confirmation: Option<UiDocumentProof>,
    // A signed ticket is assumed applied unless the driver reports stale skip.
    activation_ticket: Option<UiDocumentProof>,
    response: Option<DeferredResponse>,
    resume_pending: bool,
    disposed: bool,
    terminal: Option<UiLifecycleError>,
}
impl Record {
    fn new(descriptor: PendingUiDescriptor) -> Self {
        Self {
            descriptor,
            root_replacement: false,
            presentation: None,
            phase: Phase::Queued,
            outstanding: None,
            created_at: None,
            blank_finished: false,
            load_accepted: false,
            navigation: None,
            installed: None,
            document_high_water: 0,
            source_high_water: 0,
            navigation_result: None,
            confirmation: None,
            activation_ticket: None,
            response: None,
            resume_pending: false,
            disposed: false,
            terminal: None,
        }
    }
    fn installed_proof(&self) -> Option<UiDocumentProof> {
        self.installed.as_ref().map(|document| document.proof)
    }
    fn project_document_path(&self, manager: &mut UiLifecycleManager, instance: UiInstanceId) {
        let Some(target) = self
            .installed
            .as_ref()
            .and_then(|document| document.target.as_ref())
        else {
            return;
        };
        if manager
            .instance(instance)
            .is_some_and(|current| current.current_path != target.path)
        {
            if let Err(error) = manager.navigate_same_definition(instance, &target.path) {
                log::warn!("cannot project confirmed UI document path: {error}");
            }
        }
    }
    fn matches_start(&self, navigation_id: u64, start_revision: u64) -> bool {
        self.navigation
            .as_ref()
            .is_some_and(|(id, revision, _)| *id == navigation_id && *revision == start_revision)
    }
    fn qualified_document(&self) -> Option<UiDocumentProof> {
        let document = self.installed.as_ref()?;
        let target = document.target.as_ref()?;
        let target_qualified = if self.phase == Phase::Committed {
            target.resource() == self.descriptor.target.resource()
        } else {
            target == &self.descriptor.target
        };
        (target_qualified
            && self.matches_start(document.proof.navigation_id, document.proof.start_revision)
            && self.navigation_result == Some(Ok(()))
            && self.confirmation == Some(document.proof))
        .then_some(document.proof)
    }
}

pub(crate) struct UiViewLifetime {
    owner: u64,
    next_action: u64,
    next_handoff: u64,
    next_recovery: u64,
    groups: BTreeMap<HandoffId, HandoffGroup>,
    recovery: Option<RecoveryEpisode>,
    timeout: Duration,
    creation_frozen: bool,
    native_fault_reported: bool,
    records: BTreeMap<u64, Record>,
    facts: Vec<NativeFact>,
}
impl UiViewLifetime {
    pub(crate) fn new(timeout: Duration) -> Self {
        assert!(!timeout.is_zero(), "UI load timeout must be positive");
        Self {
            owner: NEXT_OWNER.fetch_add(1, Ordering::Relaxed),
            next_action: 1,
            next_handoff: 1,
            next_recovery: 1,
            groups: BTreeMap::new(),
            recovery: None,
            timeout,
            creation_frozen: false,
            native_fault_reported: false,
            records: BTreeMap::new(),
            facts: Vec::new(),
        }
    }
    pub(crate) fn set_timeout(&mut self, timeout: Duration) {
        assert!(!timeout.is_zero(), "UI load timeout must be positive");
        self.timeout = timeout;
    }
    pub(crate) fn open_configured_root(
        &mut self,
        manager: &mut UiLifecycleManager,
        now: Instant,
    ) -> Result<Option<UiInstanceId>, UiLifecycleError> {
        let pending = manager.begin_configured_root()?;
        self.reconcile(manager, now);
        Ok(pending.map(UiInstanceId::from_host_id))
    }
    pub(crate) fn opening(&self, pending: u64) -> UiOpeningId {
        UiOpeningId {
            owner: self.owner,
            pending,
        }
    }
    pub(crate) fn owns_opening(&self, opening: UiOpeningId) -> bool {
        opening.owner == self.owner
    }
    fn action_id(&mut self) -> UiActionId {
        let id = UiActionId(self.next_action);
        self.next_action += 1;
        id
    }
    pub(crate) fn open(
        &mut self,
        manager: &mut UiLifecycleManager,
        source: UiCommandSource,
        target: UiOpenTarget,
        now: Instant,
    ) -> Result<UiOpeningId, UiLifecycleError> {
        let pending = match manager.claim_prefetch(source, &target)? {
            Some(id) => id,
            None => manager.begin_open(source, target)?,
        };
        let descriptor = manager
            .pending_descriptor(pending)
            .expect("manager issued pending");
        let record = self
            .records
            .entry(pending)
            .or_insert_with(|| Record::new(descriptor.clone()));
        record.descriptor = descriptor;
        if record.phase == Phase::Prepared {
            record.phase = Phase::AwaitBlank;
            record.created_at = Some(now);
            record.resume_pending = true;
        }
        Ok(self.opening(pending))
    }
    pub(crate) fn bind_open_response(
        &mut self,
        opening: UiOpeningId,
        sender: ResponseSender<Value>,
        success: Value,
    ) -> Result<(), (ResponseSender<Value>, Value)> {
        if self.owns_opening(opening) {
            if let Some(record) = self.records.get_mut(&opening.pending) {
                if !record.descriptor.prefetched
                    && !matches!(
                        record.phase,
                        Phase::Committed
                            | Phase::Retained(_)
                            | Phase::Destroying
                            | Phase::Disposing
                    )
                    && record.terminal.is_none()
                    && record.response.is_none()
                {
                    record.response = Some(DeferredResponse { sender, success });
                    return Ok(());
                }
            }
        }
        Err((sender, success))
    }
    pub(crate) fn command_blocks_creation(&self, call: &RequestCall<Value>) -> bool {
        !self
            .records
            .values()
            .filter_map(|record| record.response.as_ref())
            .any(|response| response.sender.matches_call(call))
    }
    pub(crate) fn replace_root(
        &mut self,
        manager: &mut UiLifecycleManager,
        scene: SceneKind,
        now: Instant,
    ) -> Result<UiRootReplacement, UiLifecycleError> {
        if let Some((id, target)) = manager.pending_root_replacement_target() {
            if target == scene {
                return Ok(UiRootReplacement::Pending(id));
            }
            self.cancel_root_replacement(manager);
        }
        if manager.root_scene() == scene {
            return Ok(UiRootReplacement::Unchanged);
        }
        let result = manager.begin_root_replacement(scene)?;
        if let UiRootReplacement::Completed(destroyed) = &result {
            self.retain_destroyed(destroyed, HandoffCause::World);
        }
        self.reconcile(manager, now);
        Ok(result)
    }
    fn cancel_root_replacement(&mut self, manager: &mut UiLifecycleManager) {
        for id in manager.cancel_root_replacement() {
            if let Some(record) = self.records.get_mut(&id) {
                record.terminal = Some(UiLifecycleError::StaleUiInstance);
            }
        }
    }
    pub(crate) fn back(
        &mut self,
        manager: &mut UiLifecycleManager,
        source: UiInstanceId,
    ) -> Result<Vec<UiInstanceId>, UiLifecycleError> {
        let destroyed = manager.back(source)?;
        self.retain_destroyed(&destroyed, HandoffCause::Ordinary);
        Ok(destroyed)
    }
    pub(crate) fn unload_definitions(
        &mut self,
        manager: &mut UiLifecycleManager,
        definitions: impl IntoIterator<Item = String>,
    ) -> Result<Vec<UiInstanceId>, UiLifecycleError> {
        let destroyed = manager.unload_definitions(definitions)?;
        self.retain_destroyed(&destroyed, HandoffCause::Ordinary);
        Ok(destroyed)
    }
    fn handoff_id(&mut self) -> HandoffId {
        let id = HandoffId {
            owner: self.owner,
            generation: self.next_handoff,
        };
        self.next_handoff += 1;
        id
    }
    fn retain_destroyed(&mut self, destroyed: &[UiInstanceId], cause: HandoffCause) {
        let mut members = destroyed
            .iter()
            .filter_map(|instance| {
                self.records
                    .get(&instance.value())
                    .filter(|record| record.phase == Phase::Committed)
                    .map(|_| instance.value())
            })
            .collect::<BTreeSet<_>>();
        if !matches!(cause, HandoffCause::Ordinary) {
            members.extend(self.records.iter().filter_map(|(id, record)| {
                matches!(record.phase, Phase::Retained(_)).then_some(*id)
            }));
            self.groups.clear();
        }
        if members.is_empty() {
            return;
        }
        let id = self.handoff_id();
        for member in &members {
            let record = self.records.get_mut(member).expect("retained member");
            record.phase = Phase::Retained(id);
            record.presentation = None;
        }
        self.groups.insert(
            id,
            HandoffGroup {
                members,
                cause,
                target: None,
                notified: false,
                world_presented: false,
            },
        );
    }
    fn sync_recovery(&mut self, manager: &UiLifecycleManager) {
        let snapshot = manager.recovery_surface();
        if self.recovery.as_ref().map(|episode| &episode.snapshot) == snapshot {
            return;
        }
        self.recovery = snapshot.cloned().map(|snapshot| {
            let id = RecoveryId {
                owner: self.owner,
                generation: self.next_recovery,
            };
            self.next_recovery += 1;
            RecoveryEpisode {
                snapshot,
                id,
                notified: false,
                presented: false,
            }
        });
    }
    fn desired_presentation(
        &self,
        manager: &UiLifecycleManager,
        opening: UiOpeningId,
    ) -> Option<UiPresentation> {
        let instance = self
            .committed_identity(opening)
            .and_then(|id| manager.instance(id))?;
        let definition = manager.registry.resource(&instance.definition)?;
        let focused = manager.focused_instance();
        Some(UiPresentation {
            bounds: instance.bounds,
            interaction_mode: definition.interaction_mode,
            visible: instance.visible,
            focused: focused == Some(instance.id) && instance.visible,
            host_focused: instance.visible
                && definition.interaction_mode == ClientInteractionMode::InGame
                && focused.is_none()
                && manager.recovery_surface().is_none(),
        })
    }
    /// Live committed identities in native sync order: visible, then hidden;
    /// each partition preserves the manager's back-to-front stacking order.
    pub(crate) fn presentation_order(&self, manager: &UiLifecycleManager) -> Vec<UiOpeningId> {
        let stacking = manager.stacking_order();
        [true, false]
            .into_iter()
            .flat_map(|visible| {
                stacking.iter().filter_map(move |id| {
                    let opening = self.opening(id.value());
                    (self.committed_identity(opening).is_some()
                        && manager
                            .instance(*id)
                            .is_some_and(|instance| instance.visible == visible))
                    .then_some(opening)
                })
            })
            .collect()
    }
    fn ordinary_target(&self, manager: &UiLifecycleManager) -> HandoffTarget {
        if let Some(episode) = &self.recovery {
            return HandoffTarget::Recovery(episode.id);
        }
        manager
            .stacking_order()
            .into_iter()
            .rev()
            .find_map(|id| {
                manager
                    .instance(id)
                    .filter(|instance| {
                        instance.loaded
                            && instance.visible
                            && manager.registry.resource(&instance.definition).is_some()
                    })
                    .map(|_| HandoffTarget::View(self.opening(id.value())))
            })
            .unwrap_or(HandoffTarget::World)
    }
    fn sync_handoffs(&mut self, manager: &UiLifecycleManager, actions: &mut Vec<UiAction>) {
        self.sync_recovery(manager);
        if let Some(episode) = &mut self.recovery {
            if !episode.notified {
                episode.notified = true;
                actions.push(UiAction::RecoveryHandoffNeeded {
                    recovery: episode.id,
                });
            }
        }
        let mut force = BTreeSet::new();
        for (mut id, mut group) in std::mem::take(&mut self.groups) {
            let target = match group.cause {
                HandoffCause::RootReplacement(opening)
                    if self
                        .committed_identity(opening)
                        .is_some_and(|instance| manager.instance(instance).is_some()) =>
                {
                    HandoffTarget::View(opening)
                }
                HandoffCause::World => HandoffTarget::World,
                _ => {
                    group.cause = HandoffCause::Ordinary;
                    self.ordinary_target(manager)
                }
            };
            if group.target != Some(target) {
                if group.target.is_some() {
                    id = self.handoff_id();
                    for member in &group.members {
                        self.records.get_mut(member).expect("retained member").phase =
                            Phase::Retained(id);
                    }
                }
                group.target = Some(target);
                group.notified = false;
                group.world_presented = false;
                if let HandoffTarget::View(opening) = target {
                    force.insert(opening);
                }
            }
            if target == HandoffTarget::World && !group.notified {
                group.notified = true;
                actions.push(UiAction::WorldHandoffNeeded { handoff: id });
            }
            self.groups.insert(id, group);
        }
        for opening in self.presentation_order(manager) {
            let Some(desired) = self.desired_presentation(manager, opening) else {
                continue;
            };
            let request = self
                .records
                .get(&opening.pending)
                .and_then(|record| record.presentation.as_ref());
            if force.contains(&opening)
                || request.is_none_or(|request| {
                    request.desired != desired || request.confirmed == Some(false)
                })
            {
                let action_id = self.action_id();
                self.records
                    .get_mut(&opening.pending)
                    .expect("committed record")
                    .presentation = Some(PresentationRequest {
                    desired,
                    action_id,
                    confirmed: None,
                    expected_document: self.records[&opening.pending].installed_proof(),
                });
                actions.push(UiAction::ApplyPresentation {
                    opening,
                    action_id,
                    presentation: desired,
                    expected_document: self.records[&opening.pending].installed_proof(),
                });
            }
        }
        let mut completed = Vec::new();
        for (id, group) in &mut self.groups {
            let ready = match group.target {
                Some(HandoffTarget::View(opening)) => {
                    let request = self
                        .records
                        .get(&opening.pending)
                        .and_then(|record| record.presentation.as_ref());
                    request.is_some_and(|request| {
                        request.desired.visible
                            && request.confirmed == Some(true)
                            && request.expected_document.is_some()
                            && self.records.get(&opening.pending).is_some_and(|record| {
                                record.qualified_document() == request.expected_document
                            })
                    })
                }
                Some(HandoffTarget::Recovery(recovery)) => self
                    .recovery
                    .as_ref()
                    .is_some_and(|episode| episode.id == recovery && episode.presented),
                Some(HandoffTarget::World) => group.world_presented,
                None => false,
            };
            if ready {
                completed.push(*id);
            }
        }
        for id in completed {
            let group = self.groups.remove(&id).expect("completed group");
            for member in group.members {
                if self
                    .records
                    .get(&member)
                    .is_none_or(|record| record.phase != Phase::Retained(id))
                {
                    continue;
                }
                let action_id = self.action_id();
                let opening = self.opening(member);
                let record = self.records.get_mut(&member).expect("retained member");
                record.phase = Phase::Destroying;
                record.terminal = Some(UiLifecycleError::StaleUiInstance);
                record.outstanding = Some(action_id);
                actions.push(UiAction::BeginDestroying { opening, action_id });
            }
        }
    }
    pub(crate) fn reconcile(&mut self, manager: &mut UiLifecycleManager, _now: Instant) {
        let removed = self
            .records
            .iter()
            .filter_map(|(id, record)| {
                let instance = UiInstanceId::from_host_id(*id);
                (record.phase == Phase::Committed && manager.instance(instance).is_none())
                    .then_some(instance)
            })
            .collect::<Vec<_>>();
        self.retain_destroyed(&removed, HandoffCause::Ordinary);
        let definitions = manager
            .instances()
            .filter(|instance| instance.visible && instance.loaded)
            .filter_map(|instance| manager.registry.resource(&instance.definition))
            .flat_map(|definition| definition.imports.values())
            .filter(|import| import.prefetch)
            .map(|import| import.resource.clone())
            .collect::<BTreeSet<_>>();
        let targets = definitions
            .into_iter()
            .filter_map(|definition| manager.resolve_resource_path(&definition, None).ok())
            .collect::<BTreeSet<_>>();
        manager.retain_prefetch_targets(&targets);
        let descriptors = manager.pending_descriptors();
        let mut available = MAX_PREPARED_UI_CANDIDATES.saturating_sub(
            descriptors
                .iter()
                .filter(|descriptor| descriptor.prefetched)
                .count(),
        );
        let pending_targets = descriptors
            .into_iter()
            .map(|descriptor| descriptor.target)
            .collect::<BTreeSet<_>>();
        for target in targets {
            if available == 0 {
                break;
            }
            let Some(definition) = manager.registry.resource(target.resource()) else {
                continue;
            };
            if manager.live_count(target.resource()) >= definition.max_instances
                || pending_targets.contains(&target)
            {
                continue;
            }
            if manager.begin_prefetch(target).is_ok() {
                available -= 1;
            }
        }
        for descriptor in manager.pending_descriptors() {
            let record = self
                .records
                .entry(descriptor.id)
                .or_insert_with(|| Record::new(descriptor.clone()));
            record.root_replacement |= manager.pending_root_replacement(descriptor.id).is_some();
            record.descriptor = descriptor;
        }
    }
    pub(crate) fn observe(&mut self, fact: NativeFact) {
        self.facts.push(fact);
    }
    pub(crate) fn committed_identity(&self, opening: UiOpeningId) -> Option<UiInstanceId> {
        (opening.owner == self.owner)
            .then(|| self.records.get(&opening.pending))
            .flatten()
            .filter(|record| record.phase == Phase::Committed)
            .map(|_| UiInstanceId::from_host_id(opening.pending))
    }
    pub(crate) fn recovery_request(&self) -> Option<(RecoveryId, &RecoverySurface)> {
        self.recovery
            .as_ref()
            .map(|episode| (episode.id, &episode.snapshot))
    }
    fn controller_quiet(&self, activity: &NativeActivity) -> bool {
        !self.creation_frozen
            && activity.parent_available
            && !activity.recovery.close_failed
            && (!activity.recovery.exists
                || (activity.recovery.scripts == Some(0)
                    && activity.recovery.navigation_pending == Some(false)))
            && activity.callback_depth == Some(0)
            && !activity.commands_pending
            && self.records.iter().all(|(id, record)| {
                !matches!(
                    record.phase,
                    Phase::Creating
                        | Phase::AwaitBlank
                        | Phase::Loading
                        | Phase::Destroying
                        | Phase::Disposing
                ) && (record.created_at.is_none()
                    || (activity.scripts.get(&self.opening(*id)) == Some(&Some(0))
                        && activity.navigation_pending.get(&self.opening(*id))
                            == Some(&Some(false))))
            })
            && !self.records.values().any(|record| {
                record.phase == Phase::Committed
                    && record
                        .presentation
                        .as_ref()
                        .is_some_and(|request| request.confirmed != Some(true))
            })
    }
    /// Called only after an empty action batch, using a fresh physical snapshot.
    pub(crate) fn can_create_recovery(&self, activity: &NativeActivity) -> bool {
        self.recovery.is_some()
            && activity.recovery.create_requested
            && !activity.recovery.close_requested
            && !activity.recovery.exists
            && self.controller_quiet(activity)
    }
    /// Abandoned recovery navigation is not a Close blocker.
    pub(crate) fn can_close_recovery(&self, activity: &NativeActivity) -> bool {
        self.recovery.is_none()
            && activity.recovery.close_requested
            && activity.recovery.exists
            && !activity.recovery.close_failed
            && activity.callback_depth == Some(0)
            && activity.recovery.scripts == Some(0)
    }
    pub(crate) fn advance(
        &mut self,
        manager: &mut UiLifecycleManager,
        now: Instant,
        activity: &NativeActivity,
    ) -> Vec<UiAction> {
        self.reconcile(manager, now);
        self.sync_recovery(manager);
        self.creation_frozen |= activity.recovery.close_failed;
        let mut actions = Vec::new();
        for fact in std::mem::take(&mut self.facts) {
            match &fact {
                NativeFact::NativeFault { message } => {
                    self.creation_frozen = true;
                    if !self.native_fault_reported {
                        self.native_fault_reported = true;
                        actions.push(UiAction::ReportFatalNativeFailure {
                            message: message.clone(),
                        });
                    }
                    continue;
                }
                NativeFact::RecoveryPresented { recovery } => {
                    if let Some(episode) = &mut self.recovery {
                        if episode.id == *recovery {
                            episode.presented = true;
                        }
                    }
                    continue;
                }
                NativeFact::WorldPresented { handoff } => {
                    if let Some(group) = self.groups.get_mut(handoff) {
                        if group.target == Some(HandoffTarget::World) {
                            group.world_presented = true;
                        }
                    }
                    continue;
                }
                _ => {}
            }
            let opening = match &fact {
                NativeFact::CreateResult { opening, .. }
                | NativeFact::BlankFinished { opening }
                | NativeFact::LoadResult { opening, .. }
                | NativeFact::NavigationStarted { opening, .. }
                | NativeFact::NavigationCompleted { opening, .. }
                | NativeFact::Handshake { opening, .. }
                | NativeFact::DocumentInstalled { opening, .. }
                | NativeFact::SourceQualificationChanged { opening, .. }
                | NativeFact::ActivationSkipped { opening, .. }
                | NativeFact::DestroyingDispatched { opening, .. }
                | NativeFact::CloseResult { opening, .. }
                | NativeFact::PresentationResult { opening, .. } => *opening,
                NativeFact::NativeFault { .. }
                | NativeFact::RecoveryPresented { .. }
                | NativeFact::WorldPresented { .. } => {
                    unreachable!("handled above")
                }
            };
            if opening.owner != self.owner {
                continue;
            }
            let Some(record) = self.records.get_mut(&opening.pending) else {
                continue;
            };
            if record.terminal.is_some()
                && !matches!(
                    fact,
                    NativeFact::CreateResult { .. }
                        | NativeFact::DestroyingDispatched { .. }
                        | NativeFact::CloseResult { .. }
                )
            {
                continue;
            }
            match fact {
                NativeFact::PresentationResult {
                    action_id, result, ..
                } if record.phase == Phase::Committed => {
                    if let Some(request) = &mut record.presentation {
                        if request.action_id == action_id && request.confirmed.is_none() {
                            request.confirmed = Some(result == Ok(request.desired.visible));
                        }
                    }
                }
                NativeFact::CreateResult {
                    action_id,
                    created_at,
                    result,
                    ..
                } if record.phase == Phase::Creating && record.outstanding == Some(action_id) => {
                    record.outstanding = None;
                    record.created_at = created_at;
                    let result = if result.is_ok() && created_at.is_none() {
                        Err(
                            "native Create success did not include actual owner creation time"
                                .into(),
                        )
                    } else {
                        result
                    };
                    match result {
                        Ok(()) => {
                            record.phase = Phase::AwaitBlank;
                        }
                        Err(message) => {
                            self.creation_frozen = true;
                            record.phase = if created_at.is_some() {
                                Phase::AwaitBlank
                            } else {
                                Phase::Queued
                            };
                            record.terminal =
                                Some(UiLifecycleError::NavigationFailed(message.clone()));
                            actions.push(UiAction::ReportFatalCreateFailure { opening, message });
                        }
                    }
                }
                NativeFact::DestroyingDispatched { action_id, .. }
                    if record.phase == Phase::Destroying
                        && record.outstanding == Some(action_id) =>
                {
                    record.outstanding = None
                }
                NativeFact::CloseResult {
                    action_id, result, ..
                } if record.phase == Phase::Disposing && record.outstanding == Some(action_id) => {
                    record.outstanding = None;
                    record.disposed = result.is_ok();
                    self.creation_frozen |= result.is_err();
                }
                NativeFact::BlankFinished { .. }
                    if matches!(record.phase, Phase::Creating | Phase::AwaitBlank) =>
                {
                    record.blank_finished = true
                }
                NativeFact::LoadResult {
                    action_id, result, ..
                } if record.phase == Phase::Loading && record.outstanding == Some(action_id) => {
                    record.outstanding = None;
                    match result {
                        Ok(()) => record.load_accepted = true,
                        Err(message) => {
                            record.terminal = Some(UiLifecycleError::NavigationFailed(message))
                        }
                    }
                }
                NativeFact::NavigationStarted {
                    target,
                    navigation_id,
                    start_revision,
                    ..
                } if matches!(record.phase, Phase::Loading | Phase::Committed) => {
                    // Same-revision replay (exact or conflicting) never rolls back
                    // an installation/source update. Only native revisions order Starts.
                    if record
                        .navigation
                        .as_ref()
                        .is_none_or(|(_, current, _)| start_revision > *current)
                    {
                        record.navigation = Some((navigation_id, start_revision, target));
                        record.navigation_result = None;
                        record.installed = None;
                        record.confirmation = None;
                        record.presentation = None;
                    }
                }
                NativeFact::DocumentInstalled { proof, target, .. }
                    if matches!(record.phase, Phase::Loading | Phase::Committed)
                        && record.matches_start(proof.navigation_id, proof.start_revision) =>
                {
                    // One installation per admitted revision; even a duplicate
                    // cannot erase confirmation or a later source qualification.
                    if record.installed.is_none()
                        && proof.document_identity > record.document_high_water
                        && proof.source_revision > record.source_high_water
                    {
                        record.document_high_water = proof.document_identity;
                        record.source_high_water = proof.source_revision;
                        record.installed = Some(InstalledDocument { proof, target });
                        record.confirmation = None;
                        record.presentation = None;
                    }
                }
                NativeFact::SourceQualificationChanged { proof, target, .. }
                    if matches!(record.phase, Phase::Loading | Phase::Committed) =>
                {
                    if let Some(document) = &mut record.installed {
                        let previous = document.proof;
                        if proof.navigation_id == previous.navigation_id
                            && proof.start_revision == previous.start_revision
                            && proof.document_identity == previous.document_identity
                            && proof.source_revision > record.source_high_water
                        {
                            record.source_high_water = proof.source_revision;
                            document.proof = proof;
                            document.target = target;
                            if record.confirmation == Some(previous) {
                                record.confirmation = Some(proof);
                            }
                            record.presentation = None;
                        }
                    }
                }
                NativeFact::NavigationCompleted {
                    navigation_id,
                    start_revision,
                    result,
                    ..
                } if matches!(record.phase, Phase::Loading | Phase::Committed)
                    && record.matches_start(navigation_id, start_revision) =>
                {
                    if record.navigation_result.is_none() {
                        if result.is_err() {
                            record.presentation = None;
                        }
                        record.navigation_result = Some(result);
                    }
                }
                NativeFact::Handshake { proof, .. }
                    if matches!(record.phase, Phase::Loading | Phase::Committed)
                        && record.installed_proof() == Some(proof) =>
                {
                    record.confirmation = Some(proof);
                }
                NativeFact::ActivationSkipped {
                    expected_document, ..
                } if record.phase == Phase::Committed
                    && record.activation_ticket == Some(expected_document) =>
                {
                    record.activation_ticket = None;
                }
                _ => {}
            }
        }
        let ids = self.records.keys().copied().collect::<Vec<_>>();
        let mut remove = Vec::new();
        for id in ids {
            let opening = self.opening(id);
            let record = self.records.get_mut(&id).expect("collected record");
            if record.disposed {
                remove.push(id);
                continue;
            }
            if record.phase == Phase::Committed {
                if let Some(proof) = record.qualified_document() {
                    // A confirmed native document may navigate within its own
                    // Definition; the original Open target is not a reload policy.
                    record.project_document_path(manager, UiInstanceId::from_host_id(id));
                    if record
                        .activation_ticket
                        .is_none_or(|ticket| ticket.document_identity != proof.document_identity)
                    {
                        record.activation_ticket = Some(proof);
                        actions.push(UiAction::ActivateCommittedAndReleaseQueued {
                            opening,
                            instance: UiInstanceId::from_host_id(id),
                            expected_document: proof,
                        });
                    }
                }
                continue;
            }
            if matches!(record.phase, Phase::Retained(_)) {
                continue;
            }
            if record.resume_pending {
                record.resume_pending = false;
                actions.push(UiAction::ResumePrepared { opening });
            }
            if manager.pending_descriptor(id).is_none() {
                record.terminal = Some(UiLifecycleError::StaleUiInstance);
            }
            let document_ready = record.load_accepted && record.qualified_document().is_some();
            if record.terminal.is_none() {
                if let Some(Err(message)) = &record.navigation_result {
                    record.terminal = Some(UiLifecycleError::NavigationFailed(message.clone()));
                } else if record.phase != Phase::Prepared
                    && record
                        .created_at
                        .is_some_and(|start| now.saturating_duration_since(start) >= self.timeout)
                    && !document_ready
                    && !(record.descriptor.prefetched && record.blank_finished)
                {
                    record.terminal = Some(UiLifecycleError::LoadTimeout);
                }
            }
            if record.terminal.is_none()
                && record.root_replacement
                && record.phase == Phase::Loading
                && document_ready
            {
                // Validate before manager's destructive Root commit. Old live counts
                // are intentionally irrelevant: replacement clears them at commit.
                let validation = manager
                    .pending_descriptor(id)
                    .filter(|_| manager.pending_root_replacement(id).is_some())
                    .and_then(|descriptor| manager.registry.resource(descriptor.target.resource()))
                    .ok_or(UiLifecycleError::StaleUiInstance)
                    .and_then(|definition| {
                        (definition.max_instances > 0)
                            .then_some(())
                            .ok_or(UiLifecycleError::UiInstanceLimit)
                    });
                if let Err(error) = validation {
                    record.terminal = Some(error);
                }
            }
            if let Some(error) = &record.terminal {
                let _ = manager.fail_open(id, error.to_string());
                if let Some(response) = record.response.take() {
                    actions.push(UiAction::SettleOpenResponse {
                        opening,
                        sender: response.sender,
                        value: error_response(response.success, error),
                    });
                }
                if record.phase == Phase::Creating {
                    continue;
                }
                if record.created_at.is_none() {
                    remove.push(id);
                    continue;
                }
                if !matches!(record.phase, Phase::Destroying | Phase::Disposing) {
                    let action_id = self.action_id();
                    let record = self.records.get_mut(&id).expect("collected record");
                    record.phase = Phase::Destroying;
                    record.outstanding = Some(action_id);
                    actions.push(UiAction::BeginDestroying { opening, action_id });
                }
                continue;
            }
            if record.phase == Phase::Prepared {
                continue;
            }
            if record.phase == Phase::Loading && document_ready {
                let root_replacement = record.root_replacement;
                let commit = if root_replacement {
                    manager
                        .commit_root_replacement(id)
                        .map(|commit| (commit.instance, commit.destroyed))
                } else {
                    manager
                        .commit_open(id)
                        .map(|instance| (instance, Vec::new()))
                };
                match commit {
                    Ok((instance, destroyed)) => {
                        let expected_document = record
                            .qualified_document()
                            .expect("ready installed document");
                        record.phase = Phase::Committed;
                        record.activation_ticket = Some(expected_document);
                        actions.push(UiAction::ActivateCommittedAndReleaseQueued {
                            opening,
                            instance,
                            expected_document,
                        });
                        if let Some(response) = record.response.take() {
                            actions.push(UiAction::SettleOpenResponse {
                                opening,
                                sender: response.sender,
                                value: response.success,
                            });
                        }
                        if root_replacement {
                            self.retain_destroyed(
                                &destroyed,
                                HandoffCause::RootReplacement(opening),
                            );
                        }
                    }
                    Err(error) => {
                        record.terminal = Some(error.clone());
                        if let Some(response) = record.response.take() {
                            actions.push(UiAction::SettleOpenResponse {
                                opening,
                                sender: response.sender,
                                value: error_response(response.success, &error),
                            });
                        }
                    }
                }
                continue;
            }
            if record.phase == Phase::AwaitBlank
                && record.blank_finished
                && record.descriptor.prefetched
            {
                record.phase = Phase::Prepared;
                actions.push(UiAction::SuspendPrepared { opening });
            }
            if record.phase == Phase::AwaitBlank
                && record.blank_finished
                && !record.descriptor.prefetched
            {
                let target = record.descriptor.target.clone();
                let action_id = self.action_id();
                let record = self.records.get_mut(&id).expect("collected record");
                record.phase = Phase::Loading;
                record.outstanding = Some(action_id);
                actions.push(UiAction::LoadDocument {
                    opening,
                    action_id,
                    target,
                });
            }
        }
        for id in remove {
            self.records.remove(&id);
        }
        self.sync_handoffs(manager, &mut actions);
        let presentation_pending = self.records.values().any(|record| {
            record.phase == Phase::Committed
                && record
                    .presentation
                    .as_ref()
                    .is_some_and(|request| request.confirmed != Some(true))
        });
        if actions.is_empty() && !presentation_pending && activity.callback_depth == Some(0) {
            if let Some(id) = self.records.iter().find_map(|(id, record)| {
                (record.phase == Phase::Destroying
                    && record.outstanding.is_none()
                    && activity.scripts.get(&self.opening(*id)) == Some(&Some(0)))
                .then_some(*id)
            }) {
                let action_id = self.action_id();
                let opening = self.opening(id);
                let record = self.records.get_mut(&id).expect("selected retirement");
                record.phase = Phase::Disposing;
                record.outstanding = Some(action_id);
                actions.push(UiAction::CloseAndDropView { opening, action_id });
            }
        }
        let may_create = actions.is_empty()
            && !activity.recovery.exists
            && !activity.recovery.create_requested
            && !activity.recovery.close_requested
            && self.controller_quiet(activity);
        if may_create {
            if let Some(id) = self.records.iter().find_map(|(id, record)| {
                (record.phase == Phase::Queued && record.terminal.is_none()).then_some(*id)
            }) {
                let action_id = self.action_id();
                let opening = self.opening(id);
                let record = self.records.get_mut(&id).expect("selected queued record");
                record.phase = Phase::Creating;
                record.outstanding = Some(action_id);
                actions.push(UiAction::CreateBlank {
                    opening,
                    action_id,
                    descriptor: record.descriptor.clone(),
                });
            }
        }
        actions
    }
}

fn error_response(success: Value, error: &UiLifecycleError) -> Value {
    let code = match error {
        UiLifecycleError::UiInstanceLimit => "ui_instance_limit",
        UiLifecycleError::DuplicatePendingOpen => "ui_open_pending",
        UiLifecycleError::UnknownImport(_) => "unknown_ui_import",
        UiLifecycleError::StaleUiInstance => "stale_ui_instance",
        UiLifecycleError::CannotCloseUiRoot => "cannot_close_ui_root",
        UiLifecycleError::Registry(_) => "invalid_ui_resource",
        UiLifecycleError::NavigationFailed(_) => "ui_navigation_failed",
        UiLifecycleError::LoadTimeout => "ui_load_timeout",
        UiLifecycleError::InternalTree(_) => "internal_ui_error",
    };
    json!({"version":1,"command":success["command"],"ok":false,"error":{"code":code,"message":error.to_string()}})
}
