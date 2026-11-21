//! World-local Controller binding and liveness checks.
use bevy::prelude::{Resource, World};
use roundo_contracts::ControllerId;
use roundo_controller::{
    AiControllerId, Controller, ControllerRegistry,
    authority_gate::{AuthorityGate, BindingId, Deadline, GateError, RequestId, Terminal},
};
use std::collections::{HashMap, HashSet};

const MAX_CONTROLLERS: usize = 64;

#[derive(Resource)]
pub struct Admission {
    gates: HashMap<ControllerId, AuthorityGate>,
    gate_entities: HashMap<ControllerId, bevy::prelude::Entity>,
    removed: HashSet<ControllerId>,
    max_span: u64,
}

impl Admission {
    pub fn new(max_span: u64) -> Self {
        Self {
            gates: HashMap::new(),
            gate_entities: HashMap::new(),
            removed: HashSet::new(),
            max_span,
        }
    }
    /// Called synchronously from the trusted Controller removal boundary. Retains
    /// the old entity identity as a tombstone: reused IDs cannot acquire or
    /// consume an old binding; drain the failed terminal before any reuse.
    pub fn controller_removed(&mut self, controller: ControllerId) {
        if let Some(gate) = self.gates.get_mut(&controller) {
            gate.take_terminal();
            gate.revoke();
        }
        // Retain the entity identity: reused numeric IDs remain denied.
        self.removed.insert(controller);
    }
    fn live_controller(
        world: &World,
        controller: ControllerId,
    ) -> Result<bevy::prelude::Entity, GateError> {
        let entity = world
            .get_resource::<ControllerRegistry>()
            .and_then(|r| r.controller_entity(controller))
            .ok_or(GateError::Unauthorized)?;
        if world
            .get::<Controller>(entity)
            .is_none_or(|c| c.id() != controller)
        {
            return Err(GateError::Unauthorized);
        }
        Ok(entity)
    }
    fn checked_gate(
        &self,
        world: &World,
        controller: ControllerId,
    ) -> Result<&AuthorityGate, GateError> {
        let entity = Self::live_controller(world, controller)?;
        if self.removed.contains(&controller)
            || self.gate_entities.get(&controller) != Some(&entity)
        {
            return Err(GateError::Unauthorized);
        }
        self.gates.get(&controller).ok_or(GateError::WrongState)
    }
    fn gate<'a>(
        gates: &'a mut HashMap<ControllerId, AuthorityGate>,
        entities: &mut HashMap<ControllerId, bevy::prelude::Entity>,
        removed: &HashSet<ControllerId>,
        world: &World,
        max_span: u64,
        controller: ControllerId,
    ) -> Result<&'a mut AuthorityGate, GateError> {
        let entity = Self::live_controller(world, controller)?;
        if removed.contains(&controller)
            || entities.get(&controller).is_some_and(|&old| old != entity)
        {
            return Err(GateError::Unauthorized);
        }
        if !gates.contains_key(&controller) {
            if gates.len() == MAX_CONTROLLERS {
                return Err(GateError::Occupied);
            }
            gates.insert(controller, AuthorityGate::new(max_span));
            entities.insert(controller, entity);
        }
        Ok(gates.get_mut(&controller).unwrap())
    }
    pub fn revoke_controller(&mut self, controller: ControllerId) {
        if let Some(gate) = self.gates.get_mut(&controller) {
            gate.revoke();
        }
    }
    pub fn take_terminal(
        &mut self,
        world: &World,
        controller: ControllerId,
    ) -> Option<Terminal> {
        if !self.removed.contains(&controller) && self.checked_gate(world, controller).is_err()
        {
            self.controller_removed(controller);
        }
        self.gates
            .get_mut(&controller)
            .and_then(AuthorityGate::take_terminal)
    }
    /// Transaction-only failure after dispatch failure or lost settlement. This
    /// never restores the previous binding or publishes Released.
    pub(crate) fn fail_transition(
        &mut self,
        world: &World,
        controller: ControllerId,
        request: RequestId,
    ) -> Result<(), GateError> {
        self.checked_gate(world, controller)?;
        self.gates.get_mut(&controller).unwrap().fail(request)
    }
    pub fn begin_ai_acquire(
        &mut self,
        world: &World,
        controller: ControllerId,
        ai: AiControllerId,
        time: Deadline,
    ) -> Result<RequestId, GateError> {
        Self::gate(
            &mut self.gates,
            &mut self.gate_entities,
            &self.removed,
            world,
            self.max_span,
            controller,
        )?
        .begin_acquire(ai, Self::live_ai(world, ai), time)
    }
    pub fn commit_ai_acquire(
        &mut self,
        world: &World,
        controller: ControllerId,
        ai: AiControllerId,
        request: RequestId,
        now: u64,
    ) -> Result<BindingId, GateError> {
        Self::gate(
            &mut self.gates,
            &mut self.gate_entities,
            &self.removed,
            world,
            self.max_span,
            controller,
        )?
        .commit_acquire(request, ai, Self::live_ai(world, ai), now, true)
    }
    pub fn begin_ai_release(
        &mut self,
        world: &World,
        controller: ControllerId,
        ai: AiControllerId,
        time: Deadline,
    ) -> Result<RequestId, GateError> {
        Self::gate(
            &mut self.gates,
            &mut self.gate_entities,
            &self.removed,
            world,
            self.max_span,
            controller,
        )?
        .begin_release(ai, Self::live_ai(world, ai), time)
    }
    /// Validate a trusted Submit already admitted before the local Release closed
    /// the gate. No fresh producer admission is granted while Draining.
    pub(crate) fn draining_ai_binding(
        &self,
        world: &World,
        controller: ControllerId,
        ai: AiControllerId,
        request: RequestId,
        binding: BindingId,
    ) -> bool {
        Self::live_ai(world, ai)
            && self
                .checked_gate(world, controller)
                .ok()
                .and_then(|gate| gate.draining_binding(request, ai))
                == Some(binding)
    }
    /// Only ECS settlement may call this after consuming the local release marker.
    pub(crate) fn commit_ai_release(
        &mut self,
        world: &World,
        controller: ControllerId,
        ai: AiControllerId,
        request: RequestId,
        now: u64,
        old_binding: BindingId,
    ) -> Result<(), GateError> {
        let gate = self.checked_gate(world, controller)?;
        if gate.draining_binding(request, ai) != Some(old_binding) {
            return Err(GateError::WrongRequest);
        }
        if !Self::live_ai(world, ai) {
            return Err(GateError::Unauthorized);
        }
        let gate = Self::gate(
            &mut self.gates,
            &mut self.gate_entities,
            &self.removed,
            world,
            self.max_span,
            controller,
        )?;
        gate.record_local_revocation(request, ai, now)?;
        gate.commit_release(request, ai, now, true)
    }
    pub fn active_ai(&self, world: &World, controller: ControllerId) -> Option<AiControllerId> {
        self.checked_gate(world, controller).ok()?.active_source()
    }
    fn live_ai(world: &World, ai: AiControllerId) -> bool {
        world
            .get_resource::<ControllerRegistry>()
            .is_some_and(|r| r.is_ai_registered(ai))
    }
    pub fn submit_ai(
        &self,
        world: &World,
        controller: ControllerId,
        ai: AiControllerId,
    ) -> Result<BindingId, GateError> {
        self.checked_gate(world, controller)?
            .submit_if_active(ai, Self::live_ai(world, ai))
    }
}
