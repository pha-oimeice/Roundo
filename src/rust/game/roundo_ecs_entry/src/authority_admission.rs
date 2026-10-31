//! Serialized ECS admission and Controller authority gate.
//! Calls (including authenticated join/disconnect and gate transitions) require
//! exclusive `&mut self`; a caller must invoke them on the same serialized ingress.
//! Integration must route Access changes and Controller removal through the callbacks
//! before consuming terminals; the revision also rejects direct Access downgrade+
//! restore between submits, but cannot synchronously emit its failed terminal.
use bevy::prelude::{Resource, World};
use roundo_contracts::{ConnectionId, ControllerAccessLevel, ControllerId, PlayerId};
use roundo_controller::{
    AiControllerId, Controller, ControllerRegistry,
    authority_gate::{
        AdmittedConnection, AuthorityFacts, AuthorityGate, BindingId, Deadline, GateError,
        GateState, RequestId, Source, Terminal,
    },
};
use roundo_player::{Player, PlayerRegistry};
use std::collections::{HashMap, HashSet};

const MAX_ADMITTED: usize = 64;
const MAX_CONTROLLERS: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionError {
    Unassociated,
    Full,
    Exhausted,
    Quarantined,
}

#[derive(Resource)]
pub struct Admission {
    next_generation: u64,
    admitted: HashMap<ConnectionId, (PlayerId, AdmittedConnection)>,
    quarantined: HashSet<PlayerId>,
    gates: HashMap<ControllerId, AuthorityGate>,
    gate_entities: HashMap<ControllerId, bevy::prelude::Entity>,
    removed: HashSet<ControllerId>,
    max_span: u64,
}

struct Facts<'a> {
    world: &'a World,
    controller: ControllerId,
    admitted: &'a HashMap<ConnectionId, (PlayerId, AdmittedConnection)>,
    quarantined: &'a HashSet<PlayerId>,
}
impl AuthorityFacts for Facts<'_> {
    fn player_access(&self, player: PlayerId) -> ControllerAccessLevel {
        self.world
            .get_resource::<PlayerRegistry>()
            .and_then(|r| r.player_entity(player))
            .and_then(|e| self.world.get::<Player>(e))
            .map(|p| p.controller_access(self.controller))
            .unwrap_or_default()
    }
    fn player_access_revision(&self, player: PlayerId) -> Option<u64> {
        self.world
            .get_resource::<PlayerRegistry>()
            .and_then(|r| r.player_entity(player))
            .and_then(|e| self.world.get::<Player>(e))
            .and_then(|p| p.controller_access_revision(self.controller))
    }
    fn sole_admitted_for(&self, player: PlayerId, connection: AdmittedConnection) -> bool {
        !self.quarantined.contains(&player)
            && self
                .world
                .get_resource::<PlayerRegistry>()
                .is_some_and(|r| r.player_for_connection(connection.id) == Some(player))
            && self.admitted.get(&connection.id) == Some(&(player, connection))
            && self.admitted.values().filter(|(p, _)| *p == player).count() == 1
    }
    fn live_ai(&self, ai: AiControllerId) -> bool {
        self.world
            .get_resource::<ControllerRegistry>()
            .is_some_and(|r| r.is_ai_registered(ai))
    }
}

