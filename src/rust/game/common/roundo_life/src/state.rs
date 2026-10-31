mod management;
use crate::resource::{ManagedData, ModifyGrant, ReadGrant, Registration};
pub use management::LifeStateManagement;
use roundo_dsa::data_structure::resource_access::AccessError;

use crate::{
    BodyCaller, BodyInstance, ConceptBodyFactory, ConceptBodyHandle, FrozenStateHistoryConfig,
    LifeAdministration, LifeFactory, LifeHandle, LifeInstances, StateData, TypeRegistrationError,
    admin::{RegistrationDirection, forget_registration, record_registration},
    data::{register_type, type_is_registered},
};
use bevy::{
    ecs::system::{SystemParam, SystemState},
    prelude::{App, Component, Has, Query, World},
};
use std::{
    collections::{BTreeMap, VecDeque},
    marker::PhantomData,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateError {
    ResourceAccess(AccessError),
    LifeNotFound,
    BodyNotFound,
    MissingWriterDeclaration,
    MissingReaderDeclaration,
    PolicyAlreadyExists,
    PolicyNotFound,
    OriginNotApproved,
    ReaderNotApproved,
    SlotAlreadyExists,
    SlotNotFound,
    SlotChanged,
    UninitializedForbidden,
    ReaderAlreadyRegistered,
    ReaderNotRegistered,
    WriterNotRegistered,
    OriginMismatch,
    HistoryOffsetOutOfRange,
    TypeNotRegistered,
}

pub fn register_state_type<S: StateData>(app: &mut App) -> Result<(), TypeRegistrationError> {
    register_type::<S>(app.world_mut())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StatePolicy {
    allow_uninitialized: bool,
}

impl StatePolicy {
    pub const fn initialized_only() -> Self {
        Self {
            allow_uninitialized: false,
        }
    }

    pub const fn allow_uninitialized() -> Self {
        Self {
            allow_uninitialized: true,
        }
    }
}

#[derive(Component)]
struct StateWriterDeclaration<S: StateData>(PhantomData<fn() -> S>);

#[derive(Component)]
struct StateReaderDeclaration<S: StateData>(PhantomData<fn() -> S>);

#[derive(Component)]
struct StateAdmission<S: StateData> {
    policy: StatePolicy,
    approved_origins: BTreeMap<ConceptBodyHandle, u64>,
    approved_readers: BTreeMap<ConceptBodyHandle, u64>,
    marker: PhantomData<fn() -> S>,
}

#[derive(Component)]
pub(crate) struct StateSlot<S: StateData> {
    generation: u64,
    origin: ConceptBodyHandle,
    writer: Option<Registration<ModifyGrant<VecDeque<S>>>>,
    readers: std::collections::BTreeMap<ConceptBodyHandle, Registration<ReadGrant<VecDeque<S>>>>,
    values: ManagedData<VecDeque<S>>,
    max_history: usize,
}

impl<S: StateData> StateSlot<S> {
    fn set(&mut self, applicant: ConceptBodyHandle, value: S) -> Result<(), StateError> {
        if self.origin != applicant {
            return Err(StateError::OriginMismatch);
        }
        let writer = self
            .writer
            .as_ref()
            .ok_or(StateError::WriterNotRegistered)?;
        self.values
            .modify(&writer.access, |values| {
                values.push_front(value);
                values.truncate(self.max_history.saturating_add(1));
            })
            .map_err(StateError::ResourceAccess)
    }

    fn inspect<R>(
        &self,
        applicant: ConceptBodyHandle,
        inspect: impl for<'a> FnOnce(StateTickView<'a, S>) -> R,
    ) -> Result<R, StateError> {
        let reader = self
            .readers
            .get(&applicant)
            .ok_or(StateError::ReaderNotRegistered)?;
        self.values
            .read(&reader.access, |values| {
                inspect(StateTickView {
                    values,
                    max_history: self.max_history,
                })
            })
            .map_err(StateError::ResourceAccess)
    }
}

/// Owned structural metadata; generation stays stable across writer freeze and
/// rebind. It is not a Slot reference, payload snapshot or removal permission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StateSlotEvidence {
    generation: u64,
    origin: ConceptBodyHandle,
    writer_registered: bool,
    readers: usize,
}
impl StateSlotEvidence {
    pub const fn generation(self) -> u64 {
        self.generation
    }
    pub const fn origin(self) -> ConceptBodyHandle {
        self.origin
    }
    pub const fn has_writer(self) -> bool {
        self.writer_registered
    }
    pub const fn reader_count(self) -> usize {
        self.readers
    }
}

