use crate::{ConceptBodyHandle, LifeDataTypeKey, LifeHandle};
use bevy::prelude::{Resource, World};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RegistrationDirection {
    Reader,
    Writer,
    ApprovalReader,
    ApprovalWriter,
}

#[derive(Clone, Copy)]
struct RegistrationCleanup {
    life: LifeHandle,
    data_type: LifeDataTypeKey,
    direction: RegistrationDirection,
    /// Monotonic registration/approval identity. Cleanup metadata is derived
    /// only; stale records cannot target a later registration or approval.
    registration_generation: u64,
    cleanup: fn(&mut World, LifeHandle, ConceptBodyHandle, u64),
}

#[derive(Resource)]
pub(crate) struct RegistrationReverseIndex {
    next_registration_generation: u64,
    links: HashMap<ConceptBodyHandle, Vec<RegistrationCleanup>>,
}

impl Default for RegistrationReverseIndex {
    fn default() -> Self {
        Self {
            next_registration_generation: 1,
            links: HashMap::new(),
        }
    }
}

pub(crate) fn record_registration(
    world: &mut World,
    body: ConceptBodyHandle,
    life: LifeHandle,
    data_type: LifeDataTypeKey,
    direction: RegistrationDirection,
    cleanup: fn(&mut World, LifeHandle, ConceptBodyHandle, u64),
) -> u64 {
    world.init_resource::<RegistrationReverseIndex>();
    world
        .resource_mut::<RegistrationReverseIndex>()
        .record(body, life, data_type, direction, cleanup)
}

impl RegistrationReverseIndex {
    pub(crate) fn record(
        &mut self,
        body: ConceptBodyHandle,
        life: LifeHandle,
        data_type: LifeDataTypeKey,
        direction: RegistrationDirection,
        cleanup: fn(&mut World, LifeHandle, ConceptBodyHandle, u64),
    ) -> u64 {
        let registration_generation = self.next_registration_generation;
        self.next_registration_generation = registration_generation
            .checked_add(1)
            .expect("registration generation exhausted");
        self.links
            .entry(body)
            .or_default()
            .push(RegistrationCleanup {
                life,
                data_type,
                direction,
                registration_generation,
                cleanup,
            });
        registration_generation
    }

    pub(crate) fn forget_generation(
        &mut self,
        body: ConceptBodyHandle,
        life: LifeHandle,
        data_type: LifeDataTypeKey,
        direction: RegistrationDirection,
        generation: u64,
    ) {
        let Some(links) = self.links.get_mut(&body) else {
            return;
        };
        links.retain(|link| {
            !(link.life == life
                && link.data_type == data_type
                && link.direction == direction
                && link.registration_generation == generation)
        });
        if links.is_empty() {
            self.links.remove(&body);
        }
    }

    pub(crate) fn forget(
        &mut self,
        body: ConceptBodyHandle,
        life: LifeHandle,
        data_type: LifeDataTypeKey,
        direction: RegistrationDirection,
    ) {
        let Some(links) = self.links.get_mut(&body) else {
            return;
        };
        links.retain(|link| {
            !(link.life == life && link.data_type == data_type && link.direction == direction)
        });
        if links.is_empty() {
            self.links.remove(&body);
        }
    }
}

pub(crate) fn cleanup_removed_body(world: &mut World, body: ConceptBodyHandle) {
    let cleanups = world
        .get_resource_mut::<RegistrationReverseIndex>()
        .and_then(|mut index| index.links.remove(&body))
        .unwrap_or_default();
    for cleanup in cleanups {
        (cleanup.cleanup)(world, cleanup.life, body, cleanup.registration_generation);
    }
}

pub(crate) fn forget_registration(
    world: &mut World,
    body: ConceptBodyHandle,
    life: LifeHandle,
    data_type: LifeDataTypeKey,
    direction: RegistrationDirection,
) {
    let Some(mut index) = world.get_resource_mut::<RegistrationReverseIndex>() else {
        return;
    };
    index.forget(body, life, data_type, direction);
}

pub(crate) fn forget_registration_generation(
    world: &mut World,
    body: ConceptBodyHandle,
    life: LifeHandle,
    data_type: LifeDataTypeKey,
    direction: RegistrationDirection,
    generation: u64,
) {
    if let Some(mut index) = world.get_resource_mut::<RegistrationReverseIndex>() {
        index.forget_generation(body, life, data_type, direction, generation);
    }
}

pub(crate) fn forget_life(world: &mut World, life: LifeHandle) {
    let Some(mut index) = world.get_resource_mut::<RegistrationReverseIndex>() else {
        return;
    };
    index.links.retain(|_, links| {
        links.retain(|link| link.life != life);
        !links.is_empty()
    });
}
