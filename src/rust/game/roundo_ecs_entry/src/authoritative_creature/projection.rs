//! Local Player read authorization and Creature Record projection.

use super::*;
use bevy::ecs::system::SystemParam;
use roundo_contracts::ControllerId;
use roundo_creature::FactoryMotionRoutes;
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
    records: Query<'w, 's, &'static CreatureMotionRecord>,
}

impl CreatureObservations<'_, '_> {
    fn player(&self, id: PlayerId) -> Option<&Player> {
        let registry = self.player_registry.as_ref()?;
        let player = self.players.get(registry.player_entity(id)?).ok()?;
        (player.id() == id).then_some(player)
    }

    fn live_creature_controller(
        &self,
        controller_id: ControllerId,
    ) -> Option<(LifeHandle, FactoryMotionRoute)> {
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
            domain
                if domain == IntentDomain::gaze() && controller_ids.gaze == controller_id => {}
            _ => return None,
        }
        Some((life, actual_route))
    }

    /// Copies the authorized Life's Record, including a spawn baseline or frozen
    /// value. No current commit receipt is required.
    pub fn project(
        &self,
        player_id: PlayerId,
        life: LifeHandle,
    ) -> Option<CreatureObservation> {
        let player = self.player(player_id)?;
        let route = player.controller_accesses().find_map(|(id, level)| {
            if !matches!(
                level,
                ControllerAccessLevel::Read | ControllerAccessLevel::ReadWrite
            ) {
                return None;
            }
            let (owner, route) = self.live_creature_controller(id)?;
            (owner == life).then_some(route)
        })?;
        let record_entity = self.instances.resolve_body(route.record()).ok()?;
        Some(CreatureObservation::from_record(
            self.records.get(record_entity).ok()?,
        ))
    }
}

/// Projects an owned Record only after current local Player authorization.
/// Use the read-only parameter in an ordinary system; a copied Handle is not a grant.
pub fn project_authorized_creature_observation(
    observations: &CreatureObservations,
    player_id: PlayerId,
    life: LifeHandle,
) -> Option<CreatureObservation> {
    observations.project(player_id, life)
}
