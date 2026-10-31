//! Frozen, World-bound registration of native attribute adapters.

use super::{AttributeId, AttributeKind, AttributeValue};
use bevy::{
    ecs::{
        component::{Component, ComponentId, Mutable},
        world::{FilteredEntityMut, FilteredEntityRef, WorldId},
    },
    prelude::{App, Resource, World},
};
use std::{
    any::{Any, TypeId},
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
};

/// A native component explicitly exposed through the central attribute interface.
///
/// `value` must be a pure shared view. `from_value` must validate and construct an
/// independent candidate without changing the World, inputs or shared state.
/// Adapters are trusted native code, not a script sandbox. Whole-component
/// replacement must preserve that domain's attribute semantics; do not register
/// identities, authorization gates or derived projections as writable attributes.
pub trait AttributeComponent: Component<Mutability = Mutable> + Sized {
    const ID: AttributeId;
    type Value: Clone + Send + Sync + 'static;
    fn value(&self) -> &Self::Value;
    fn from_value(value: Self::Value) -> Result<Self, InvalidAttributeValue>;
}

impl<Kind: AttributeKind> AttributeComponent for AttributeValue<Kind>
where
    Kind::Value: Clone,
{
    const ID: AttributeId = Kind::ID;
    type Value = Kind::Value;
    fn value(&self) -> &Self::Value {
        self.get()
    }
    fn from_value(value: Self::Value) -> Result<Self, InvalidAttributeValue> {
        Ok(Self::new(value))
    }
}

/// Module-local adapter for an existing component, including foreign engine
/// types. This avoids orphan-rule workarounds or a duplicate wrapper component.
/// The purity, validation and whole-component replacement contract is the same
/// as [`AttributeComponent`]. Adapter objects are never stored per entity.
pub trait AttributeAdapter: Send + Sync + 'static {
    const ID: AttributeId;
    type Component: Component<Mutability = Mutable>;
    type Value: Clone + Send + Sync + 'static;
    fn value(component: &Self::Component) -> &Self::Value;
    fn from_value(value: Self::Value) -> Result<Self::Component, InvalidAttributeValue>;
}

impl<C: AttributeComponent> AttributeAdapter for C {
    const ID: AttributeId = C::ID;
    type Component = C;
    type Value = C::Value;
    fn value(component: &C) -> &Self::Value {
        component.value()
    }
    fn from_value(value: Self::Value) -> Result<C, InvalidAttributeValue> {
        C::from_value(value)
    }
}

/// A domain adapter rejected a structurally typed candidate value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidAttributeValue;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct Domain {
    world: WorldId,
    epoch: u64,
}

/// Read-only native type information for one registered attribute.
#[derive(Debug)]
pub struct AttributeDescriptor {
    id: AttributeId,
    component_type: TypeId,
    value_type: TypeId,
    component_name: &'static str,
    value_name: &'static str,
}
impl AttributeDescriptor {
    pub const fn id(&self) -> AttributeId {
        self.id
    }
    pub const fn name(&self) -> &'static str {
        self.id.name()
    }
    pub fn component_type_id(&self) -> TypeId {
        self.component_type
    }
    pub fn value_type_id(&self) -> TypeId {
        self.value_type
    }
    pub const fn component_type_name(&self) -> &'static str {
        self.component_name
    }
    pub const fn value_type_name(&self) -> &'static str {
        self.value_name
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttributeRegistrationError {
    DuplicateId,
    DuplicateComponent,
    AlreadyInstalled,
    IdentityExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttributeAccessError {
    WrongWorld,
    WrongRegistry,
    UnknownAttribute,
    EntityNotFound,
    MissingAttribute,
    ValueTypeMismatch,
    InvalidValue,
}

macro_rules! display_error {
    ($name:ty) => {
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{self:?}")
            }
        }
        impl std::error::Error for $name {}
    };
}
display_error!(AttributeRegistrationError);
display_error!(AttributeAccessError);
display_error!(InvalidAttributeValue);

type ReadValue = for<'w, 's> fn(FilteredEntityRef<'w, 's>) -> Option<&'w (dyn Any + Send + Sync)>;
type WriteValue = for<'w, 's> fn(
    &mut FilteredEntityMut<'w, 's>,
    &(dyn Any + Send + Sync),
) -> Result<(), AttributeAccessError>;

struct Definition {
    id: AttributeId,
    component_type: TypeId,
    value_type: TypeId,
    component_name: &'static str,
    value_name: &'static str,
    register: fn(&mut World) -> ComponentId,
    read: ReadValue,
    write: WriteValue,
}

pub(super) struct Entry {
    descriptor: AttributeDescriptor,
    component: ComponentId,
    read: ReadValue,
    write: WriteValue,
}
impl Entry {
    pub(super) fn descriptor(&self) -> &AttributeDescriptor {
        &self.descriptor
    }
    pub(super) fn read<'w, 's>(
        &self,
        entity: FilteredEntityRef<'w, 's>,
    ) -> Option<&'w (dyn Any + Send + Sync)> {
        (self.read)(entity)
    }
    pub(super) fn write(
        &self,
        entity: &mut FilteredEntityMut<'_, '_>,
        value: &(dyn Any + Send + Sync),
    ) -> Result<(), AttributeAccessError> {
        if !entity.contains_id(self.component) {
            return Err(AttributeAccessError::MissingAttribute);
        }
        (self.write)(entity, value)
    }
}