/// Read-only control-plane evidence of current Life-held State registration.
/// Checks live Factory generations without reading values, changing history or
/// granting approval/registration. No Slot or reverse metadata escapes.
#[derive(SystemParam)]
pub struct LifeStateRegistrations<'w, 's, S: StateData> {
    instances: LifeInstances<'w, 's>,
    slots: Query<'w, 's, &'static StateSlot<S>>,
    admissions: Query<'w, 's, &'static StateAdmission<S>>,
    declarations: Query<
        'w,
        's,
        (
            Has<StateWriterDeclaration<S>>,
            Has<StateReaderDeclaration<S>>,
        ),
    >,
}

impl<S: StateData> LifeStateRegistrations<'_, '_, S> {
    /// Observes the caller-owned policy without approval or Slot access.
    pub fn state_policy(&self, life: LifeHandle) -> Result<StatePolicy, StateError> {
        let entity = self
            .instances
            .resolve_life(life)
            .map_err(|_| StateError::LifeNotFound)?;
        Ok(self
            .admissions
            .get(entity)
            .map_err(|_| StateError::PolicyNotFound)?
            .policy)
    }
    /// Distinguishes an absent Slot from a present but frozen/unregistered one.
    pub fn has_slot(&self, life: LifeHandle) -> Result<bool, StateError> {
        let entity = self
            .instances
            .resolve_life(life)
            .map_err(|_| StateError::LifeNotFound)?;
        Ok(self.slots.get(entity).is_ok())
    }
    pub fn slot_evidence(&self, life: LifeHandle) -> Result<Option<StateSlotEvidence>, StateError> {
        let entity = self
            .instances
            .resolve_life(life)
            .map_err(|_| StateError::LifeNotFound)?;
        Ok(self.slots.get(entity).ok().map(|slot| StateSlotEvidence {
            generation: slot.generation,
            origin: slot.origin,
            writer_registered: slot.writer.is_some(),
            readers: slot.readers.len(),
        }))
    }
    pub fn validate_writer_declaration(&self, body: ConceptBodyHandle) -> Result<(), StateError> {
        let entity = self
            .instances
            .resolve_body(body)
            .map_err(|_| StateError::BodyNotFound)?;
        self.declarations
            .get(entity)
            .is_ok_and(|value| value.0)
            .then_some(())
            .ok_or(StateError::MissingWriterDeclaration)
    }
    pub fn validate_reader_declaration(&self, body: ConceptBodyHandle) -> Result<(), StateError> {
        let entity = self
            .instances
            .resolve_body(body)
            .map_err(|_| StateError::BodyNotFound)?;
        self.declarations
            .get(entity)
            .is_ok_and(|value| value.1)
            .then_some(())
            .ok_or(StateError::MissingReaderDeclaration)
    }
    pub fn validate_writer(
        &self,
        life: LifeHandle,
        body: ConceptBodyHandle,
    ) -> Result<(), StateError> {
        let entity = self
            .instances
            .resolve_life(life)
            .map_err(|_| StateError::LifeNotFound)?;
        self.instances
            .resolve_body(body)
            .map_err(|_| StateError::BodyNotFound)?;
        let slot = self
            .slots
            .get(entity)
            .map_err(|_| StateError::SlotNotFound)?;
        if slot.origin != body {
            return Err(StateError::OriginMismatch);
        }
        slot.writer
            .as_ref()
            .ok_or(StateError::WriterNotRegistered)?
            .access
            .check()
            .map_err(StateError::ResourceAccess)
    }

