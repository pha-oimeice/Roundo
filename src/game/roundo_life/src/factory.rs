use crate::handle::{ConceptBodyHandle, ConceptBodyIdentity, LifeHandle, LifeIdentity};
use bevy::prelude::{Bundle, Component, Entity, Event, Resource, World};

// Infrastructure roots live for the World, not for a gameplay removal request.
#[derive(Component)]
struct WorldRoot;

/// Short-lived trusted control-plane authority.
///
/// [`with_life_administration`] mints it only for a trusted composition host
/// that already owns exclusive `&mut World`. Body-facing code receives neither
/// World nor this authority; it only receives a temporary [`BodyCaller`] in a
/// host-bound callback. This is an authority boundary for cooperative runtime
/// code, not a sandbox against a malicious all-powerful World host.
///
pub struct LifeAdministration<'world> {
    world: *mut World,
    _world: std::marker::PhantomData<&'world mut World>,
}

/// A non-transferable Body-facing execution context. It owns the original
/// World's short borrow and exposes only typed data-plane operations.
pub struct BodyCaller<'world> {
    world: &'world mut World,
    handle: ConceptBodyHandle,
}

impl BodyCaller<'_> {
    pub(crate) fn handle(&self) -> ConceptBodyHandle {
        self.handle
    }
    pub(crate) fn world_mut(&mut self) -> &mut World {
        self.world
    }
}

impl LifeAdministration<'_> {
    /// Returns whether this administration was minted for `world`.
    ///
    /// This read-only affinity check grants no additional capability and does
    /// not expose a World reference or pointer. APIs that accept both an
    /// administration and a World use it to reject cross-World authority.
    pub fn matches(&self, world: &World) -> bool {
        std::ptr::eq(self.world, world)
    }
}

/// Runs one Body-facing operation in the provenance of `body`. The callback is
/// deliberately not given World or administration. An administration minted
/// for another World is rejected.
///
pub fn with_body_caller<R>(
    world: &mut World,
    administration: &LifeAdministration<'_>,
    body: ConceptBodyHandle,
    operation: impl for<'caller> FnOnce(&'caller mut BodyCaller<'caller>) -> R,
) -> Result<R, FactoryError> {
    if !std::ptr::eq(administration.world, world) || !ConceptBodyFactory::is_live(world, body) {
        return Err(FactoryError::InstanceNotFound);
    }
    Ok(operation(&mut BodyCaller {
        world,
        handle: body,
    }))
}

/// Runs a trusted composition administration transaction for an exclusive-World
/// composition host. Handles intentionally do not grant administration or removal.
pub fn with_life_administration<R>(
    world: &mut World,
    operation: impl for<'world> FnOnce(&'world mut World, &'world LifeAdministration<'world>) -> R,
) -> R {
    let world_ptr = world as *mut World;
    operation(
        world,
        &LifeAdministration {
            world: world_ptr,
            _world: std::marker::PhantomData,
        },
    )
}

