mod management;
use crate::resource::{ManagedData, ModifyGrant, ReadGrant, Registration};
pub use management::LifeEventManagement;
use roundo_infrastructure::dsa::data_structure::resource_access::AccessError;

use crate::{
    AuthoritativeFixedTick, BodyCaller, BodyInstance, ConceptBodyFactory, ConceptBodyHandle,
    EventData, LifeAdministration, LifeFactory, LifeFixedSet, LifeHandle, LifeInstances,
    TypeRegistrationError,
    admin::{RegistrationDirection, forget_registration},
    data::{register_type, type_is_registered},
};
use bevy::{
    ecs::system::{SystemParam, SystemState},
    prelude::{
        App, Component, Entity, FixedUpdate, Has, IntoScheduleConfigs, Query, Res, ResMut,
        Resource, World,
    },
};
use std::{collections::BTreeMap, marker::PhantomData};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventError {
    ResourceAccess(AccessError),
    TypeNotRegistered,
    InvalidPolicy,
    LifeNotFound,
    BodyNotFound,
    MissingWriterDeclaration,
    MissingReaderDeclaration,
    WriterNotApproved,
    ReaderNotApproved,
    InterfaceAlreadyExists,
    InterfaceNotFound,
    WriterAlreadyRegistered,
    WriterNotRegistered,
    ReaderAlreadyRegistered,
    ReaderNotRegistered,
    InvalidLifetime,
    InvalidPayload,
    CapacityExceeded,
    TickOverflow,
    SequenceExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventPolicy {
    max_entries: usize,
    max_retained_bytes: usize,
    min_lifetime: u64,
    max_lifetime: u64,
}

impl EventPolicy {
    pub fn new(
        max_entries: usize,
        max_retained_bytes: usize,
        min_lifetime: u64,
        max_lifetime: u64,
    ) -> Result<Self, EventError> {
        if max_entries == 0
            || max_retained_bytes == 0
            || min_lifetime == 0
            || min_lifetime > max_lifetime
        {
            return Err(EventError::InvalidPolicy);
        }
        Ok(Self {
            max_entries,
            max_retained_bytes,
            min_lifetime,
            max_lifetime,
        })
    }
}

#[derive(Component)]
struct EventWriterDeclaration<E: EventData>(PhantomData<fn() -> E>);

#[derive(Component)]
struct EventReaderDeclaration<E: EventData>(PhantomData<fn() -> E>);

struct EventEntry<E> {
    expires_at: u64,
    retained_size: usize,
    payload: E,
}

struct EventBuffer<E> {
    entries: BTreeMap<u64, EventEntry<E>>,
    next_sequence: u64,
    retained_bytes: usize,
}

#[derive(Component)]
pub(crate) struct EventInterface<E: EventData> {
    generation: u64,
    writer: Option<(ConceptBodyHandle, Registration<ModifyGrant<EventBuffer<E>>>)>,
    approved_writers: BTreeMap<ConceptBodyHandle, u64>,
    readers: BTreeMap<ConceptBodyHandle, (u64, Registration<ReadGrant<EventBuffer<E>>>)>,
    approved_readers: BTreeMap<ConceptBodyHandle, u64>,
    data: ManagedData<EventBuffer<E>>,
    policy: EventPolicy,
}

impl<E: EventData> EventInterface<E> {
    fn write(
        &mut self,
        tick: u64,
        applicant: ConceptBodyHandle,
        payload: E,
        retained_size: usize,
        lifetime: u64,
    ) -> Result<(u64, u64, u64), EventError> {
        let (_, writer) = self
            .writer
            .as_ref()
            .filter(|(body, _)| *body == applicant)
            .ok_or(EventError::WriterNotRegistered)?;
        self.data
            .modify(&writer.access, |data| {
                if lifetime < self.policy.min_lifetime || lifetime > self.policy.max_lifetime {
                    return Err(EventError::InvalidLifetime);
                }
                let next_bytes = data
                    .retained_bytes
                    .checked_add(retained_size)
                    .ok_or(EventError::CapacityExceeded)?;
                if data.entries.len() >= self.policy.max_entries
                    || next_bytes > self.policy.max_retained_bytes
                {
                    return Err(EventError::CapacityExceeded);
                }
                let expires_at = tick.checked_add(lifetime).ok_or(EventError::TickOverflow)?;
                let sequence = data.next_sequence;
                data.next_sequence = sequence
                    .checked_add(1)
                    .ok_or(EventError::SequenceExhausted)?;
                data.entries.insert(
                    sequence,
                    EventEntry {
                        expires_at,
                        retained_size,
                        payload,
                    },
                );
                data.retained_bytes = next_bytes;
                Ok((expires_at, self.generation, sequence))
            })
            .map_err(EventError::ResourceAccess)?
    }

    fn read<R>(
        &mut self,
        tick: u64,
        applicant: ConceptBodyHandle,
        inspect: impl for<'a> FnOnce(&'a E) -> R,
    ) -> Result<Option<R>, EventError> {
        let (cursor, registration) = self
            .readers
            .get_mut(&applicant)
            .ok_or(EventError::ReaderNotRegistered)?;
        self.data
            .read(&registration.access, |data| {
                let Some((&sequence, _)) = data
                    .entries
                    .range(*cursor..)
                    .find(|(_, entry)| tick < entry.expires_at)
                else {
                    return None;
                };
                *cursor = sequence + 1;
                Some(inspect(&data.entries[&sequence].payload))
            })
            .map_err(EventError::ResourceAccess)
    }
}

