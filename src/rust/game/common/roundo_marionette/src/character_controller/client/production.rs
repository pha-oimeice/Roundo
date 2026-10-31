//! Local production eligibility and ticket handoff lifecycle owned by Access.

use super::{
    ClientControlOutcome, ClientControllerCommandState, ClientPlayerControllerAccess,
    CreatureAuthorityUpdate,
};
use crate::character_controller::rotation::ClientRotationSyncState;
use roundo_contracts::{
    ControllerAccessLevel, ControllerBindingId, ControllerControlState, ControllerId,
    ControllerIntentDomain, ControllerOperationError, PlayerControllerAccessSnapshot,
    PlayerControllerOperation,
    control_handoff::{ControlPreparing, ControlRequestId, ControlSuccess, DrainBarrier, Freeze},
};
use std::collections::HashMap;

const PRODUCER_FREEZE_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq)]
struct FrozenProducer {
    request_id: ControlRequestId,
    installed: bool,
}

/// Bounded local closure state; overflow closes every producer until session reset.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ProducerFreeze {
    frozen: HashMap<ControllerId, FrozenProducer>,
    overflowed: bool,
}

impl ProducerFreeze {
    fn blocks(&self, controller_id: ControllerId) -> bool {
        self.overflowed || self.frozen.contains_key(&controller_id)
    }

    fn freeze(&mut self, freeze: Freeze) -> bool {
        if freeze.request_id.0 == 0
            || self.blocks(freeze.controller_id)
            || self
                .frozen
                .values()
                .any(|entry| entry.request_id == freeze.request_id)
        {
            return false;
        }
        if self.frozen.len() >= PRODUCER_FREEZE_CAPACITY {
            self.overflowed = true;
            return false;
        }
        self.frozen.insert(
            freeze.controller_id,
            FrozenProducer {
                request_id: freeze.request_id,
                installed: false,
            },
        );
        true
    }

    fn prepare(&mut self, preparing: ControlPreparing) -> bool {
        if preparing.request_id.0 == 0 || self.overflowed {
            return false;
        }
        if self.frozen.len() >= PRODUCER_FREEZE_CAPACITY
            && !self.frozen.contains_key(&preparing.controller_id)
        {
            self.overflowed = true;
            return false;
        }
        self.frozen.insert(
            preparing.controller_id,
            FrozenProducer {
                request_id: preparing.request_id,
                installed: false,
            },
        );
        true
    }

    fn matches(&self, controller_id: ControllerId, request_id: ControlRequestId) -> bool {
        !self.overflowed
            && request_id.0 != 0
            && self
                .frozen
                .get(&controller_id)
                .is_some_and(|entry| entry.request_id == request_id)
    }

    fn install(&mut self, controller_id: ControllerId, request_id: ControlRequestId) -> bool {
        if !self.matches(controller_id, request_id) {
            return false;
        }
        if let Some(entry) = self.frozen.get_mut(&controller_id) {
            entry.installed = true;
        }
        true
    }

    fn unfreeze(&mut self, controller_id: ControllerId, request_id: ControlRequestId) -> bool {
        if !self.matches(controller_id, request_id)
            || !self
                .frozen
                .get(&controller_id)
                .is_some_and(|entry| entry.installed)
        {
            return false;
        }
        self.frozen.remove(&controller_id);
        true
    }