    pub fn validate_reader(
        &self,
        life: LifeHandle,
        body: ConceptBodyHandle,
    ) -> Result<(), StateError> {
        let entity = self
            .instances
            .resolve_life(life)
            .map_err(|_| StateError::LifeNotFound)?;
        self.instances
            .resolve_body(body)
            .map_err(|_| StateError::BodyNotFound)?;
        let slot = self
            .slots
            .get(entity)
            .map_err(|_| StateError::SlotNotFound)?;
        slot.readers
            .get(&body)
            .ok_or(StateError::ReaderNotRegistered)?
            .access
            .check()
            .map_err(StateError::ResourceAccess)
    }
}

/// Read-only typed State access. Views are synchronous; retaining data requires
/// an explicit owned copy. Neither slots nor their registration metadata escape.
#[derive(SystemParam)]
pub struct LifeStateReader<'w, 's, S: StateData> {
    instances: LifeInstances<'w, 's>,
    slots: Query<'w, 's, &'static StateSlot<S>>,
}

impl<S: StateData> LifeStateReader<'_, '_, S> {
    pub fn with_state_view<R>(
        &self,
        body: &BodyInstance<'_>,
        life: LifeHandle,
        inspect: impl for<'a> FnOnce(StateTickView<'a, S>) -> R,
    ) -> Result<R, StateError> {
        let applicant = self
            .instances
            .applicant(body)
            .map_err(|_| StateError::BodyNotFound)?;
        let entity = self
            .instances
            .resolve_life(life)
            .map_err(|_| StateError::LifeNotFound)?;
        self.slots
            .get(entity)
            .map_err(|_| StateError::SlotNotFound)?
            .inspect(applicant, inspect)
    }
}

/// Typed State replacement for the live, registered immutable origin Body.
/// This parameter grants no Slot installation, registration or lifecycle authority.
#[derive(SystemParam)]
pub struct LifeStateWriter<'w, 's, S: StateData> {
    instances: LifeInstances<'w, 's>,
    slots: Query<'w, 's, &'static mut StateSlot<S>>,
}

impl<S: StateData> LifeStateWriter<'_, '_, S> {
    pub fn set_state(
        &mut self,
        body: &BodyInstance<'_>,
        life: LifeHandle,
        value: S,
    ) -> Result<(), StateError> {
        let applicant = self
            .instances
            .applicant(body)
            .map_err(|_| StateError::BodyNotFound)?;
        let entity = self
            .instances
            .resolve_life(life)
            .map_err(|_| StateError::LifeNotFound)?;
        self.slots
            .get_mut(entity)
            .map_err(|_| StateError::SlotNotFound)?
            .set(applicant, value)
    }
}

pub struct StateTickView<'a, S: StateData> {
    values: &'a VecDeque<S>,
    max_history: usize,
}

impl<'a, S: StateData> StateTickView<'a, S> {
    pub fn current(&self) -> Option<&'a S> {
        self.values.front()
    }

    pub fn get(&self, offset: usize) -> Result<Option<&'a S>, StateError> {
        if offset > self.max_history {
            return Err(StateError::HistoryOffsetOutOfRange);
        }
        Ok(self.values.get(offset))
    }

    pub fn available_values(&self) -> impl Iterator<Item = (usize, &'a S)> + '_ {
        self.values.iter().enumerate()
    }
}

pub(crate) fn declare_state_writer_on_entity<S: StateData>(
    world: &mut World,
    entity: bevy::prelude::Entity,
) -> Result<(), StateError> {
    if !type_is_registered::<S>(world) {
        return Err(StateError::TypeNotRegistered);
    }
    world
        .get_entity_mut(entity)
        .map_err(|_| StateError::BodyNotFound)?
        .insert(StateWriterDeclaration::<S>(PhantomData));
    Ok(())
}
pub fn declare_state_writer<S: StateData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    body: ConceptBodyHandle,
) -> Result<(), StateError> {
    if !administration.matches(world) {
        return Err(StateError::LifeNotFound);
    }
    let entity = ConceptBodyFactory::entity(world, body).ok_or(StateError::BodyNotFound)?;
    declare_state_writer_on_entity::<S>(world, entity)
}

