//! Read authorization, subject resolution and immutable snapshot publication.

use super::*;
use bevy::{ecs::system::SystemParam, prelude::ResMut};
use roundo_creature::{FactoryMotionReceipts, FactoryMotionRoutes};
use roundo_life::LifeInstances;

/// Current Player Read/ReadWrite authorization and owned Record projection.
/// All identity, membership and role checks are repeated at use time. This is a
/// read-only parameter: no World, admission mutation or lifecycle grant escapes.
#[derive(SystemParam)]
pub struct CreatureObservations<'w, 's> {
    instances: LifeInstances<'w, 's>,
    routes: FactoryMotionRoutes<'w, 's, CreatureAssemblyRegistry>,
    player_registry: Option<Res<'w, PlayerRegistry>>,
    controller_registry: Option<Res<'w, ControllerRegistry>>,
    assemblies: Option<Res<'w, CreatureAssemblyRegistry>>,
    players: Query<'w, 's, &'static Player>,
    controllers: Query<'w, 's, (Entity, &'static Controller)>,
    intent_routes: Query<'w, 's, &'static FactoryMotionRoute>,
    controller_ids: Query<'w, 's, &'static TestLifeControllers>,
    interaction_ids: Query<'w, 's, &'static TestInteractionController>,
    records: Query<'w, 's, &'static CreatureMotionRecord>,
}

impl CreatureObservations<'_, '_> {
    fn current_player_for_connection(
        &self,
        connection_id: ConnectionId,
    ) -> Option<(roundo_contracts::PlayerId, &Player)> {
        let registry = self.player_registry.as_ref()?;
        let id = registry.player_for_connection(connection_id)?;
        let player = self.players.get(registry.player_entity(id)?).ok()?;
        (player.id() == id).then_some((id, player))
    }

    fn live_creature_controller(
        &self,
        controller_id: ControllerId,
    ) -> Option<LiveCreatureController> {
        let registry = self.controller_registry.as_ref()?;
        let entity = registry.controller_entity(controller_id)?;
        let (_, controller) = self.controllers.get(entity).ok()?;
        if controller.id() != controller_id {
            return None;
        }
        let intent_handle = controller.intent_body_handle()?;
        let intent_entity = self.instances.resolve_body(intent_handle).ok()?;
        let actual_route = *self.intent_routes.get(intent_entity).ok()?;
        let assemblies = self.assemblies.as_ref()?;
        let life = assemblies.life_for_controller(controller_id)?;
        if !assemblies.accepts_controller_input(life)
            || assemblies.life_for_body(intent_handle) != Some(life)
        {
            return None;
        }
        let members = assemblies.get(life)?;
        let expected =
            FactoryMotionRoute::new(life, members.intent, members.execution, members.record);
        if members.intent != intent_handle
            || actual_route != expected
            || !self.routes.is_live(actual_route)
        {
            return None;
        }
        let execution = self.instances.resolve_body(members.execution).ok()?;
        let controller_ids = *self.controller_ids.get(execution).ok()?;
        if controller_ids.movement == controller_ids.gaze {
            return None;
        }
        let domain = controller.domain();
        match domain {
            domain
                if domain == IntentDomain::movement()
                    && controller_ids.movement == controller_id => {}
            domain if domain == IntentDomain::gaze() && controller_ids.gaze == controller_id => {}
            domain
                if domain == IntentDomain::block_interaction()
                    && self
                        .interaction_ids
                        .get(execution)
                        .is_ok_and(|id| id.0 == controller_id) => {}
            _ => return None,
        }
        Some(LiveCreatureController {
            life,
            route: actual_route,
            domain,
            controller_ids,
        })
    }