/// Post-invalidation notification for trusted typed cleanup observers. Factory
/// identity has already become unavailable; this event grants no authority.
#[derive(Event, Clone, Copy, Debug, Eq, PartialEq)]
pub enum FactoryInvalidation {
    Life(LifeHandle),
    ConceptBody(ConceptBodyHandle),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FactoryError {
    InstanceNotFound,
    GenerationExhausted,
    PrototypeRejected,
    CatalogueNotClosed,
    /// A World-lifetime infrastructure Body cannot be removed individually.
    ProtectedInstance,
}

trait LifePrototypeDeclaration: Send + Sync {
    fn materialize(&self, world: &mut World, life_entity: Entity) -> Result<(), FactoryError>;
}

struct EventInterfaceDeclaration<E: crate::EventData>(
    crate::EventPolicy,
    std::marker::PhantomData<E>,
);
impl<E: crate::EventData> LifePrototypeDeclaration for EventInterfaceDeclaration<E> {
    fn materialize(&self, world: &mut World, entity: Entity) -> Result<(), FactoryError> {
        crate::event::install_event_interface_on_entity::<E>(world, entity, self.0)
            .map_err(|_| FactoryError::PrototypeRejected)
    }
}

struct StatePolicyDeclaration<S: crate::StateData>(crate::StatePolicy, std::marker::PhantomData<S>);
impl<S: crate::StateData> LifePrototypeDeclaration for StatePolicyDeclaration<S> {
    fn materialize(&self, world: &mut World, entity: Entity) -> Result<(), FactoryError> {
        crate::state::install_state_policy_on_entity::<S>(world, entity, self.0)
            .map_err(|_| FactoryError::PrototypeRejected)
    }
}

#[derive(Default)]
pub struct LifePrototype {
    declarations: Vec<Box<dyn LifePrototypeDeclaration>>,
}

impl LifePrototype {
    pub fn empty() -> Self {
        Self::default()
    }
    pub fn with_event_interface<E: crate::EventData>(mut self, policy: crate::EventPolicy) -> Self {
        self.declarations
            .push(Box::new(EventInterfaceDeclaration::<E>(
                policy,
                std::marker::PhantomData,
            )));
        self
    }
    pub fn with_state_policy<S: crate::StateData>(mut self, policy: crate::StatePolicy) -> Self {
        self.declarations.push(Box::new(StatePolicyDeclaration::<S>(
            policy,
            std::marker::PhantomData,
        )));
        self
    }
}

trait BodyAccessDeclaration: Send + Sync {
    fn materialize(&self, world: &mut World, body_entity: Entity) -> Result<(), FactoryError>;
}
struct EventWriterAccess<E: crate::EventData>(std::marker::PhantomData<E>);
impl<E: crate::EventData> BodyAccessDeclaration for EventWriterAccess<E> {
    fn materialize(&self, world: &mut World, entity: Entity) -> Result<(), FactoryError> {
        crate::event::declare_event_writer_on_entity::<E>(world, entity)
            .map_err(|_| FactoryError::PrototypeRejected)
    }
}
struct EventReaderAccess<E: crate::EventData>(std::marker::PhantomData<E>);
impl<E: crate::EventData> BodyAccessDeclaration for EventReaderAccess<E> {
    fn materialize(&self, world: &mut World, entity: Entity) -> Result<(), FactoryError> {
        crate::event::declare_event_reader_on_entity::<E>(world, entity)
            .map_err(|_| FactoryError::PrototypeRejected)
    }
}
struct StateWriterAccess<S: crate::StateData>(std::marker::PhantomData<S>);
impl<S: crate::StateData> BodyAccessDeclaration for StateWriterAccess<S> {
    fn materialize(&self, world: &mut World, entity: Entity) -> Result<(), FactoryError> {
        crate::state::declare_state_writer_on_entity::<S>(world, entity)
            .map_err(|_| FactoryError::PrototypeRejected)
    }
}
struct StateReaderAccess<S: crate::StateData>(std::marker::PhantomData<S>);
impl<S: crate::StateData> BodyAccessDeclaration for StateReaderAccess<S> {
    fn materialize(&self, world: &mut World, entity: Entity) -> Result<(), FactoryError> {
        crate::state::declare_state_reader_on_entity::<S>(world, entity)
            .map_err(|_| FactoryError::PrototypeRejected)
    }
}

pub struct ConceptBodyPrototype<B: Bundle> {
    bundle: B,
    declarations: Vec<Box<dyn BodyAccessDeclaration>>,
}
impl<B: Bundle> ConceptBodyPrototype<B> {
    pub fn new(bundle: B) -> Self {
        Self {
            bundle,
            declarations: Vec::new(),
        }
    }
    pub fn with_event_writer<E: crate::EventData>(mut self) -> Self {
        self.declarations
            .push(Box::new(EventWriterAccess::<E>(std::marker::PhantomData)));
        self
    }
    pub fn with_event_reader<E: crate::EventData>(mut self) -> Self {
        self.declarations
            .push(Box::new(EventReaderAccess::<E>(std::marker::PhantomData)));
        self
    }
    pub fn with_state_writer<S: crate::StateData>(mut self) -> Self {
        self.declarations
            .push(Box::new(StateWriterAccess::<S>(std::marker::PhantomData)));
        self
    }
    pub fn with_state_reader<S: crate::StateData>(mut self) -> Self {
        self.declarations
            .push(Box::new(StateReaderAccess::<S>(std::marker::PhantomData)));
        self
    }
}

#[derive(Clone, Copy)]
struct Slot {
    generation: u64,
    entity: Option<Entity>,
}

#[derive(Default)]
struct Slots {
    values: Vec<Slot>,
    free: Vec<u32>,
}

impl Slots {
    fn reserve(&mut self) -> Result<(u32, u64), FactoryError> {
        if let Some(slot) = self.free.pop() {
            let value = &self.values[slot as usize];
            debug_assert!(value.entity.is_none());
            return Ok((slot, value.generation));
        }
        let slot =
            u32::try_from(self.values.len()).map_err(|_| FactoryError::GenerationExhausted)?;
        self.values.push(Slot {
            generation: 1,
            entity: None,
        });
        Ok((slot, 1))
    }