/// Owned control-plane metadata, not a payload view or registration capability.
/// Taking a snapshot does not advance any reader cursor or remove entries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventInterfaceEvidence {
    policy: EventPolicy,
    generation: u64,
    writer: Option<ConceptBodyHandle>,
    readers: usize,
    retained_entries: usize,
    retained_bytes: usize,
    unexpired_entries: usize,
}
impl EventInterfaceEvidence {
    pub const fn policy(self) -> EventPolicy {
        self.policy
    }
    pub const fn generation(self) -> u64 {
        self.generation
    }
    pub const fn writer(self) -> Option<ConceptBodyHandle> {
        self.writer
    }
    pub const fn retained_entries(self) -> usize {
        self.retained_entries
    }
    pub const fn retained_bytes(self) -> usize {
        self.retained_bytes
    }
    pub const fn has_writer(self) -> bool {
        self.writer.is_some()
    }
    pub const fn reader_count(self) -> usize {
        self.readers
    }
    pub const fn unexpired_entries(self) -> usize {
        self.unexpired_entries
    }
    pub const fn is_unregistered_and_empty(self) -> bool {
        self.writer.is_none() && self.readers == 0 && self.unexpired_entries == 0
    }
}

/// Read-only control-plane evidence of current Life-held Event registration.
/// Checks live Factory generations without consuming payloads/cursors or granting
/// approval/registration. Reverse cleanup metadata is never authorization truth.
#[derive(SystemParam)]
pub struct LifeEventRegistrations<'w, 's, E: EventData> {
    instances: LifeInstances<'w, 's>,
    interfaces: Query<'w, 's, &'static EventInterface<E>>,
    tick: Option<Res<'w, AuthoritativeFixedTick>>,
    declarations: Query<
        'w,
        's,
        (
            Has<EventWriterDeclaration<E>>,
            Has<EventReaderDeclaration<E>>,
        ),
    >,
}

