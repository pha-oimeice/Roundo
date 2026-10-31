//! Narrow ECS access to live Factory instances; no lifecycle or administration authority.

use crate::{
    ConceptBodyHandle, FactoryError, LifeHandle,
    factory::{ConceptBodyIndex, LifeIndex},
    handle::{ConceptBodyIdentity, LifeIdentity},
};
use bevy::{
    ecs::{system::SystemParam, world::WorldId},
    prelude::{Entity, FromWorld, Local, Query, Res, World},
};

pub(crate) struct InstanceWorld(WorldId);
impl FromWorld for InstanceWorld {
    fn from_world(world: &mut World) -> Self {
        Self(world.id())
    }
}

/// Temporary provenance selected by a trusted ECS adapter from a live Body entity.
///
/// It is not constructible from a copied Handle and cannot outlive the binding
/// callback. Typed access rechecks this World's current Factory identity and
/// registration on every operation. Trusted systems can select Bodies; this is
/// not a sandbox against arbitrary systems installed by the application host.
///
pub struct BodyInstance<'body> {
    identity: &'body ConceptBodyIdentity,
    entity: Entity,
    world: WorldId,
}

impl BodyInstance<'_> {
    pub fn handle(&self) -> ConceptBodyHandle {
        self.identity.handle()
    }
}

/// Read-only, generation-checked instance lookup for ECS systems.
///
/// Lookup indexes remain Factory-owned. Entity resolution is instantaneous and
/// grants neither data access nor authority to create/remove an instance.
#[derive(SystemParam)]
pub struct LifeInstances<'w, 's> {
    world: Local<'s, InstanceWorld>,
    lives: Option<Res<'w, LifeIndex>>,
    bodies: Option<Res<'w, ConceptBodyIndex>>,
    life_identities: Query<'w, 's, &'static LifeIdentity>,
    body_identities: Query<'w, 's, &'static ConceptBodyIdentity>,
}

impl LifeInstances<'_, '_> {
    /// Enumerates live Factory identities without granting data or management access.
    /// Ordering is unspecified; callers needing stable pages must sort owned handles.
    pub fn lives(&self) -> impl Iterator<Item = LifeHandle> + '_ {
        self.life_identities
            .iter()
            .map(|identity| identity.0)
            .filter(|handle| self.resolve_life(*handle).is_ok())
    }

    pub fn bodies(&self) -> impl Iterator<Item = ConceptBodyHandle> + '_ {
        self.body_identities
            .iter()
            .map(ConceptBodyIdentity::handle)
            .filter(|handle| self.resolve_body(*handle).is_ok())
    }

    pub fn resolve_life(&self, handle: LifeHandle) -> Result<Entity, FactoryError> {
        let entity = self
            .lives
            .as_ref()
            .and_then(|index| index.resolve(handle))
            .ok_or(FactoryError::InstanceNotFound)?;
        (self
            .life_identities
            .get(entity)
            .ok()
            .map(|identity| identity.0)
            == Some(handle))
        .then_some(entity)
        .ok_or(FactoryError::InstanceNotFound)
    }

    pub fn resolve_body(&self, handle: ConceptBodyHandle) -> Result<Entity, FactoryError> {
        let entity = self
            .bodies
            .as_ref()
            .and_then(|index| index.resolve(handle))
            .ok_or(FactoryError::InstanceNotFound)?;
        (self
            .body_identities
            .get(entity)
            .ok()
            .map(ConceptBodyIdentity::handle)
            == Some(handle))
        .then_some(entity)
        .ok_or(FactoryError::InstanceNotFound)
    }

    /// Binds a live queried Body entity for one synchronous operation.
    /// The adapter must select the entity under its own domain/producer policy;
    /// this method establishes Factory provenance, not Player authorization.
    pub fn with_body<R>(
        &self,
        entity: Entity,
        operation: impl for<'body> FnOnce(BodyInstance<'body>) -> R,
    ) -> Result<R, FactoryError> {
        let identity = self
            .body_identities
            .get(entity)
            .map_err(|_| FactoryError::InstanceNotFound)?;
        if self.resolve_body(identity.handle())? != entity {
            return Err(FactoryError::InstanceNotFound);
        }
        Ok(operation(BodyInstance {
            identity,
            entity,
            world: self.world.0,
        }))
    }

    pub(crate) fn applicant(
        &self,
        body: &BodyInstance<'_>,
    ) -> Result<ConceptBodyHandle, FactoryError> {
        if body.world != self.world.0 || self.resolve_body(body.handle())? != body.entity {
            return Err(FactoryError::InstanceNotFound);
        }
        Ok(body.handle())
    }
}
