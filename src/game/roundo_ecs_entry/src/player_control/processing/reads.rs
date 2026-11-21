//! Read-only local Controller route validation. Returned Handles/Entities are observations, not
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