impl<E: EventData> LifeEventRegistrations<'_, '_, E> {
    pub fn interface_evidence(
        &self,
        life: LifeHandle,
    ) -> Result<EventInterfaceEvidence, EventError> {
        let entity = self
            .instances
            .resolve_life(life)
            .map_err(|_| EventError::LifeNotFound)?;
        let tick = self
            .tick
            .as_ref()
            .ok_or(EventError::TypeNotRegistered)?
            .get();
        let interface = self
            .interfaces
            .get(entity)
            .map_err(|_| EventError::InterfaceNotFound)?;
        let (retained_entries, retained_bytes, unexpired_entries) =
            interface.data.inspect(|data| {
                (
                    data.entries.len(),
                    data.retained_bytes,
                    data.entries
                        .values()
                        .filter(|entry| tick < entry.expires_at)
                        .count(),
                )
            });
        Ok(EventInterfaceEvidence {
            policy: interface.policy,
            generation: interface.generation,
            writer: interface.writer.as_ref().map(|value| value.0),
            readers: interface.readers.len(),
            retained_entries,
            retained_bytes,
            unexpired_entries,
        })
    }
    pub fn validate_writer_declaration(&self, body: ConceptBodyHandle) -> Result<(), EventError> {
        let entity = self
            .instances
            .resolve_body(body)
            .map_err(|_| EventError::BodyNotFound)?;
        self.declarations
            .get(entity)
            .is_ok_and(|value| value.0)
            .then_some(())
            .ok_or(EventError::MissingWriterDeclaration)
    }
    pub fn validate_reader_declaration(&self, body: ConceptBodyHandle) -> Result<(), EventError> {
        let entity = self
            .instances
            .resolve_body(body)
            .map_err(|_| EventError::BodyNotFound)?;
        self.declarations
            .get(entity)
            .is_ok_and(|value| value.1)
            .then_some(())
            .ok_or(EventError::MissingReaderDeclaration)
    }
    pub fn validate_writer(
        &self,
        life: LifeHandle,
        body: ConceptBodyHandle,
    ) -> Result<(), EventError> {
        let entity = self
            .instances
            .resolve_life(life)
            .map_err(|_| EventError::LifeNotFound)?;
        self.instances
            .resolve_body(body)
            .map_err(|_| EventError::BodyNotFound)?;
        let interface = self
            .interfaces
            .get(entity)
            .map_err(|_| EventError::InterfaceNotFound)?;
        interface
            .writer
            .as_ref()
            .filter(|(registered, _)| *registered == body)
            .ok_or(EventError::WriterNotRegistered)?
            .1
            .access
            .check()
            .map_err(EventError::ResourceAccess)
    }

    pub fn validate_reader(
        &self,
        life: LifeHandle,
        body: ConceptBodyHandle,
    ) -> Result<(), EventError> {
        let entity = self
            .instances
            .resolve_life(life)
            .map_err(|_| EventError::LifeNotFound)?;
        self.instances
            .resolve_body(body)
            .map_err(|_| EventError::BodyNotFound)?;
        let interface = self
            .interfaces
            .get(entity)
            .map_err(|_| EventError::InterfaceNotFound)?;
        interface
            .readers
            .get(&body)
            .ok_or(EventError::ReaderNotRegistered)?
            .1
            .access
            .check()
            .map_err(EventError::ResourceAccess)
    }
}

/// Typed FIFO reader with independent registered cursor. Payload borrows are
/// confined to the callback; copying creates independently owned data.
#[derive(SystemParam)]
pub struct LifeEventReader<'w, 's, E: EventData> {
    instances: LifeInstances<'w, 's>,
    tick: Option<Res<'w, AuthoritativeFixedTick>>,
    interfaces: Query<'w, 's, &'static mut EventInterface<E>>,
}

impl<E: EventData> LifeEventReader<'_, '_, E> {
    pub fn read_next_with<R>(
        &mut self,
        body: &BodyInstance<'_>,
        life: LifeHandle,
        inspect: impl for<'a> FnOnce(&'a E) -> R,
    ) -> Result<Option<R>, EventError> {
        let tick = self
            .tick
            .as_ref()
            .ok_or(EventError::TypeNotRegistered)?
            .get();
        let applicant = self
            .instances
            .applicant(body)
            .map_err(|_| EventError::BodyNotFound)?;
        let entity = self
            .instances
            .resolve_life(life)
            .map_err(|_| EventError::LifeNotFound)?;
        self.interfaces
            .get_mut(entity)
            .map_err(|_| EventError::InterfaceNotFound)?
            .read(tick, applicant, inspect)
    }
}

/// Typed Event writer. The registered writer, bounded capacity and positive
/// lifetime are checked before acceptance; due work is installed synchronously.
#[derive(SystemParam)]
pub struct LifeEventWriter<'w, 's, E: EventData> {
    instances: LifeInstances<'w, 's>,
    tick: Option<Res<'w, AuthoritativeFixedTick>>,
    runtime: Option<ResMut<'w, EventTypeRuntime<E>>>,
    interfaces: Query<'w, 's, &'static mut EventInterface<E>>,
}

