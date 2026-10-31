use bevy::prelude::{Resource, World};
use std::{any::TypeId, collections::HashMap, fmt};

mod sealed {
    pub trait Shape {}
}

#[doc(hidden)]
pub mod __private {
    pub trait Sealed {}
}

pub trait DataShape: sealed::Shape + Send + Sync + 'static {}

#[derive(Debug)]
pub enum EventShape {}
impl sealed::Shape for EventShape {}
impl DataShape for EventShape {}

#[derive(Debug)]
pub enum StateShape {}
impl sealed::Shape for StateShape {}
impl DataShape for StateShape {}

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LifeDataTypeKey(&'static str);

impl LifeDataTypeKey {
    /// Stable source-declared name, not a registration or access capability.
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    #[doc(hidden)]
    pub const fn from_source(value: &'static str) -> Self {
        Self(value)
    }
}

impl fmt::Debug for LifeDataTypeKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("LifeDataTypeKey")
            .field(&self.0)
            .finish()
    }
}

pub trait LifeData: __private::Sealed + Send + Sync + 'static {
    type Shape: DataShape;
    const KEY: LifeDataTypeKey;
}

pub trait EventData: LifeData<Shape = EventShape> {
    fn retained_size(&self) -> usize;
    /// Source-defined payload validation performed before an Event is accepted.
    fn validate(&self) -> bool {
        true
    }
}

pub trait StateData: LifeData<Shape = StateShape> {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeRegistrationError {
    DuplicateKey,
    TypeAlreadyRegistered,
    CatalogueClosed,
}

#[derive(Resource, Default)]
struct LifeDataTypeRegistry {
    by_key: HashMap<LifeDataTypeKey, TypeId>,
    by_type: HashMap<TypeId, LifeDataTypeKey>,
    closed: bool,
}

pub(crate) fn register_type<T: LifeData>(world: &mut World) -> Result<(), TypeRegistrationError> {
    world.init_resource::<LifeDataTypeRegistry>();
    let type_id = TypeId::of::<T>();
    let mut registry = world.resource_mut::<LifeDataTypeRegistry>();
    if registry.closed {
        return Err(TypeRegistrationError::CatalogueClosed);
    }
    if registry.by_type.contains_key(&type_id) {
        return Err(TypeRegistrationError::TypeAlreadyRegistered);
    }
    if registry.by_key.contains_key(&T::KEY) {
        return Err(TypeRegistrationError::DuplicateKey);
    }
    registry.by_key.insert(T::KEY, type_id);
    registry.by_type.insert(type_id, T::KEY);
    Ok(())
}

pub(crate) fn catalogue_is_closed(world: &World) -> bool {
    world
        .get_resource::<LifeDataTypeRegistry>()
        .is_some_and(|registry| registry.closed)
}

pub(crate) fn type_is_registered<T: LifeData>(world: &World) -> bool {
    world
        .get_resource::<LifeDataTypeRegistry>()
        .is_some_and(|registry| {
            registry.closed && registry.by_type.contains_key(&TypeId::of::<T>())
        })
}

pub fn close_life_data_catalogue(
    app: &mut bevy::prelude::App,
) -> Result<(), TypeRegistrationError> {
    app.world_mut().init_resource::<LifeDataTypeRegistry>();
    let mut registry = app.world_mut().resource_mut::<LifeDataTypeRegistry>();
    if registry.closed {
        return Ok(());
    }
    registry.closed = true;
    Ok(())
}

#[macro_export]
macro_rules! declare_event_data {
    ($type:ty, $key:literal, $retained_size:expr) => {
        impl $crate::__private::Sealed for $type {}
        impl $crate::LifeData for $type {
            type Shape = $crate::EventShape;
            const KEY: $crate::LifeDataTypeKey = $crate::LifeDataTypeKey::from_source($key);
        }
        impl $crate::EventData for $type {
            fn retained_size(&self) -> usize {
                let retained_size: fn(&$type) -> usize = $retained_size;
                retained_size(self)
            }
        }
    };
}

#[macro_export]
macro_rules! declare_state_data {
    ($type:ty, $key:literal) => {
        impl $crate::__private::Sealed for $type {}
        impl $crate::LifeData for $type {
            type Shape = $crate::StateShape;
            const KEY: $crate::LifeDataTypeKey = $crate::LifeDataTypeKey::from_source($key);
        }
        impl $crate::StateData for $type {}
    };
}