impl Admission {
    pub fn new(max_span: u64) -> Self {
        Self {
            next_generation: 1,
            admitted: HashMap::new(),
            quarantined: HashSet::new(),
            gates: HashMap::new(),
            gate_entities: HashMap::new(),
            removed: HashSet::new(),
            max_span,
        }
    }
    /// Trusted server callback only, after server identity resolution and Player binding.
    /// A duplicate/replacement ID or second Player connection quarantines that Player;
    /// no snapshot of existing producers may authorize the newcomer during a drain.
    pub fn authenticated_connected(
        &mut self,
        world: &World,
        id: ConnectionId,
    ) -> Result<AdmittedConnection, AdmissionError> {
        let player = world
            .get_resource::<PlayerRegistry>()
            .and_then(|r| r.player_for_connection(id))
            .ok_or(AdmissionError::Unassociated)?;
        if self.admitted.contains_key(&id)
            || self.quarantined.contains(&player)
            || self.admitted.values().any(|(p, _)| *p == player)
        {
            if self
                .admitted
                .values()
                .any(|(admitted, _)| *admitted == player)
            {
                self.quarantined.insert(player);
                self.revoke_player(player);
            }
            if let Some(&(previous, _)) = self.admitted.get(&id) {
                self.quarantined.insert(previous);
                self.revoke_player(previous);
            }
            return Err(AdmissionError::Quarantined);
        }
        if self.admitted.len() == MAX_ADMITTED {
            return Err(AdmissionError::Full);
        }
        let next = self
            .next_generation
            .checked_add(1)
            .ok_or(AdmissionError::Exhausted)?;
        let connection = AdmittedConnection {
            id,
            generation: self.next_generation,
        };
        self.next_generation = next;
        self.admitted.insert(id, (player, connection));
        Ok(connection)
    }
    pub fn authenticated_disconnected(&mut self, connection: AdmittedConnection) {
        if let Some(&(player, current)) = self.admitted.get(&connection.id) {
            if current == connection {
                self.admitted.remove(&connection.id);
                self.revoke_player(player);
                if !self.admitted.values().any(|(p, _)| *p == player) {
                    self.quarantined.remove(&player);
                }
                // Explicit fresh admission is required; no implicit reopen.
            }
        }
    }
    /// Serialized access mutation seam for eventual integration. All writers must
    /// use this boundary; raw Player mutations still advance the revision so a
    /// revoke-and-restore interval cannot reauthorize an old binding.
    pub fn set_player_access(
        &mut self,
        world: &mut World,
        player: PlayerId,
        controller: ControllerId,
        level: ControllerAccessLevel,
    ) -> Result<(), GateError> {
        Self::live_controller(world, controller)?;
        let entity = world
            .get_resource::<PlayerRegistry>()
            .and_then(|r| r.player_entity(player))
            .ok_or(GateError::Unauthorized)?;
        let mut target = world
            .get_mut::<Player>(entity)
            .ok_or(GateError::Unauthorized)?;
        let old = target.controller_access(controller);
        target.set_controller_access(controller, level);
        if old == ControllerAccessLevel::ReadWrite && level != ControllerAccessLevel::ReadWrite {
            if let Some(gate) = self.gates.get_mut(&controller) {
                if gate.belongs_to_player(player) {
                    gate.revoke();
                }
            }
        }
        Ok(())
    }
    pub fn local_ai_removed(&mut self, ai: AiControllerId) {
        for gate in self.gates.values_mut() {
            // Revocation of unrelated gates would destroy another owner's transition.
            if gate.belongs_to(Source::LocalAi(ai)) {
                gate.revoke();
            }
        }
    }
    fn revoke_player(&mut self, player: PlayerId) {
        for gate in self.gates.values_mut() {
            if gate.belongs_to_player(player) {
                gate.revoke();
            }
        }
    }
    fn facts<'a>(&'a self, world: &'a World, controller: ControllerId) -> Facts<'a> {
        Facts {
            world,
            controller,
            admitted: &self.admitted,
            quarantined: &self.quarantined,
        }
    }
    fn player_source(&self, connection: AdmittedConnection) -> Result<Source, GateError> {
        let &(player, current) = self
            .admitted
            .get(&connection.id)
            .ok_or(GateError::Unauthorized)?;
        if current != connection || self.quarantined.contains(&player) {
            return Err(GateError::Unauthorized);
        }
        Ok(Source::Player { player, connection })
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
    /// Expiry is caller-driven on the same authoritative non-wrapping clock.
    pub fn expire(
        &mut self,
        world: &World,
        controller: ControllerId,
        now: u64,
    ) -> Result<(), GateError> {
        Self::gate(
            &mut self.gates,
            &mut self.gate_entities,
            &self.removed,
            world,
            self.max_span,
            controller,
        )?
        .expire(now)
    }
    pub fn state(&self, world: &World, controller: ControllerId) -> GateState {
        self.checked_gate(world, controller)
            .map_or(GateState::Closed, AuthorityGate::state)
    }
    pub fn active_binding(&self, world: &World, controller: ControllerId) -> Option<BindingId> {
        let entity = Self::live_controller(world, controller).ok()?;
        self.active_binding_for_entity(controller, entity)
    }
    /// Read adapter seam: the caller must resolve the current registry entity and
    /// verify its Controller ID first. The gate independently rejects tombstones
    /// and entity replacement; supplying an old identity cannot reopen a binding.
    pub(crate) fn active_binding_for_entity(
        &self,
        controller: ControllerId,
        entity: bevy::prelude::Entity,
    ) -> Option<BindingId> {
        if self.removed.contains(&controller)
            || self.gate_entities.get(&controller) != Some(&entity)
        {
            return None;
        }
        self.gates.get(&controller)?.active_binding()
    }
    /// Typed composition may provide current facts without obtaining World access.
    pub(crate) fn submit_player_with_facts(
        &self,
        controller: ControllerId,
        entity: bevy::prelude::Entity,
        player: PlayerId,
        connection: AdmittedConnection,
        facts: &impl AuthorityFacts,
    ) -> Result<BindingId, GateError> {
        if self.active_binding_for_entity(controller, entity).is_none()
            || self.admitted.get(&connection.id) != Some(&(player, connection))
        {
            return Err(GateError::Unauthorized);
        }
        self.gates
            .get(&controller)
            .ok_or(GateError::Unauthorized)?
            .submit_if_active(Source::Player { player, connection }, facts)
    }

    pub fn active_player(&self, controller: ControllerId) -> Option<(ConnectionId, PlayerId)> {
        let gate = self.gates.get(&controller)?;
        if gate.active_binding().is_none() {
            return None;
        }
        self.admitted.values().find_map(|(player, connection)| {
            (gate.belongs_to(Source::Player {
                player: *player,
                connection: *connection,
            }) && !self.quarantined.contains(player))
            .then_some((connection.id, *player))
        })
    }
    pub fn snapshot_binding(
        &self,
        controller: ControllerId,
        connection: ConnectionId,
    ) -> Option<roundo_contracts::ControllerBindingId> {
        (self.active_player(controller)?.0 == connection)
            .then(|| {
                self.gates
                    .get(&controller)?
                    .active_binding()
                    .map(Into::into)
            })
            .flatten()
    }
    pub fn connection(&self, id: ConnectionId) -> Option<AdmittedConnection> {
        self.admitted.get(&id).map(|(_, connection)| *connection)
    }
    pub fn revoke_connection(&mut self, id: ConnectionId) {
        if let Some(connection) = self.connection(id) {
            self.authenticated_disconnected(connection);
        }
    }
    pub fn revoke_controller(&mut self, controller: ControllerId) {
        if let Some(gate) = self.gates.get_mut(&controller) {
            gate.revoke();
        }
    }
    pub fn take_terminal(&mut self, world: &World, controller: ControllerId) -> Option<Terminal> {
        if !self.removed.contains(&controller) && self.checked_gate(world, controller).is_err() {
            self.controller_removed(controller);
        }
        self.gates
            .get_mut(&controller)
            .and_then(AuthorityGate::take_terminal)
    }
    pub fn begin_player_acquire(
        &mut self,
        world: &World,
        controller: ControllerId,
        connection: AdmittedConnection,
        time: Deadline,
    ) -> Result<RequestId, GateError> {
        let source = self.player_source(connection)?;
        let Source::Player { player, .. } = source else {
            unreachable!()
        };
        let facts = Facts {
            world,
            controller,
            admitted: &self.admitted,
            quarantined: &self.quarantined,
        };
        if facts.player_access(player) != ControllerAccessLevel::ReadWrite
            || !facts.sole_admitted_for(player, connection)
        {
            return Err(GateError::Unauthorized);
        }
        Self::gate(
            &mut self.gates,
            &mut self.gate_entities,
            &self.removed,
            world,
            self.max_span,
            controller,
        )?
        .begin_acquire(source, &facts, time)
    }
    pub fn commit_player_acquire(
        &mut self,
        world: &World,
        controller: ControllerId,
        connection: AdmittedConnection,
        request: RequestId,
        now: u64,
        ready: bool,
    ) -> Result<BindingId, GateError> {
        let source = self.player_source(connection)?;
        let facts = Facts {
            world,
            controller,
            admitted: &self.admitted,
            quarantined: &self.quarantined,
        };
        Self::gate(
            &mut self.gates,
            &mut self.gate_entities,
            &self.removed,
            world,
            self.max_span,
            controller,
        )?
        .commit_acquire(request, source, &facts, now, ready)
    }
    pub fn begin_player_release(
        &mut self,
        world: &World,
        controller: ControllerId,
        connection: AdmittedConnection,
        time: Deadline,
    ) -> Result<RequestId, GateError> {
        let source = self.player_source(connection)?;
        let facts = Facts {
            world,
            controller,
            admitted: &self.admitted,
            quarantined: &self.quarantined,
        };
        Self::gate(
            &mut self.gates,
            &mut self.gate_entities,
            &self.removed,
            world,
            self.max_span,
            controller,
        )?
        .begin_release(source, &facts, time)
    }
    pub(crate) fn record_player_prefix(
        &mut self,
        world: &World,
        controller: ControllerId,
        connection: AdmittedConnection,
        request: RequestId,
        now: u64,
    ) -> Result<(), GateError> {
        self.player_source(connection)?;
        Self::gate(
            &mut self.gates,
            &mut self.gate_entities,
            &self.removed,
            world,
            self.max_span,
            controller,
        )?
        .record_prefix(request, connection, now)
    }
    pub(crate) fn commit_player_release(
        &mut self,
        world: &World,
        controller: ControllerId,
        connection: AdmittedConnection,
        request: RequestId,
        now: u64,
        certificate: crate::player_control::SettlementCertificate,
    ) -> Result<(), GateError> {
        let source = self.player_source(connection)?;
        let Source::Player { player, .. } = source else {
            unreachable!()
        };
        let old_binding = self
            .checked_gate(world, controller)?
            .draining_binding(request, source)
            .ok_or(GateError::WrongRequest)?;
        if !certificate.matches(connection, player, controller, request, old_binding) {
            return Err(GateError::MissingProof);
        }
        Self::gate(
            &mut self.gates,
            &mut self.gate_entities,
            &self.removed,
            world,
            self.max_span,
            controller,
        )?
        .commit_release(request, source, now, true)
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
    pub fn submit_player(
        &self,
        world: &World,
        controller: ControllerId,
        connection: AdmittedConnection,
    ) -> Result<BindingId, GateError> {
        let source = self.player_source(connection)?;
        self.checked_gate(world, controller)?
            .submit_if_active(source, &self.facts(world, controller))
    }
    pub fn begin_ai_acquire(
        &mut self,
        world: &World,
        controller: ControllerId,
        ai: AiControllerId,
        time: Deadline,
    ) -> Result<RequestId, GateError> {
        let facts = Facts {
            world,
            controller,
            admitted: &self.admitted,
            quarantined: &self.quarantined,
        };
        Self::gate(
            &mut self.gates,
            &mut self.gate_entities,
            &self.removed,
            world,
            self.max_span,
            controller,
        )?
        .begin_acquire(Source::LocalAi(ai), &facts, time)
    }
    pub fn commit_ai_acquire(
        &mut self,
        world: &World,
        controller: ControllerId,
        ai: AiControllerId,
        request: RequestId,
        now: u64,
    ) -> Result<BindingId, GateError> {
        let facts = Facts {
            world,
            controller,
            admitted: &self.admitted,
            quarantined: &self.quarantined,
        };
        Self::gate(
            &mut self.gates,
            &mut self.gate_entities,
            &self.removed,
            world,
            self.max_span,
            controller,
        )?
        .commit_acquire(request, Source::LocalAi(ai), &facts, now, true)
    }
    pub fn begin_ai_release(
        &mut self,
        world: &World,
        controller: ControllerId,
        ai: AiControllerId,
        time: Deadline,
    ) -> Result<RequestId, GateError> {
        let facts = Facts {
            world,
            controller,
            admitted: &self.admitted,
            quarantined: &self.quarantined,
        };
        Self::gate(
            &mut self.gates,
            &mut self.gate_entities,
            &self.removed,
            world,
            self.max_span,
            controller,
        )?
        .begin_release(Source::LocalAi(ai), &facts, time)
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
        self.facts(world, controller).live_ai(ai)
            && self
                .checked_gate(world, controller)
                .ok()
                .and_then(|gate| gate.draining_binding(request, Source::LocalAi(ai)))
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
        if gate.draining_binding(request, Source::LocalAi(ai)) != Some(old_binding) {
            return Err(GateError::WrongRequest);
        }
        let facts = self.facts(world, controller);
        if !facts.live_ai(ai) {
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
        gate.commit_release(request, Source::LocalAi(ai), now, true)
    }
    pub fn active_ai(&self, world: &World, controller: ControllerId) -> Option<AiControllerId> {
        let entity = Self::live_controller(world, controller).ok()?;
        self.active_ai_for_entity(controller, entity)
    }
    /// Read adapter seam with the same identity contract as
    /// `active_binding_for_entity`: resolve the live registry Entity and verify
    /// its Controller ID before calling. This reports occupancy, not Submit
    /// authorization (which also validates the source's current facts).
    pub(crate) fn active_ai_for_entity(
        &self,
        controller: ControllerId,
        entity: bevy::prelude::Entity,
    ) -> Option<AiControllerId> {
        self.active_binding_for_entity(controller, entity)?;
        match self.gates.get(&controller)?.active_source()? {
            Source::LocalAi(ai) => Some(ai),
            _ => None,
        }
    }
    pub fn submit_ai(
        &self,
        world: &World,
        controller: ControllerId,
        ai: AiControllerId,
    ) -> Result<BindingId, GateError> {
        self.checked_gate(world, controller)?
            .submit_if_active(Source::LocalAi(ai), &self.facts(world, controller))
    }
}