    fn publish(&mut self, slot: u32, generation: u64, entity: Entity) {
        let value = &mut self.values[slot as usize];
        debug_assert_eq!(value.generation, generation);
        debug_assert!(value.entity.is_none());
        value.entity = Some(entity);
    }

    fn resolve(&self, slot: u32, generation: u64) -> Option<Entity> {
        let value = self.values.get(slot as usize)?;
        (value.generation == generation)
            .then_some(value.entity)
            .flatten()
    }

    fn invalidate(&mut self, slot: u32, generation: u64) -> Result<Entity, FactoryError> {
        let value = self
            .values
            .get_mut(slot as usize)
            .filter(|value| value.generation == generation)
            .ok_or(FactoryError::InstanceNotFound)?;
        let entity = value.entity.ok_or(FactoryError::InstanceNotFound)?;
        let next_generation = value
            .generation
            .checked_add(1)
            .ok_or(FactoryError::GenerationExhausted)?;
        value.entity = None;
        value.generation = next_generation;
        self.free.push(slot);
        Ok(entity)
    }
}

#[derive(Resource, Default)]
pub(crate) struct LifeIndex(Slots);

#[derive(Resource, Default)]
pub(crate) struct ConceptBodyIndex(Slots);

impl LifeIndex {
    pub(crate) fn resolve(&self, handle: LifeHandle) -> Option<Entity> {
        self.0.resolve(handle.slot(), handle.generation())
    }
}

impl ConceptBodyIndex {
    pub(crate) fn resolve(&self, handle: ConceptBodyHandle) -> Option<Entity> {
        self.0.resolve(handle.slot(), handle.generation())
    }
}

pub struct LifeFactory;

impl LifeFactory {
    pub fn create(
        world: &mut World,
        _administration: &LifeAdministration<'_>,
        prototype: &LifePrototype,
    ) -> Result<LifeHandle, FactoryError> {
        if !_administration.matches(world) {
            return Err(FactoryError::InstanceNotFound);
        }
        if !crate::data::catalogue_is_closed(world) {
            return Err(FactoryError::CatalogueNotClosed);
        }
        world.init_resource::<LifeIndex>();
        let (slot, generation) = world.resource_mut::<LifeIndex>().0.reserve()?;
        let handle = LifeHandle::new(slot, generation);
        let entity = world.spawn_empty().id();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            world.entity_mut(entity).insert(LifeIdentity(handle));
            for declaration in &prototype.declarations {
                declaration.materialize(world, entity)?;
            }
            Ok(())
        }));
        match result {
            Ok(Ok(())) => {
                world
                    .resource_mut::<LifeIndex>()
                    .0
                    .publish(slot, generation, entity);
                Ok(handle)
            }
            Ok(Err(error)) => {
                let _ = world.despawn(entity);
                world.resource_mut::<LifeIndex>().0.free.push(slot);
                Err(error)
            }
            Err(payload) => {
                let _ = world.despawn(entity);
                world.resource_mut::<LifeIndex>().0.free.push(slot);
                std::panic::resume_unwind(payload)
            }
        }
    }

    pub fn remove(
        world: &mut World,
        _administration: &LifeAdministration<'_>,
        handle: LifeHandle,
    ) -> Result<(), FactoryError> {
        if !_administration.matches(world) {
            return Err(FactoryError::InstanceNotFound);
        }
        let entity = world
            .get_resource_mut::<LifeIndex>()
            .ok_or(FactoryError::InstanceNotFound)?
            .0
            .invalidate(handle.slot(), handle.generation())?;
        crate::admin::forget_life(world, handle);
        let _ = world.despawn(entity);
        world.trigger(FactoryInvalidation::Life(handle));
        world.flush();
        Ok(())
    }

    pub fn is_live(world: &World, handle: LifeHandle) -> bool {
        Self::entity(world, handle).is_some()
    }

    /// Resolves a live Handle for immediate trusted ECS composition work.
    pub fn resolve_entity(world: &World, handle: LifeHandle) -> Result<Entity, FactoryError> {
        Self::entity(world, handle).ok_or(FactoryError::InstanceNotFound)
    }

    pub(crate) fn entity(world: &World, handle: LifeHandle) -> Option<Entity> {
        let entity = world
            .get_resource::<LifeIndex>()?
            .0
            .resolve(handle.slot(), handle.generation())?;
        (world.get::<LifeIdentity>(entity)?.0 == handle).then_some(entity)
    }
}