impl<E: EventData> LifeEventWriter<'_, '_, E> {
    pub fn write_event(
        &mut self,
        body: &BodyInstance<'_>,
        life: LifeHandle,
        payload: E,
        lifetime: u64,
    ) -> Result<(), EventError> {
        let tick = self
            .tick
            .as_ref()
            .ok_or(EventError::TypeNotRegistered)?
            .get();
        let applicant = self
            .instances
            .applicant(body)
            .map_err(|_| EventError::BodyNotFound)?;
        let entity = self
            .instances
            .resolve_life(life)
            .map_err(|_| EventError::LifeNotFound)?;
        if !payload.validate() {
            return Err(EventError::InvalidPayload);
        }
        let retained_size = payload.retained_size();
        let runtime = self.runtime.as_mut().ok_or(EventError::TypeNotRegistered)?;
        let (expires_at, generation, sequence) = self
            .interfaces
            .get_mut(entity)
            .map_err(|_| EventError::InterfaceNotFound)?
            .write(tick, applicant, payload, retained_size, lifetime)?;
        runtime
            .buckets
            .entry(expires_at)
            .or_default()
            .push(DueEvent {
                life_entity: entity,
                interface_generation: generation,
                sequence,
            });
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct DueEvent {
    life_entity: Entity,
    interface_generation: u64,
    sequence: u64,
}

#[derive(Resource)]
struct EventTypeRuntime<E: EventData> {
    next_interface_generation: u64,
    buckets: BTreeMap<u64, Vec<DueEvent>>,
    marker: PhantomData<fn() -> E>,
}

impl<E: EventData> Default for EventTypeRuntime<E> {
    fn default() -> Self {
        Self {
            next_interface_generation: 1,
            buckets: BTreeMap::new(),
            marker: PhantomData,
        }
    }
}

impl<E: EventData> EventTypeRuntime<E> {
    fn forget_interface_due(&mut self, entity: Entity, generation: u64) {
        self.buckets.retain(|_, bucket| {
            bucket
                .retain(|due| due.life_entity != entity || due.interface_generation != generation);
            !bucket.is_empty()
        });
    }
}

pub fn register_event_type<E: EventData>(app: &mut App) -> Result<(), TypeRegistrationError> {
    register_type::<E>(app.world_mut())?;
    app.world_mut()
        .register_component_hooks::<EventInterface<E>>()
        .on_remove(remove_interface_due::<E>);
    app.init_resource::<EventTypeRuntime<E>>().add_systems(
        FixedUpdate,
        flush_due::<E>.in_set(LifeFixedSet::ExpireEvents),
    );
    Ok(())
}

pub(crate) fn declare_event_writer_on_entity<E: EventData>(
    world: &mut World,
    entity: Entity,
) -> Result<(), EventError> {
    if !type_is_registered::<E>(world) {
        return Err(EventError::TypeNotRegistered);
    }
    world
        .get_entity_mut(entity)
        .map_err(|_| EventError::BodyNotFound)?
        .insert(EventWriterDeclaration::<E>(PhantomData));
    Ok(())
}

pub fn declare_event_writer<E: EventData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    body: ConceptBodyHandle,
) -> Result<(), EventError> {
    if !administration.matches(world) {
        return Err(EventError::LifeNotFound);
    }
    let entity = ConceptBodyFactory::entity(world, body).ok_or(EventError::BodyNotFound)?;
    declare_event_writer_on_entity::<E>(world, entity)
}

pub(crate) fn declare_event_reader_on_entity<E: EventData>(
    world: &mut World,
    entity: Entity,
) -> Result<(), EventError> {
    if !type_is_registered::<E>(world) {
        return Err(EventError::TypeNotRegistered);
    }
    world
        .get_entity_mut(entity)
        .map_err(|_| EventError::BodyNotFound)?
        .insert(EventReaderDeclaration::<E>(PhantomData));
    Ok(())
}

pub fn declare_event_reader<E: EventData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    body: ConceptBodyHandle,
) -> Result<(), EventError> {
    if !administration.matches(world) {
        return Err(EventError::LifeNotFound);
    }
    let entity = ConceptBodyFactory::entity(world, body).ok_or(EventError::BodyNotFound)?;
    declare_event_reader_on_entity::<E>(world, entity)
}