/// Startup-only collector. Registration has no World or instance side effects.
///
/// Each enum ID and native component may appear once. A rejected declaration leaves
/// earlier declarations intact; a module's multi-registration helper is not a
/// transaction. Install only a successfully assembled catalogue. Installation
/// consumes the builder and publishes an immutable registry, not attribute values.
///
///
/// A Component is not exposed merely because it exists in the engine:
#[derive(Default)]
pub struct AttributeRegistryBuilder {
    definitions: Vec<Definition>,
}
impl AttributeRegistryBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Generic adaptation occurs once here, never at each ID-based access call.
    /// Accepts a self-describing AttributeComponent (including AttributeValue<K>)
    /// or a module-local AttributeAdapter for an existing engine component.
    /// The ID is taken from C::ID; callers cannot relabel a definition.
    ///
    pub fn register<C: AttributeAdapter>(&mut self) -> Result<(), AttributeRegistrationError> {
        if self
            .definitions
            .iter()
            .any(|definition| definition.id == C::ID)
        {
            return Err(AttributeRegistrationError::DuplicateId);
        }
        if self
            .definitions
            .iter()
            .any(|definition| definition.component_type == TypeId::of::<C::Component>())
        {
            return Err(AttributeRegistrationError::DuplicateComponent);
        }
        self.definitions.push(Definition {
            id: C::ID,
            component_type: TypeId::of::<C::Component>(),
            value_type: TypeId::of::<C::Value>(),
            component_name: std::any::type_name::<C::Component>(),
            value_name: std::any::type_name::<C::Value>(),
            register: World::register_component::<C::Component>,
            read: read::<C>,
            write: write::<C>,
        });
        Ok(())
    }

    /// Publishes one sealed registry. Refuses replacement before changing World
    /// component metadata. Native component registration panic/OOM is not rolled
    /// back, but no partial AttributeRegistry is published.
    pub fn install(self, app: &mut App) -> Result<(), AttributeRegistrationError> {
        let world = app.world_mut();
        if world.contains_resource::<AttributeRegistry>() {
            return Err(AttributeRegistrationError::AlreadyInstalled);
        }
        static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);
        let epoch = NEXT_EPOCH
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                next.checked_add(1)
            })
            .map_err(|_| AttributeRegistrationError::IdentityExhausted)?;
        let domain = Domain {
            world: world.id(),
            epoch,
        };
        let mut entries = BTreeMap::new();
        for definition in self.definitions {
            let id = definition.id;
            let component = (definition.register)(world);
            entries.insert(
                id,
                Entry {
                    component,
                    descriptor: AttributeDescriptor {
                        id,
                        component_type: definition.component_type,
                        value_type: definition.value_type,
                        component_name: definition.component_name,
                        value_name: definition.value_name,
                    },
                    read: definition.read,
                    write: definition.write,
                },
            );
        }
        world.insert_resource(AttributeRegistry { domain, entries });
        Ok(())
    }
}

/// Immutable metadata and adapters only. Native component columns are the sole
/// value store; there is no shadow map, payload lock or mutable registry getter.
#[derive(Resource)]
pub struct AttributeRegistry {
    domain: Domain,
    entries: BTreeMap<AttributeId, Entry>,
}
impl AttributeRegistry {
    pub fn describe(&self, id: AttributeId) -> Result<&AttributeDescriptor, AttributeAccessError> {
        Ok(&self.entry(id)?.descriptor)
    }
    /// Enum ID order, independent of module registration order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &AttributeDescriptor> {
        self.entries.values().map(|entry| &entry.descriptor)
    }
    pub(super) fn components(&self) -> impl Iterator<Item = ComponentId> + '_ {
        self.entries.values().map(|entry| entry.component)
    }
    pub(super) fn initialized_domain(&self, world: WorldId) -> Domain {
        assert_eq!(
            self.domain.world, world,
            "AttributeRegistry belongs to another World"
        );
        self.domain
    }
    pub(super) fn checked_entry(
        &self,
        world: WorldId,
        expected_domain: Domain,
        id: AttributeId,
    ) -> Result<&Entry, AttributeAccessError> {
        if self.domain.world != world {
            return Err(AttributeAccessError::WrongWorld);
        }
        if self.domain != expected_domain {
            return Err(AttributeAccessError::WrongRegistry);
        }
        self.entry(id)
    }
    fn entry(&self, id: AttributeId) -> Result<&Entry, AttributeAccessError> {
        self.entries
            .get(&id)
            .ok_or(AttributeAccessError::UnknownAttribute)
    }
}

fn read<'w, 's, C: AttributeAdapter>(
    entity: FilteredEntityRef<'w, 's>,
) -> Option<&'w (dyn Any + Send + Sync)> {
    Some(C::value(entity.get::<C::Component>()?))
}
fn write<C: AttributeAdapter>(
    entity: &mut FilteredEntityMut<'_, '_>,
    value: &(dyn Any + Send + Sync),
) -> Result<(), AttributeAccessError> {
    let value = value
        .downcast_ref::<C::Value>()
        .ok_or(AttributeAccessError::ValueTypeMismatch)?;
    let candidate = C::from_value(value.clone()).map_err(|_| AttributeAccessError::InvalidValue)?;
    let mut current = entity
        .get_mut::<C::Component>()
        .ok_or(AttributeAccessError::MissingAttribute)?;
    *current = candidate;
    Ok(())
}