    /// Copies the authorized Life's Record, including a spawn baseline or frozen
    /// value. Unlike Snapshot publication, no current commit receipt is required.
    pub fn project(
        &self,
        connection_id: ConnectionId,
        life: LifeHandle,
    ) -> Option<CreatureObservation> {
        let (_, player) = self.current_player_for_connection(connection_id)?;
        let route = player.controller_accesses().find_map(|(id, level)| {
            if !matches!(
                level,
                ControllerAccessLevel::Read | ControllerAccessLevel::ReadWrite
            ) {
                return None;
            }
            let live = self.live_creature_controller(id)?;
            (live.life == life && live.domain != IntentDomain::block_interaction())
                .then_some(live.route)
        })?;
        let record_entity = self.instances.resolve_body(route.record()).ok()?;
        Some(CreatureObservation::from_record(
            self.records.get(record_entity).ok()?,
        ))
    }
}

/// Updates only Presence projections, never the Execution's physical pose.
pub(super) fn project_creature_presence(
    observations: CreatureObservations,
    registry: Res<PlayerRegistry>,
    owners: Query<&BootstrappedTestLife>,
    mut projections: Query<
        (&roundo_presence::PlayerConnection, &mut Transform),
        With<roundo_presence::ServerPlayer>,
    >,
) {
    for (connection, mut transform) in &mut projections {
        let Some(player) = registry.player_for_connection(connection.0) else {
            continue;
        };
        let Some(owner) = registry
            .player_entity(player)
            .and_then(|entity| owners.get(entity).ok())
        else {
            continue;
        };
        let Some(value) = observations.project(connection.0, owner.life) else {
            continue;
        };
        transform.translation = bevy::prelude::Vec3::from_array(*value.translation());
        transform.rotation = bevy::prelude::Quat::from_array(*value.gaze_orientation());
    }
}

/// Projects an owned Record only after current connected-Player authorization.
/// Use the read-only parameter in an ordinary system; a copied Handle is not a grant.
pub fn project_authorized_creature_observation(
    observations: &CreatureObservations,
    connection_id: ConnectionId,
    life: LifeHandle,
) -> Option<CreatureObservation> {
    observations.project(connection_id, life)
}

#[derive(SystemParam)]
pub(super) struct SnapshotAuthorization<'w, 's> {
    observations: CreatureObservations<'w, 's>,
    admission: Option<Res<'w, crate::authority_admission::Admission>>,
}

impl SnapshotAuthorization<'_, '_> {
    pub(super) fn live_admitted_connection(
        &self,
        connection_id: ConnectionId,
    ) -> Option<(roundo_contracts::PlayerId, u64)> {
        let admitted = self.admission.as_ref()?.connection(connection_id)?;
        let (player_id, _) = self
            .observations
            .current_player_for_connection(connection_id)?;
        (admitted.id == connection_id).then_some((player_id, admitted.generation))
    }

    fn active_player_controllers(&self) -> HashMap<ConnectionId, Vec<ActivePlayerController>> {
        let mut grouped = HashMap::<ConnectionId, Vec<ActivePlayerController>>::new();
        let Some(admission) = self.admission.as_ref() else {
            return grouped;
        };
        let Some(registry) = self.observations.controller_registry.as_ref() else {
            return grouped;
        };
        for (entity, controller) in &self.observations.controllers {
            let controller_id = controller.id();
            let Some((connection_id, player_id)) = admission.active_player(controller_id) else {
                continue;
            };
            let connection = self.live_admitted_connection(connection_id);
            let player_is_current = connection.is_some_and(|(current, _)| current == player_id);
            let current_access = self
                .observations
                .current_player_for_connection(connection_id)
                .map(|(_, player)| player.controller_access(controller_id));
            let access_is_readable = matches!(
                current_access,
                Some(ControllerAccessLevel::Read | ControllerAccessLevel::ReadWrite)
            );
            let controller_is_current = registry.controller_entity(controller_id) == Some(entity);
            let active_binding = controller_is_current
                .then(|| admission.active_binding_for_entity(controller_id, entity))
                .flatten()
                .map(Into::into);
            let binding_is_current = active_binding.is_some()
                && admission.snapshot_binding(controller_id, connection_id) == active_binding;
            grouped
                .entry(connection_id)
                .or_default()
                .push(ActivePlayerController {
                    entity,
                    controller_id,
                    player_id,
                    connection_generation: connection.map(|(_, generation)| generation),
                    binding: active_binding,
                    valid: player_is_current
                        && access_is_readable
                        && binding_is_current
                        && controller_is_current,
                });
        }
        grouped
    }