pub(crate) fn install_event_interface_on_entity<E: EventData>(
    world: &mut World,
    entity: Entity,
    policy: EventPolicy,
) -> Result<(), EventError> {
    if !type_is_registered::<E>(world) {
        return Err(EventError::TypeNotRegistered);
    }
    if world.get::<EventInterface<E>>(entity).is_some() {
        return Err(EventError::InterfaceAlreadyExists);
    }
    let data = ManagedData::create(
        world,
        EventBuffer::<E> {
            entries: BTreeMap::new(),
            next_sequence: 0,
            retained_bytes: 0,
        },
    )
    .map_err(EventError::ResourceAccess)?;
    let generation = {
        let mut runtime = world
            .get_resource_mut::<EventTypeRuntime<E>>()
            .ok_or(EventError::TypeNotRegistered)?;
        let generation = runtime.next_interface_generation;
        runtime.next_interface_generation = generation
            .checked_add(1)
            .ok_or(EventError::SequenceExhausted)?;
        generation
    };
    world
        .get_entity_mut(entity)
        .map_err(|_| EventError::LifeNotFound)?
        .insert(EventInterface::<E> {
            generation,
            writer: None,
            approved_writers: BTreeMap::new(),
            readers: BTreeMap::new(),
            approved_readers: BTreeMap::new(),
            data,
            policy,
        });
    Ok(())
}

pub fn install_event_interface<E: EventData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    life: LifeHandle,
    policy: EventPolicy,
) -> Result<(), EventError> {
    if !administration.matches(world) {
        return Err(EventError::LifeNotFound);
    }
    let entity = LifeFactory::entity(world, life).ok_or(EventError::LifeNotFound)?;
    install_event_interface_on_entity::<E>(world, entity, policy)
}

fn with_event_management<E: EventData, R>(
    world: &mut World,
    operation: impl FnOnce(&mut LifeEventManagement<E>) -> R,
) -> R {
    world.init_resource::<crate::admin::RegistrationReverseIndex>();
    let mut state = SystemState::<LifeEventManagement<E>>::new(world);
    operation(
        &mut state
            .get_mut(world)
            .expect("Event management resources initialized"),
    )
}

pub fn approve_event_writer<E: EventData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    life: LifeHandle,
    body: ConceptBodyHandle,
) -> Result<(), EventError> {
    if !administration.matches(world) {
        return Err(EventError::LifeNotFound);
    }
    with_event_management::<E, _>(world, |management| management.approve_writer(life, body))
}

pub fn approve_event_reader<E: EventData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    life: LifeHandle,
    body: ConceptBodyHandle,
) -> Result<(), EventError> {
    if !administration.matches(world) {
        return Err(EventError::LifeNotFound);
    }
    with_event_management::<E, _>(world, |management| management.approve_reader(life, body))
}

pub fn register_event_writer<E: EventData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    life: LifeHandle,
    body: ConceptBodyHandle,
) -> Result<(), EventError> {
    if !administration.matches(world) {
        return Err(EventError::LifeNotFound);
    }
    with_event_management::<E, _>(world, |management| management.register_writer(life, body))
}

pub fn register_event_reader<E: EventData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    life: LifeHandle,
    body: ConceptBodyHandle,
) -> Result<(), EventError> {
    if !administration.matches(world) {
        return Err(EventError::LifeNotFound);
    }
    with_event_management::<E, _>(world, |management| management.register_reader(life, body))
}

pub fn deregister_event_writer<E: EventData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    life: LifeHandle,
    body: ConceptBodyHandle,
) -> Result<(), EventError> {
    if !administration.matches(world) {
        return Err(EventError::LifeNotFound);
    }
    with_event_management::<E, _>(world, |management| management.deregister_writer(life, body))
}

pub fn deregister_event_reader<E: EventData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    life: LifeHandle,
    body: ConceptBodyHandle,
) -> Result<(), EventError> {
    if !administration.matches(world) {
        return Err(EventError::LifeNotFound);
    }
    with_event_management::<E, _>(world, |management| management.deregister_reader(life, body))
}