pub(crate) fn declare_state_reader_on_entity<S: StateData>(
    world: &mut World,
    entity: bevy::prelude::Entity,
) -> Result<(), StateError> {
    if !type_is_registered::<S>(world) {
        return Err(StateError::TypeNotRegistered);
    }
    world
        .get_entity_mut(entity)
        .map_err(|_| StateError::BodyNotFound)?
        .insert(StateReaderDeclaration::<S>(PhantomData));
    Ok(())
}
pub fn declare_state_reader<S: StateData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    body: ConceptBodyHandle,
) -> Result<(), StateError> {
    if !administration.matches(world) {
        return Err(StateError::LifeNotFound);
    }
    let entity = ConceptBodyFactory::entity(world, body).ok_or(StateError::BodyNotFound)?;
    declare_state_reader_on_entity::<S>(world, entity)
}

pub(crate) fn install_state_policy_on_entity<S: StateData>(
    world: &mut World,
    entity: bevy::prelude::Entity,
    policy: StatePolicy,
) -> Result<(), StateError> {
    if !type_is_registered::<S>(world) {
        return Err(StateError::TypeNotRegistered);
    }
    if world.get::<StateAdmission<S>>(entity).is_some() {
        return Err(StateError::PolicyAlreadyExists);
    }
    world
        .get_entity_mut(entity)
        .map_err(|_| StateError::LifeNotFound)?
        .insert(StateAdmission::<S> {
            policy,
            approved_origins: BTreeMap::new(),
            approved_readers: BTreeMap::new(),
            marker: PhantomData,
        });
    Ok(())
}

pub fn install_state_policy<S: StateData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    life: LifeHandle,
    policy: StatePolicy,
) -> Result<(), StateError> {
    if !administration.matches(world) {
        return Err(StateError::LifeNotFound);
    }
    let entity = LifeFactory::entity(world, life).ok_or(StateError::LifeNotFound)?;
    install_state_policy_on_entity::<S>(world, entity, policy)
}

fn with_state_management<S: StateData, R>(
    world: &mut World,
    operation: impl FnOnce(&mut LifeStateManagement<S>) -> R,
) -> R {
    world.init_resource::<crate::admin::RegistrationReverseIndex>();
    let mut state = SystemState::<LifeStateManagement<S>>::new(world);
    operation(
        &mut state
            .get_mut(world)
            .expect("State management resources initialized"),
    )
}

pub fn approve_state_origin<S: StateData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    life: LifeHandle,
    body: ConceptBodyHandle,
) -> Result<(), StateError> {
    if !administration.matches(world) {
        return Err(StateError::LifeNotFound);
    }
    with_state_management::<S, _>(world, |management| management.approve_origin(life, body))
}

pub fn approve_state_reader<S: StateData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    life: LifeHandle,
    body: ConceptBodyHandle,
) -> Result<(), StateError> {
    if !administration.matches(world) {
        return Err(StateError::LifeNotFound);
    }
    with_state_management::<S, _>(world, |management| management.approve_reader(life, body))
}