    fn close(&mut self, controller_id: ControllerId) {
        if self.frozen.len() >= PRODUCER_FREEZE_CAPACITY
            && !self.frozen.contains_key(&controller_id)
        {
            self.overflowed = true;
        } else {
            self.frozen.insert(
                controller_id,
                FrozenProducer {
                    request_id: ControlRequestId(0),
                    installed: false,
                },
            );
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct TicketBindings {
    movement: Option<ControllerBindingId>,
    movement_controller: Option<ControllerId>,
    gaze: Option<ControllerBindingId>,
    gaze_controller: Option<ControllerId>,
}

impl ClientPlayerControllerAccess {
    /// Current local movement binding observation, not server authorization.
    /// Excludes frozen/preparing bindings and survives UI input suppression.
    pub fn active_movement_binding(&self) -> Option<(u64, ControllerId, ControllerBindingId)> {
        let controller = self.producing_controller(ControllerIntentDomain::Movement)?;
        (self.ticket_bindings.movement_controller == Some(controller)).then_some((
            self.active_epoch()?,
            controller,
            self.ticket_bindings.movement?,
        ))
    }

    /// Production permission includes session and local closure, not only projection.
    pub(crate) fn producing_controller(
        &self,
        domain: ControllerIntentDomain,
    ) -> Option<ControllerId> {
        self.active_epoch()?;
        let controller_id = match domain {
            ControllerIntentDomain::Movement => self.movement_controller(),
            ControllerIntentDomain::Gaze => self.gaze_controller(),
            ControllerIntentDomain::BlockInteraction => self.block_interaction_controller(),
        }?;
        (self.is_controlled_by_self(controller_id) && !self.frozen.blocks(controller_id))
            .then_some(controller_id)
    }

    /// Pending camera motion has a narrower gate than permission to upload gaze.
    pub(crate) fn blocks_pending_gaze(&self) -> bool {
        self.gaze_controller()
            .is_some_and(|controller_id| self.frozen.blocks(controller_id))
    }

    pub(super) fn active_epoch(&self) -> Option<u64> {
        self.epoch
    }

    pub(super) fn begin_session(
        &mut self,
        epoch: u64,
        commands: &mut ClientControllerCommandState,
        rotation: &mut ClientRotationSyncState,
    ) -> CreatureAuthorityUpdate {
        self.clear();
        self.epoch = Some(epoch);
        *commands = ClientControllerCommandState::default();
        rotation.reset();
        CreatureAuthorityUpdate::SessionStarted { epoch }
    }

    pub(super) fn end_session(
        &mut self,
        epoch: u64,
        commands: &mut ClientControllerCommandState,
        rotation: &mut ClientRotationSyncState,
    ) -> Option<CreatureAuthorityUpdate> {
        if self.active_epoch() != Some(epoch) {
            return None;
        }
        self.clear();
        *commands = ClientControllerCommandState::default();
        rotation.reset();
        Some(CreatureAuthorityUpdate::SessionEnded { epoch })
    }

    pub(super) fn apply_freeze(
        &mut self,
        epoch: u64,
        freeze: Freeze,
        rotation: &mut ClientRotationSyncState,
    ) -> Option<DrainBarrier> {
        if self.active_epoch() != Some(epoch) || !self.frozen.freeze(freeze) {
            return None;
        }
        if self.gaze_controller() == Some(freeze.controller_id) {
            rotation.discard_pending();
        }
        Some(DrainBarrier {
            request_id: freeze.request_id,
            controller_id: freeze.controller_id,
            session_epoch: epoch,
        })
    }

    pub(super) fn apply_preparing(
        &mut self,
        epoch: u64,
        preparing: ControlPreparing,
        commands: &mut ClientControllerCommandState,
        rotation: &mut ClientRotationSyncState,
    ) -> Option<CreatureAuthorityUpdate> {
        if self.active_epoch() != Some(epoch)
            || !(self.movement_controller() == Some(preparing.controller_id)
                || self.gaze_controller() == Some(preparing.controller_id)
                || self.block_interaction_controller() == Some(preparing.controller_id)
                || self.ticket_bindings.movement_controller == Some(preparing.controller_id)
                || self.ticket_bindings.gaze_controller == Some(preparing.controller_id))
            || !self.frozen.prepare(preparing)
        {
            return None;
        }
        self.revoke_bindings(preparing.controller_id, commands, rotation);
        if self.block_interaction_controller() == Some(preparing.controller_id) {
            commands.block_sequence = 0;
        }
        if self.movement_controller() == Some(preparing.controller_id) {
            commands.movement = Default::default();
        }
        if self.gaze_controller() == Some(preparing.controller_id) {
            rotation.reset();
        }
        self.set_snapshot_control_state(
            preparing.controller_id,
            ControllerControlState::Uncontrolled,
        );
        Some(CreatureAuthorityUpdate::PreparingRevoked {
            epoch,
            preparing,
            movement: self.ticket_bindings.movement,
            gaze: self.ticket_bindings.gaze,
        })
    }

    pub(super) fn apply_ticket_acquired(
        &mut self,
        epoch: u64,
        request_id: ControlRequestId,
        controller_id: ControllerId,
        binding: ControllerBindingId,
    ) -> Option<CreatureAuthorityUpdate> {
        if self.active_epoch() != Some(epoch)
            || !(self.movement_controller() == Some(controller_id)
                || self.gaze_controller() == Some(controller_id)
                || self.block_interaction_controller() == Some(controller_id))
            || !self.frozen.install(controller_id, request_id)
        {
            return None;
        }
        if self.movement_controller() == Some(controller_id) {
            self.ticket_bindings.movement = Some(binding);
            self.ticket_bindings.movement_controller = Some(controller_id);
        }
        if self.gaze_controller() == Some(controller_id) {
            self.ticket_bindings.gaze = Some(binding);
            self.ticket_bindings.gaze_controller = Some(controller_id);
        }
        // Block has no prediction binding slot, but install still marks its entry.
        Some(CreatureAuthorityUpdate::BindingInstalled {
            epoch,
            request_id,
            controller_id,
            movement: self.ticket_bindings.movement,
            gaze: self.ticket_bindings.gaze,
        })
    }

    pub(super) fn apply_ticket_activated(
        &mut self,
        epoch: u64,
        request_id: ControlRequestId,
        controller_id: ControllerId,
    ) -> bool {
        if self.active_epoch() != Some(epoch)
            || !(self.ticket_bindings.movement_controller == Some(controller_id)
                || self.ticket_bindings.gaze_controller == Some(controller_id)
                || self.block_interaction_controller() == Some(controller_id))
            || !self.frozen.unfreeze(controller_id, request_id)
        {
            return false;
        }
        self.set_snapshot_control_state(controller_id, ControllerControlState::ControlledBySelf);
        true
    }

    pub(super) fn apply_ticket_released(
        &mut self,
        epoch: u64,
        request_id: ControlRequestId,
        controller_id: ControllerId,
        commands: &mut ClientControllerCommandState,
        rotation: &mut ClientRotationSyncState,
    ) -> Option<CreatureAuthorityUpdate> {
        if self.active_epoch() != Some(epoch) {
            return None;
        }
        self.frozen.close(controller_id);
        self.revoke_bindings(controller_id, commands, rotation);
        self.set_snapshot_control_state(controller_id, ControllerControlState::Uncontrolled);
        Some(CreatureAuthorityUpdate::BindingClosed {
            epoch,
            request_id,
            controller_id,
            movement: self.ticket_bindings.movement,
            gaze: self.ticket_bindings.gaze,
        })
    }

    pub(super) fn apply_ticket_completed(
        &mut self,
        epoch: u64,
        request_id: ControlRequestId,
        controller_id: ControllerId,
        outcome: ClientControlOutcome,
        commands: &mut ClientControllerCommandState,
        rotation: &mut ClientRotationSyncState,
    ) {
        if self.active_epoch() != Some(epoch) {
            return;
        }
        if !matches!(
            outcome,
            ClientControlOutcome::Terminal(Ok(ControlSuccess::Acquired(_)))
        ) {
            self.frozen.close(controller_id);
            self.revoke_bindings(controller_id, commands, rotation);
        }
        // Completion is an observation, not a second local settlement authority.
        self.last_control_result = Some((request_id, controller_id, outcome));
        self.control_completion_count = self.control_completion_count.saturating_add(1);
        self.record_control_completion(request_id);
    }

    pub(super) fn apply_access_snapshot(
        &mut self,
        epoch: u64,
        snapshot: PlayerControllerAccessSnapshot,
        rotation: &mut ClientRotationSyncState,
    ) -> Vec<ControllerId> {
        if self.active_epoch() != Some(epoch) {
            return Vec::new();
        }
        let select = |domain| {
            snapshot.controllers.iter().find_map(|entry| {
                (entry.intent_domain == domain
                    && entry.access_level == ControllerAccessLevel::ReadWrite)
                    .then_some(entry.controller_id)
            })
        };
        let movement = select(ControllerIntentDomain::Movement);
        let gaze = select(ControllerIntentDomain::Gaze);
        let block = select(ControllerIntentDomain::BlockInteraction);
        if self.gaze != gaze {
            rotation.discard_pending();
        }
        self.movement = movement;
        self.gaze = gaze;
        self.snapshot = Some(snapshot);
        self.last_rejection = None;
        [movement, gaze, block]
            .into_iter()
            .flatten()
            .filter(|controller_id| {
                !self.frozen.blocks(*controller_id)
                    && self.snapshot.as_ref().is_some_and(|snapshot| {
                        snapshot.controllers.iter().any(|entry| {
                            entry.controller_id == *controller_id
                                && entry.control_state == ControllerControlState::Uncontrolled
                        })
                    })
            })
            .collect()
    }

    pub(super) fn apply_legacy_control_state(
        &mut self,
        epoch: u64,
        controller_id: ControllerId,
        control_state: ControllerControlState,
    ) {
        if self.active_epoch() == Some(epoch) {
            self.set_snapshot_control_state(controller_id, control_state);
        }
    }

    pub(super) fn apply_operation_rejected(
        &mut self,
        epoch: u64,
        operation: PlayerControllerOperation,
        error: ControllerOperationError,
    ) {
        if self.active_epoch() == Some(epoch) {
            self.last_rejection = Some((operation, error));
        }
    }

    /// Shared binding revocation; selected-domain extras belong only to Preparing.
    fn revoke_bindings(
        &mut self,
        controller_id: ControllerId,
        commands: &mut ClientControllerCommandState,
        rotation: &mut ClientRotationSyncState,
    ) {
        if self.ticket_bindings.movement_controller == Some(controller_id) {
            self.ticket_bindings.movement = None;
            self.ticket_bindings.movement_controller = None;
            commands.movement = Default::default();
        }
        if self.ticket_bindings.gaze_controller == Some(controller_id) {
            self.ticket_bindings.gaze = None;
            self.ticket_bindings.gaze_controller = None;
            rotation.reset();
        }
    }

    fn set_snapshot_control_state(
        &mut self,
        controller_id: ControllerId,
        control_state: ControllerControlState,
    ) {
        if let Some(entry) = self.snapshot.as_mut().and_then(|snapshot| {
            snapshot
                .controllers
                .iter_mut()
                .find(|entry| entry.controller_id == controller_id)
        }) {
            entry.control_state = control_state;
        }
    }
}