pub fn remove_event_interface<E: EventData>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    life: LifeHandle,
) -> Result<(), EventError> {
    if !administration.matches(world) {
        return Err(EventError::LifeNotFound);
    }
    let entity = LifeFactory::entity(world, life).ok_or(EventError::LifeNotFound)?;
    let interface = world
        .get_entity_mut(entity)
        .map_err(|_| EventError::LifeNotFound)?
        .take::<EventInterface<E>>()
        .ok_or(EventError::InterfaceNotFound)?;
    if let Some((writer, _)) = interface.writer {
        forget_registration(world, writer, life, E::KEY, RegistrationDirection::Writer);
    }
    for reader in interface.readers.into_keys() {
        forget_registration(world, reader, life, E::KEY, RegistrationDirection::Reader);
    }
    for (body, generation) in interface.approved_writers {
        crate::admin::forget_registration_generation(
            world,
            body,
            life,
            E::KEY,
            RegistrationDirection::ApprovalWriter,
            generation,
        );
    }
    for (body, generation) in interface.approved_readers {
        crate::admin::forget_registration_generation(
            world,
            body,
            life,
            E::KEY,
            RegistrationDirection::ApprovalReader,
            generation,
        );
    }
    Ok(())
}

fn write_event<E: EventData>(
    world: &mut World,
    life: LifeHandle,
    applicant: ConceptBodyHandle,
    payload: E,
    lifetime: u64,
) -> Result<(), EventError> {
    let tick = current_tick(world)?;
    if !ConceptBodyFactory::is_live(world, applicant) {
        return Err(EventError::BodyNotFound);
    }
    let life_entity = LifeFactory::entity(world, life).ok_or(EventError::LifeNotFound)?;
    // Keep legacy synchronous callers on the same storage rules as native systems.
    // Validate payload before interface lookup, preserving the existing error order.
    if !payload.validate() {
        return Err(EventError::InvalidPayload);
    }
    let retained_size = payload.retained_size();
    let (expires_at, generation, sequence) = world
        .get_mut::<EventInterface<E>>(life_entity)
        .ok_or(EventError::InterfaceNotFound)?
        .write(tick, applicant, payload, retained_size, lifetime)?;
    world
        .get_resource_mut::<EventTypeRuntime<E>>()
        .ok_or(EventError::TypeNotRegistered)?
        .buckets
        .entry(expires_at)
        .or_default()
        .push(DueEvent {
            life_entity,
            interface_generation: generation,
            sequence,
        });
    Ok(())
}

fn read_next_with<E: EventData, R>(
    world: &mut World,
    life: LifeHandle,
    applicant: ConceptBodyHandle,
    inspect: impl for<'a> FnOnce(&'a E) -> R,
) -> Result<Option<R>, EventError> {
    let tick = current_tick(world)?;
    if !ConceptBodyFactory::is_live(world, applicant) {
        return Err(EventError::BodyNotFound);
    }
    let life_entity = LifeFactory::entity(world, life).ok_or(EventError::LifeNotFound)?;
    let mut interface = world
        .get_mut::<EventInterface<E>>(life_entity)
        .ok_or(EventError::InterfaceNotFound)?;
    interface.read(tick, applicant, inspect)
}

impl BodyCaller<'_> {
    pub fn write_event<E: EventData>(
        &mut self,
        life: LifeHandle,
        payload: E,
        lifetime: u64,
    ) -> Result<(), EventError> {
        let applicant = self.handle();
        write_event(self.world_mut(), life, applicant, payload, lifetime)
    }
    pub fn read_next_with<E: EventData, R>(
        &mut self,
        life: LifeHandle,
        inspect: impl for<'a> FnOnce(&'a E) -> R,
    ) -> Result<Option<R>, EventError> {
        let applicant = self.handle();
        read_next_with(self.world_mut(), life, applicant, inspect)
    }
}