pub fn create_state_slot<S: StateData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    life: LifeHandle,
    origin: ConceptBodyHandle,
    initial: Option<S>,
) -> Result<(), StateError> {
    if !administration.matches(world) {
        return Err(StateError::LifeNotFound);
    }
    if !type_is_registered::<S>(world) {
        return Err(StateError::TypeNotRegistered);
    }
    let life_entity = LifeFactory::entity(world, life).ok_or(StateError::LifeNotFound)?;
    let origin_entity =
        ConceptBodyFactory::entity(world, origin).ok_or(StateError::BodyNotFound)?;
    if world
        .get::<StateWriterDeclaration<S>>(origin_entity)
        .is_none()
    {
        return Err(StateError::MissingWriterDeclaration);
    }
    if world.get::<StateSlot<S>>(life_entity).is_some() {
        return Err(StateError::SlotAlreadyExists);
    }
    let admission = world
        .get::<StateAdmission<S>>(life_entity)
        .ok_or(StateError::PolicyNotFound)?;
    if !admission.approved_origins.contains_key(&origin) {
        return Err(StateError::OriginNotApproved);
    }
    if initial.is_none() && !admission.policy.allow_uninitialized {
        return Err(StateError::UninitializedForbidden);
    }
    let max_history = world
        .get_resource::<FrozenStateHistoryConfig>()
        .expect("LifeRuntimePlugin must freeze State history configuration")
        .max_history();
    let mut values = ManagedData::create(world, initial.into_iter().collect::<VecDeque<S>>())
        .map_err(StateError::ResourceAccess)?;
    let access = values.grant_modify().map_err(StateError::ResourceAccess)?;
    // Allocate registration identity before publishing a Slot. Exhaustion must
    // not leave a partially installed, unregistered Slot behind.
    let registration_generation = record_registration(
        world,
        origin,
        life,
        S::KEY,
        RegistrationDirection::Writer,
        cleanup_state_writer::<S>,
    );
    let inserted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world
            .get_entity_mut(life_entity)
            .map_err(|_| StateError::LifeNotFound)?
            .insert(StateSlot::<S> {
                generation: registration_generation,
                origin,
                writer: Some(Registration {
                    generation: registration_generation,
                    access,
                }),
                readers: std::collections::BTreeMap::new(),
                values,
                max_history,
            });
        if world
            .get::<StateSlot<S>>(life_entity)
            .is_none_or(|slot| slot.generation != registration_generation)
        {
            return Err(StateError::SlotChanged);
        }
        Ok(())
    }));
    if !matches!(&inserted, Ok(Ok(()))) {
        // Hooks can panic after materialization. Only this stable generation
        // belongs to the operation; a hook-created replacement must survive.
        let _ = remove_state_slot_if_generation::<S>(
            world,
            administration,
            life,
            registration_generation,
        );
        crate::admin::forget_registration_generation(
            world,
            origin,
            life,
            S::KEY,
            RegistrationDirection::Writer,
            registration_generation,
        );
    }
    match inserted {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

pub fn register_state_reader<S: StateData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    life: LifeHandle,
    body: ConceptBodyHandle,
) -> Result<(), StateError> {
    if !administration.matches(world) {
        return Err(StateError::LifeNotFound);
    }
    with_state_management::<S, _>(world, |management| management.register_reader(life, body))
}

pub fn deregister_state_reader<S: StateData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    life: LifeHandle,
    body: ConceptBodyHandle,
) -> Result<(), StateError> {
    if !administration.matches(world) {
        return Err(StateError::LifeNotFound);
    }
    with_state_management::<S, _>(world, |management| management.deregister_reader(life, body))
}

pub fn deregister_state_writer<S: StateData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    life: LifeHandle,
    origin: ConceptBodyHandle,
) -> Result<(), StateError> {
    if !administration.matches(world) {
        return Err(StateError::LifeNotFound);
    }
    with_state_management::<S, _>(world, |management| {
        management.deregister_writer(life, origin)
    })
}

pub fn rebind_state_writer<S: StateData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    life: LifeHandle,
    applicant: ConceptBodyHandle,
) -> Result<(), StateError> {
    if !administration.matches(world) {
        return Err(StateError::LifeNotFound);
    }
    with_state_management::<S, _>(world, |management| {
        management.rebind_writer(life, applicant)
    })
}

fn set_state<S: StateData>(
    world: &mut World,
    life: LifeHandle,
    applicant: ConceptBodyHandle,
    value: S,
) -> Result<(), StateError> {
    if !ConceptBodyFactory::is_live(world, applicant) {
        return Err(StateError::BodyNotFound);
    }
    let entity = LifeFactory::entity(world, life).ok_or(StateError::LifeNotFound)?;
    let mut slot = world
        .get_mut::<StateSlot<S>>(entity)
        .ok_or(StateError::SlotNotFound)?;
    slot.set(applicant, value)
}

fn with_state_view<S: StateData, R>(
    world: &mut World,
    life: LifeHandle,
    applicant: ConceptBodyHandle,
    inspect: impl for<'a> FnOnce(StateTickView<'a, S>) -> R,
) -> Result<R, StateError> {
    if !ConceptBodyFactory::is_live(world, applicant) {
        return Err(StateError::BodyNotFound);
    }
    let entity = LifeFactory::entity(world, life).ok_or(StateError::LifeNotFound)?;
    let slot = world
        .get::<StateSlot<S>>(entity)
        .ok_or(StateError::SlotNotFound)?;
    slot.inspect(applicant, inspect)
}

pub fn remove_state_slot<S: StateData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    life: LifeHandle,
) -> Result<(), StateError> {
    remove_state_slot_matching::<S>(world, administration, life, None)
}

