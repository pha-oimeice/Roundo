use crate::{ItemData, ItemDataKey};
use std::{
    any::TypeId,
    collections::HashMap,
    fmt,
    marker::PhantomData,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_CATALOGUE_STAMP: AtomicU64 = AtomicU64::new(1);

/// Recoverable item data catalogue/store errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemDataError {
    IdentityExhausted,
    TypeAlreadyRegistered,
    DuplicateKey,
    WrongCatalogue,
    ItemTypeMismatch,
    InvalidPayload,
}

impl fmt::Display for ItemDataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::IdentityExhausted => "item data catalogue identity space is exhausted",
            Self::TypeAlreadyRegistered => "item data source type is already registered",
            Self::DuplicateKey => "item data key is already registered",
            Self::WrongCatalogue => "item data schema belongs to another catalogue",
            Self::ItemTypeMismatch => "item has a different registered type payload",
            Self::InvalidPayload => "item data payload failed source validation",
        })
    }
}

impl std::error::Error for ItemDataError {}

/// Open typed item data catalogue builder.
pub struct ItemDataBuilder {
    pub(crate) stamp: u64,
    registry: Registry,
}

impl ItemDataBuilder {
    /// Creates a fresh open catalogue with a never-reused private stamp.
    pub fn try_new() -> Result<Self, ItemDataError> {
        Ok(Self {
            stamp: allocate_catalogue_stamp(&NEXT_CATALOGUE_STAMP)?,
            registry: Registry::default(),
        })
    }

    /// Registers one source type and mints a schema bound to this catalogue.
    pub fn register<T: ItemData>(&mut self) -> Result<ItemSchema<T>, ItemDataError> {
        let type_id = TypeId::of::<T>();
        if self.registry.by_type.contains_key(&type_id) {
            return Err(ItemDataError::TypeAlreadyRegistered);
        }
        if self.registry.by_key.contains_key(&T::KEY) {
            return Err(ItemDataError::DuplicateKey);
        }
        let record = RegistrationRecord::of::<T>();
        self.registry.by_key.insert(T::KEY, type_id);
        self.registry.by_type.insert(type_id, record);
        Ok(ItemSchema {
            stamp: self.stamp,
            marker: PhantomData,
        })
    }

    /// Freezes registration and creates an empty data store.
    pub fn freeze(self) -> crate::ItemDataStore {
        crate::ItemDataStore::from_frozen(FrozenCatalogue {
            stamp: self.stamp,
            registry: self.registry,
        })
    }
}

/// Copyable proof that source `T` was registered in exactly one catalogue.
pub struct ItemSchema<T: ItemData> {
    pub(crate) stamp: u64,
    marker: PhantomData<fn() -> T>,
}

impl<T: ItemData> ItemSchema<T> {
    /// Returns the exact source-declared key for this schema.
    pub fn key(self) -> ItemDataKey {
        T::KEY
    }
}

impl<T: ItemData> Clone for ItemSchema<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ItemData> Copy for ItemSchema<T> {}

impl<T: ItemData> fmt::Debug for ItemSchema<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ItemSchema")
            .field("key", &T::KEY)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(crate) struct FrozenCatalogue {
    pub(crate) stamp: u64,
    registry: Registry,
}

impl FrozenCatalogue {
    pub(crate) fn validate_schema<T: ItemData>(
        &self,
        schema: ItemSchema<T>,
    ) -> Result<RegistrationRecord, ItemDataError> {
        if schema.stamp != self.stamp {
            return Err(ItemDataError::WrongCatalogue);
        }
        let record = *self
            .registry
            .by_type
            .get(&TypeId::of::<T>())
            .expect("schema stamp matched but source type is not registered");
        assert_eq!(record.key, T::KEY, "registered key changed for source type");
        assert_eq!(
            record.shape,
            TypeId::of::<T::Shape>(),
            "registered shape changed for source type"
        );
        Ok(record)
    }
}

#[derive(Debug, Default)]
pub(crate) struct Registry {
    by_key: HashMap<ItemDataKey, TypeId>,
    by_type: HashMap<TypeId, RegistrationRecord>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RegistrationRecord {
    pub(crate) key: ItemDataKey,
    pub(crate) type_id: TypeId,
    pub(crate) shape: TypeId,
}

impl RegistrationRecord {
    fn of<T: ItemData>() -> Self {
        Self {
            key: T::KEY,
            type_id: TypeId::of::<T>(),
            shape: TypeId::of::<T::Shape>(),
        }
    }
}

fn allocate_catalogue_stamp(counter: &AtomicU64) -> Result<u64, ItemDataError> {
    counter
        .try_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
            if current == u64::MAX {
                None
            } else {
                Some(current + 1)
            }
        })
        .map_err(|_| ItemDataError::IdentityExhausted)
}