pub struct ConceptBodyFactory;

impl ConceptBodyFactory {
    pub fn create<B: Bundle>(
        world: &mut World,
        _administration: &LifeAdministration<'_>,
        prototype: ConceptBodyPrototype<B>,
    ) -> Result<ConceptBodyHandle, FactoryError> {
        if !_administration.matches(world) {
            return Err(FactoryError::InstanceNotFound);
        }
        if !crate::data::catalogue_is_closed(world) {
            return Err(FactoryError::CatalogueNotClosed);
        }
        Self::materialize(world, prototype)
    }

    /// Creates an empty infrastructure Concept Body that lives until World drop.
    ///
    /// This has no typed declarations and may be created before catalogue close,
    /// without closing or changing the catalogue. Ordinary/scoped/scheduled
    /// Factory removal rejects it with `ProtectedInstance`. Raw trusted World
    /// mutation is not sandboxed. This is not a gameplay spawn interface.
    pub fn create_world_root(
        world: &mut World,
        administration: &LifeAdministration<'_>,
    ) -> Result<ConceptBodyHandle, FactoryError> {
        if !administration.matches(world) {
            return Err(FactoryError::InstanceNotFound);
        }
        Self::materialize(world, ConceptBodyPrototype::new(WorldRoot))
    }

    fn materialize<B: Bundle>(
        world: &mut World,
        prototype: ConceptBodyPrototype<B>,
    ) -> Result<ConceptBodyHandle, FactoryError> {
        world.init_resource::<ConceptBodyIndex>();
        let (slot, generation) = world.resource_mut::<ConceptBodyIndex>().0.reserve()?;
        let handle = ConceptBodyHandle::new(slot, generation);
        let entity = world.spawn_empty().id();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            world
                .entity_mut(entity)
                .insert((prototype.bundle, ConceptBodyIdentity(handle)));
            for declaration in &prototype.declarations {
                declaration.materialize(world, entity)?;
            }
            Ok(())
        }));
        match result {
            Ok(Ok(())) => {
                world
                    .resource_mut::<ConceptBodyIndex>()
                    .0
                    .publish(slot, generation, entity);
                Ok(handle)
            }
            Ok(Err(error)) => {
                let _ = world.despawn(entity);
                world.resource_mut::<ConceptBodyIndex>().0.free.push(slot);
                Err(error)
            }
            Err(payload) => {
                let _ = world.despawn(entity);
                world.resource_mut::<ConceptBodyIndex>().0.free.push(slot);
                std::panic::resume_unwind(payload)
            }
        }
    }

    pub fn remove(
        world: &mut World,
        _administration: &LifeAdministration<'_>,
        handle: ConceptBodyHandle,
    ) -> Result<(), FactoryError> {
        if !_administration.matches(world) {
            return Err(FactoryError::InstanceNotFound);
        }
        let live = Self::resolve_entity(world, handle)?;
        if world.get::<WorldRoot>(live).is_some() {
            return Err(FactoryError::ProtectedInstance);
        }
        let entity = world
            .get_resource_mut::<ConceptBodyIndex>()
            .ok_or(FactoryError::InstanceNotFound)?
            .0
            .invalidate(handle.slot(), handle.generation())?;
        crate::admin::cleanup_removed_body(world, handle);
        let _ = world.despawn(entity);
        world.trigger(FactoryInvalidation::ConceptBody(handle));
        world.flush();
        Ok(())
    }

    pub fn is_live(world: &World, handle: ConceptBodyHandle) -> bool {
        Self::entity(world, handle).is_some()
    }

    /// Returns the live factory Handle attached to an ECS entity, if any.
    pub fn handle_for_entity(world: &World, entity: Entity) -> Option<ConceptBodyHandle> {
        let handle = world.get::<ConceptBodyIdentity>(entity)?.0;
        Self::is_live(world, handle).then_some(handle)
    }

    /// Resolves a live Handle for immediate trusted ECS composition work.
    pub fn resolve_entity(
        world: &World,
        handle: ConceptBodyHandle,
    ) -> Result<Entity, FactoryError> {
        Self::entity(world, handle).ok_or(FactoryError::InstanceNotFound)
    }

    pub(crate) fn entity(world: &World, handle: ConceptBodyHandle) -> Option<Entity> {
        let entity = world
            .get_resource::<ConceptBodyIndex>()?
            .0
            .resolve(handle.slot(), handle.generation())?;
        (world.get::<ConceptBodyIdentity>(entity)?.0 == handle).then_some(entity)
    }
}