pub(crate) fn remove_state_slot_if_generation<S: StateData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    life: LifeHandle,
    expected: u64,
) -> Result<(), StateError> {
    remove_state_slot_matching::<S>(world, administration, life, Some(expected))
}

fn remove_state_slot_matching<S: StateData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    life: LifeHandle,
    expected: Option<u64>,
) -> Result<(), StateError> {
    if !administration.matches(world) {
        return Err(StateError::LifeNotFound);
    }
    let entity = LifeFactory::entity(world, life).ok_or(StateError::LifeNotFound)?;
    if let Some(expected) = expected {
        let slot = world
            .get::<StateSlot<S>>(entity)
            .ok_or(StateError::SlotNotFound)?;
        if slot.generation != expected {
            return Err(StateError::SlotChanged);
        }
    }
    let slot = world
        .get_entity_mut(entity)
        .map_err(|_| StateError::LifeNotFound)?
        .take::<StateSlot<S>>()
        .ok_or(StateError::SlotNotFound)?;
    forget_registration(
        world,
        slot.origin,
        life,
        S::KEY,
        RegistrationDirection::Writer,
    );
    for (reader, _) in slot.readers {
        forget_registration(world, reader, life, S::KEY, RegistrationDirection::Reader);
    }
    Ok(())
}

impl BodyCaller<'_> {
    pub fn set_state<S: StateData>(
        &mut self,
        life: LifeHandle,
        value: S,
    ) -> Result<(), StateError> {
        let applicant = self.handle();
        set_state(self.world_mut(), life, applicant, value)
    }
    pub fn with_state_view<S: StateData, R>(
        &mut self,
        life: LifeHandle,
        inspect: impl for<'a> FnOnce(StateTickView<'a, S>) -> R,
    ) -> Result<R, StateError> {
        let applicant = self.handle();
        with_state_view(self.world_mut(), life, applicant, inspect)
    }
}

fn cleanup_state_origin_approval<S: StateData>(
    world: &mut World,
    life: LifeHandle,
    body: ConceptBodyHandle,
    expected_generation: u64,
) {
    let Some(entity) = LifeFactory::entity(world, life) else {
        return;
    };
    let Some(mut admission) = world.get_mut::<StateAdmission<S>>(entity) else {
        return;
    };
    if admission.approved_origins.get(&body) == Some(&expected_generation) {
        admission.approved_origins.remove(&body);
    }
}

fn cleanup_state_reader_approval<S: StateData>(
    world: &mut World,
    life: LifeHandle,
    body: ConceptBodyHandle,
    expected_generation: u64,
) {
    let Some(entity) = LifeFactory::entity(world, life) else {
        return;
    };
    let Some(mut admission) = world.get_mut::<StateAdmission<S>>(entity) else {
        return;
    };
    if admission.approved_readers.get(&body) == Some(&expected_generation) {
        admission.approved_readers.remove(&body);
    }
}

fn cleanup_state_writer<S: StateData>(
    world: &mut World,
    life: LifeHandle,
    body: ConceptBodyHandle,
    expected_generation: u64,
) {
    let Some(entity) = LifeFactory::entity(world, life) else {
        return;
    };
    let Some(mut slot) = world.get_mut::<StateSlot<S>>(entity) else {
        return;
    };
    if slot.origin == body
        && slot
            .writer
            .as_ref()
            .is_some_and(|writer| writer.generation == expected_generation)
    {
        let writer = slot.writer.take().unwrap();
        slot.values.revoke_modify(&writer.access);
    }
}

fn cleanup_state_reader<S: StateData>(
    world: &mut World,
    life: LifeHandle,
    body: ConceptBodyHandle,
    expected_generation: u64,
) {
    let Some(entity) = LifeFactory::entity(world, life) else {
        return;
    };
    let Some(mut slot) = world.get_mut::<StateSlot<S>>(entity) else {
        return;
    };
    if slot
        .readers
        .get(&body)
        .is_some_and(|reader| reader.generation == expected_generation)
    {
        let reader = slot.readers.remove(&body).unwrap();
        slot.values.revoke_read(&reader.access);
    }
}