    pub(super) fn resolve_snapshot_subject(
        &self,
        connection_id: ConnectionId,
        tick: u64,
    ) -> Option<SnapshotSubject> {
        let (player_id, generation) = self.live_admitted_connection(connection_id)?;
        let mut grouped = self.active_player_controllers();
        let mut active = grouped.remove(&connection_id)?;
        if active.is_empty()
            || active.iter().any(|controller| {
                !controller.valid
                    || controller.player_id != player_id
                    || controller.connection_generation != Some(generation)
                    || controller.binding.is_none()
            })
        {
            return None;
        }
        active.sort_unstable_by_key(|controller| controller.controller_id);
        let mut life = None;
        let mut route = None;
        let mut controller_ids = None;
        let mut movement_binding = None;
        let mut gaze_binding = None;
        let mut movement_seen = false;
        let mut gaze_seen = false;
        for active_controller in active {
            let live = self
                .observations
                .live_creature_controller(active_controller.controller_id)?;
            if live.life != live.route.life()
                || life.is_some_and(|current| current != live.life)
                || route.is_some_and(|current| current != live.route)
                || controller_ids.is_some_and(|current| current != live.controller_ids)
            {
                return None;
            }
            life = Some(live.life);
            route = Some(live.route);
            controller_ids = Some(live.controller_ids);
            match live.domain {
                domain if domain == IntentDomain::movement() => {
                    if movement_seen
                        || live.controller_ids.movement != active_controller.controller_id
                    {
                        return None;
                    }
                    movement_seen = true;
                    movement_binding = active_controller.binding;
                }
                domain if domain == IntentDomain::gaze() => {
                    if gaze_seen || live.controller_ids.gaze != active_controller.controller_id {
                        return None;
                    }
                    gaze_seen = true;
                    gaze_binding = active_controller.binding;
                }
                domain if domain == IntentDomain::block_interaction() => {}
                _ => return None,
            }
            if active_controller.entity
                != self
                    .observations
                    .controller_registry
                    .as_ref()?
                    .controller_entity(active_controller.controller_id)?
            {
                return None;
            }
        }
        if !movement_seen && !gaze_seen {
            return None;
        }
        let controller_ids = controller_ids?;
        let route = route?;
        if !self.observations.routes.is_live(route) {
            return None;
        }
        Some(SnapshotSubject {
            route,
            tick,
            player_id,
            connection_generation: generation,
            movement_controller: controller_ids.movement,
            gaze_controller: controller_ids.gaze,
            movement_binding,
            gaze_binding,
        })
    }
}

pub(super) fn prune_snapshot_tracking(
    authorization: &SnapshotAuthorization,
    candidates: &mut SnapshotMetadataCandidates,
    state: &mut SnapshotPublicationState,
) {
    candidates.by_connection.retain(|&connection, candidate| {
        authorization.live_admitted_connection(connection)
            == Some((candidate.player_id, candidate.connection_generation))
    });
    state.latest_by_connection.retain(|&connection, attempt| {
        authorization.live_admitted_connection(connection)
            == Some((attempt.player_id, attempt.connection_generation))
    });
}

impl SnapshotMetadataCandidates {
    pub(super) fn store(
        &mut self,
        connection_id: ConnectionId,
        candidate: SnapshotMetadataCandidate,
    ) -> bool {
        if self.by_connection.contains_key(&connection_id)
            || self.by_connection.len() >= SNAPSHOT_CONNECTION_CAPACITY
        {
            return false;
        }
        self.by_connection.insert(connection_id, candidate);
        true
    }
}

fn snapshot_candidate_matches_subject(
    candidate: SnapshotMetadataCandidate,
    subject: SnapshotSubject,
) -> bool {
    candidate.route == subject.route
        && candidate.tick == subject.tick
        && candidate.player_id == subject.player_id
        && candidate.connection_generation == subject.connection_generation
        && candidate.movement_controller == subject.movement_controller
        && candidate.gaze_controller == subject.gaze_controller
        && candidate.movement_binding == subject.movement_binding
        && candidate.gaze_binding == subject.gaze_binding
}

pub(super) fn capture_creature_snapshot_metadata(
    authorization: SnapshotAuthorization,
    tick: Option<Res<AuthoritativeFixedTick>>,
    sequences: Option<Res<crate::player_control::ControllerInputSequences>>,
    mut candidates: ResMut<SnapshotMetadataCandidates>,
    mut state: ResMut<SnapshotPublicationState>,
) {
    prune_snapshot_tracking(&authorization, &mut candidates, &mut state);
    let Some(tick) = tick.as_ref().map(|tick| tick.get()) else {
        candidates.tick = None;
        candidates.by_connection.clear();
        return;
    };
    if candidates.tick != Some(tick) {
        candidates.by_connection.clear();
        candidates.tick = Some(tick);
    }
    let mut connections = authorization
        .active_player_controllers()
        .into_keys()
        .collect::<Vec<_>>();
    connections.sort_unstable();
    for connection_id in connections {
        if candidates.by_connection.contains_key(&connection_id) {
            continue;
        }
        let Some(subject) = authorization.resolve_snapshot_subject(connection_id, tick) else {
            continue;
        };
        let Some(sequences) = sequences.as_ref() else {
            continue;
        };
        let candidate = SnapshotMetadataCandidate {
            route: subject.route,
            tick: subject.tick,
            player_id: subject.player_id,
            connection_generation: subject.connection_generation,
            movement_controller: subject.movement_controller,
            gaze_controller: subject.gaze_controller,
            acknowledged_movement: sequences.movement_sequence(subject.movement_controller),
            acknowledged_gaze: sequences.gaze_sequence(subject.gaze_controller),
            movement_binding: subject.movement_binding,
            gaze_binding: subject.gaze_binding,
        };
        let _ = candidates.store(connection_id, candidate);
    }
}

impl SnapshotPublicationState {
    pub(super) fn mark_attempt(
        &mut self,
        connection_id: ConnectionId,
        candidate: SnapshotMetadataCandidate,
    ) -> bool {
        if let Some(previous) = self.latest_by_connection.get(&connection_id) {
            if previous.connection_generation == candidate.connection_generation
                && candidate.tick <= previous.tick
            {
                return false;
            }
        } else if self.latest_by_connection.len() >= SNAPSHOT_CONNECTION_CAPACITY {
            return false;
        }
        self.latest_by_connection.insert(
            connection_id,
            SnapshotPublicationAttempt {
                tick: candidate.tick,
                player_id: candidate.player_id,
                connection_generation: candidate.connection_generation,
            },
        );
        true
    }
}

pub(super) fn publish_creature_snapshots(
    authorization: SnapshotAuthorization,
    receipts: FactoryMotionReceipts<CreatureAssemblyRegistry>,
    tick: Option<Res<AuthoritativeFixedTick>>,
    mut candidates: ResMut<SnapshotMetadataCandidates>,
    mut state: ResMut<SnapshotPublicationState>,
    sender: Option<Res<ServerMarionetteSnapshotSender>>,
) {
    prune_snapshot_tracking(&authorization, &mut candidates, &mut state);
    let Some(tick) = tick.as_ref().map(|tick| tick.get()) else {
        return;
    };
    let mut ordered = candidates
        .by_connection
        .iter()
        .map(|(&connection, &candidate)| (connection, candidate))
        .collect::<Vec<_>>();
    ordered.sort_unstable_by_key(|(connection, _)| *connection);
    for (connection_id, candidate) in ordered {
        if candidate.tick != tick
            || authorization.live_admitted_connection(connection_id)
                != Some((candidate.player_id, candidate.connection_generation))
        {
            continue;
        }
        let Some(subject) = authorization.resolve_snapshot_subject(connection_id, tick) else {
            continue;
        };
        if !snapshot_candidate_matches_subject(candidate, subject) {
            continue;
        }
        let Some(motion) = receipts.committed_motion(candidate.route) else {
            continue;
        };
        if motion.route() != candidate.route
            || motion.experience().committed_tick().get() != candidate.tick
        {
            continue;
        }
        let Ok(record_entity) = authorization
            .observations
            .instances
            .resolve_body(candidate.route.record())
        else {
            continue;
        };
        let Ok(record) = authorization.observations.records.get(record_entity) else {
            continue;
        };
        let Some(snapshot) = encode_creature_snapshot(
            record,
            motion,
            CreatureSnapshotMetadata {
                route: candidate.route,
                tick: candidate.tick,
                acknowledged_movement: candidate.acknowledged_movement,
                acknowledged_gaze: candidate.acknowledged_gaze,
                movement_binding: candidate.movement_binding,
                gaze_binding: candidate.gaze_binding,
            },
        ) else {
            continue;
        };
        if !state.mark_attempt(connection_id, candidate) {
            continue;
        }
        if let Some(sender) = sender.as_ref() {
            let _ = sender.0.try_send(
                roundo_marionette::ServerMarionetteEvent::CreatureMotionSnapshot {
                    connection_id,
                    snapshot,
                },
            );
        }
    }
}