fn cleanup_event_writer_approval<E: EventData>(
    world: &mut World,
    life: LifeHandle,
    body: ConceptBodyHandle,
    expected_generation: u64,
) {
    let Some(entity) = LifeFactory::entity(world, life) else {
        return;
    };
    let Some(mut interface) = world.get_mut::<EventInterface<E>>(entity) else {
        return;
    };
    if interface.approved_writers.get(&body) == Some(&expected_generation) {
        interface.approved_writers.remove(&body);
    }
}

fn cleanup_event_reader_approval<E: EventData>(
    world: &mut World,
    life: LifeHandle,
    body: ConceptBodyHandle,
    expected_generation: u64,
) {
    let Some(entity) = LifeFactory::entity(world, life) else {
        return;
    };
    let Some(mut interface) = world.get_mut::<EventInterface<E>>(entity) else {
        return;
    };
    if interface.approved_readers.get(&body) == Some(&expected_generation) {
        interface.approved_readers.remove(&body);
    }
}

fn cleanup_event_writer<E: EventData>(
    world: &mut World,
    life: LifeHandle,
    body: ConceptBodyHandle,
    expected_generation: u64,
) {
    let Some(entity) = LifeFactory::entity(world, life) else {
        return;
    };
    let Some(mut interface) = world.get_mut::<EventInterface<E>>(entity) else {
        return;
    };
    if interface
        .writer
        .as_ref()
        .is_some_and(|(writer, registration)| {
            *writer == body && registration.generation == expected_generation
        })
    {
        let (_, writer) = interface.writer.take().unwrap();
        interface.data.revoke_modify(&writer.access);
    }
}

fn cleanup_event_reader<E: EventData>(
    world: &mut World,
    life: LifeHandle,
    body: ConceptBodyHandle,
    expected_generation: u64,
) {
    let Some(entity) = LifeFactory::entity(world, life) else {
        return;
    };
    let Some(mut interface) = world.get_mut::<EventInterface<E>>(entity) else {
        return;
    };
    if interface
        .readers
        .get(&body)
        .is_some_and(|(_, registration)| registration.generation == expected_generation)
    {
        let (_, reader) = interface.readers.remove(&body).unwrap();
        interface.data.revoke_read(&reader.access);
    }
}

fn current_tick(world: &World) -> Result<u64, EventError> {
    world
        .get_resource::<AuthoritativeFixedTick>()
        .copied()
        .map(AuthoritativeFixedTick::get)
        .ok_or(EventError::TypeNotRegistered)
}

fn remove_interface_due<E: EventData>(
    mut world: bevy::ecs::world::DeferredWorld,
    context: bevy::ecs::lifecycle::HookContext,
) {
    // on_remove can still inspect the outgoing component. Only metadata is
    // reclaimed here; its Owner and payloads retain their normal drop point.
    let Some(interface) = world.get::<EventInterface<E>>(context.entity) else {
        return;
    };
    let generation = interface.generation;
    if let Some(mut runtime) = world.get_resource_mut::<EventTypeRuntime<E>>() {
        runtime.forget_interface_due(context.entity, generation);
    }
}

fn flush_due<E: EventData>(
    tick: Res<AuthoritativeFixedTick>,
    mut runtime: ResMut<EventTypeRuntime<E>>,
    mut interfaces: Query<&mut EventInterface<E>>,
) {
    let tick = tick.get();
    let due = {
        let keys = runtime
            .buckets
            .range(..=tick)
            .map(|(&key, _)| key)
            .collect::<Vec<_>>();
        keys.into_iter()
            .flat_map(|key| runtime.buckets.remove(&key).unwrap_or_default())
            .collect::<Vec<_>>()
    };
    for due in due {
        let Ok(mut interface) = interfaces.get_mut(due.life_entity) else {
            continue;
        };
        if interface.generation != due.interface_generation {
            continue;
        }
        // Lifetime order need not match FIFO order. Address only this due entry;
        // never search the remaining Blackboard for its sequence.
        interface.data.maintain(|data| {
            if data
                .entries
                .get(&due.sequence)
                .is_some_and(|entry| entry.expires_at <= tick)
            {
                if let Some(entry) = data.entries.remove(&due.sequence) {
                    data.retained_bytes -= entry.retained_size;
                }
            }
        });
    }
}
