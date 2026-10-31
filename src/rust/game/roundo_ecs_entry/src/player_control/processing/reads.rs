//! Read-only Player adapters. Returned Handles/Entities are observations, not
//! retained authorization: publication and settlement must revalidate at use.

use super::*;
use crate::authoritative_creature::CreatureAssemblyRegistry;
use bevy::{
    ecs::system::SystemParam,
    prelude::{Entity, In, Query, Res, With},
};
use roundo_controller::Controller;
use roundo_creature::{ExecutionBodyId, RecordBodyId};
use roundo_life::LifeInstances;

/// Current composition routing, without producer/binding or data permissions.
#[derive(SystemParam)]
pub(super) struct ControllerCommandRoutes<'w, 's> {
    pub(super) instances: LifeInstances<'w, 's>,
    registry: Option<Res<'w, ControllerRegistry>>,
    assemblies: Option<Res<'w, CreatureAssemblyRegistry>>,
    controllers: Query<'w, 's, &'static Controller>,
    routes: Query<'w, 's, &'static FactoryMotionRoute>,
    executions: Query<'w, 's, (), With<ExecutionBodyId>>,
    records: Query<'w, 's, (), With<RecordBodyId>>,
}

impl ControllerCommandRoutes<'_, '_> {
    fn controller(&self, id: ControllerId) -> Option<(Entity, &Controller)> {
        let entity = self.registry.as_ref()?.controller_entity(id)?;
        let controller = self.controllers.get(entity).ok()?;
        (controller.id() == id).then_some((entity, controller))
    }

    pub(super) fn resolve(&self, id: ControllerId) -> Option<ControllerCommandRoute> {
        let (_, controller) = self.controller(id)?;
        let intent = controller.intent_body_handle()?;
        let intent_entity = self.instances.resolve_body(intent).ok()?;
        let assemblies = self.assemblies.as_ref()?;
        let life = assemblies.life_for_body(intent)?;
        // A separately registered Controller cannot turn the local recipe's
        // membership into player/domain authorization.
        if !assemblies.accepts_controller_input(life) {
            return None;
        }
        let members = assemblies.get(life)?;
        if members.intent != intent
            || assemblies.life_for_body(members.execution) != Some(life)
            || assemblies.life_for_body(members.record) != Some(life)
        {
            return None;
        }
        self.instances.resolve_life(life).ok()?;
        let execution = self.instances.resolve_body(members.execution).ok()?;
        let record = self.instances.resolve_body(members.record).ok()?;
        self.executions.get(execution).ok()?;
        self.records.get(record).ok()?;
        if self.routes.get(intent_entity).ok()?
            != &FactoryMotionRoute::new(life, intent, members.execution, members.record)
        {
            return None;
        }
        Some(ControllerCommandRoute {
            life,
            intent,
            execution: members.execution,
            domain: controller.domain(),
        })
    }

    fn intent_entity(&self, id: ControllerId) -> Option<Entity> {
        let route = self.resolve(id)?;
        self.instances.resolve_body(route.intent).ok()
    }

    pub(super) fn gate_owned_by_player(
        &self,
        admission: &Admission,
        player: PlayerId,
    ) -> Vec<ControllerId> {
        self.controllers
            .iter()
            .filter(|controller| {
                admission
                    .active_player(controller.id())
                    .is_some_and(|(_, owner)| owner == player)
            })
            .map(Controller::id)
            .collect()
    }
}

pub(super) fn resolve_route(
    In(id): In<ControllerId>,
    routes: ControllerCommandRoutes,
) -> Option<ControllerCommandRoute> {
    routes.resolve(id)
}

pub(super) fn resolve_intent_entity(
    In(id): In<ControllerId>,
    routes: ControllerCommandRoutes,
) -> Option<Entity> {
    routes.intent_entity(id)
}

/// This is the legacy prefix eligibility check, not an AuthorityGate Submit or
/// settlement certificate. Multiple connections and a missing live route fail
/// closed; it never mutates ownership, gate state, or sequence metadata.
pub(super) fn resolve_prefix_player(
    In((connection, id)): In<(ConnectionId, ControllerId)>,
    players: Option<Res<PlayerRegistry>>,
    routes: ControllerCommandRoutes,
) -> Option<PlayerId> {
    let players = players?;
    let player = players.player_for_connection(connection)?;
    if players.connections_for_player(player).count() != 1
        || routes.controller(id)?.1.current_owner() != Some(ControllerOwner::Player(player))
        || routes.intent_entity(id).is_none()
    {
        return None;
    }
    Some(player)
}

pub(super) enum AccessProjection {
    UnknownConnection,
    Unavailable,
    Snapshot(PlayerControllerAccessSnapshot),
}

/// Projects the same Access list the FIFO owner used to construct synchronously.
/// A grant is only listed with a current complete Factory route; occupancy is
/// observed from Admission, not inferred from legacy ownership. No acquisition
/// or binding is performed here, and callers must not reuse this as Submit proof.
pub(super) fn project_access(
    In(connection): In<ConnectionId>,
    registry: Res<PlayerRegistry>,
    players: Query<&Player>,
    routes: ControllerCommandRoutes,
    admission: Res<Admission>,
    block_item: Option<Res<crate::block_item::BlockItemReference>>,
) -> AccessProjection {
    let Some(player_id) = registry.player_for_connection(connection) else {
        return AccessProjection::UnknownConnection;
    };
    let Some(player) = registry
        .player_entity(player_id)
        .and_then(|entity| players.get(entity).ok())
    else {
        return AccessProjection::Unavailable;
    };
    let mut controllers = player
        .controller_accesses()
        .filter_map(|(controller_id, access_level)| {
            let route = routes.resolve(controller_id)?;
            let entity = routes.controller(controller_id)?.0;
            let current = admission.active_player(controller_id);
            let occupied_by_ai = admission
                .active_ai_for_entity(controller_id, entity)
                .is_some();
            let intent_domain = if route.domain == roundo_controller::IntentDomain::movement() {
                ControllerIntentDomain::Movement
            } else if route.domain == roundo_controller::IntentDomain::gaze() {
                ControllerIntentDomain::Gaze
            } else if route.domain == roundo_controller::IntentDomain::block_interaction() {
                ControllerIntentDomain::BlockInteraction
            } else {
                return None;
            };
            Some(PlayerControllerAccess {
                controller_id,
                intent_domain,
                access_level,
                control_state: match current {
                    None if occupied_by_ai => ControllerControlState::ControlledByOther,
                    None => ControllerControlState::Uncontrolled,
                    Some((_, id)) if id == player_id => ControllerControlState::ControlledBySelf,
                    Some(_) => ControllerControlState::ControlledByOther,
                },
            })
        })
        .collect::<Vec<_>>();
    controllers.sort_unstable_by_key(|entry| entry.controller_id);
    AccessProjection::Snapshot(PlayerControllerAccessSnapshot {
        player_id,
        block_item: controllers
            .iter()
            .any(|entry| {
                entry.intent_domain == ControllerIntentDomain::BlockInteraction
                    && entry.access_level == roundo_contracts::ControllerAccessLevel::ReadWrite
            })
            .then(|| crate::block_item::reference(block_item.as_deref()))
            .flatten(),
        controllers,
    })
}
