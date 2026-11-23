use crate::{
    FrozenCatalogue, ItemDataError, ItemDataKey, ItemSchema, ItemTypeData, SpecializedData,
};
use roundo_inventory::ItemId;
use std::{any::Any, collections::HashMap};

/// Frozen typed item data store associated with globally meaningful item IDs.
pub struct ItemDataStore {
    catalogue: FrozenCatalogue,
    types: HashMap<ItemId, TypeEntry>,
    specialized: HashMap<ItemId, HashMap<std::any::TypeId, Box<dyn Any + Send + Sync>>>,
}

impl ItemDataStore {
    pub(crate) fn from_frozen(catalogue: FrozenCatalogue) -> Self {
        Self {
            catalogue,
            types: HashMap::new(),
            specialized: HashMap::new(),
        }
    }

    /// Detaches every payload before running any caller-owned payload destructor.
    /// Publication/lifecycle validation belongs to the runtime adapter.
    pub(crate) fn remove_item(&mut self, item: ItemId) {
        let type_data = self.types.remove(&item);
        let specialized = self.specialized.remove(&item);
        drop((type_data, specialized));
    }

    /// Stores or replaces the single type payload for an item ID.
    pub fn set_type<T: ItemTypeData>(
        &mut self,
        schema: ItemSchema<T>,
        item: ItemId,
        value: T,
    ) -> Result<(), ItemDataError> {
        let record = self.catalogue.validate_schema(schema)?;
        if !value.validate() {
            return Err(ItemDataError::InvalidPayload);
        }
        self.types.insert(
            item,
            TypeEntry {
                type_id: record.type_id,
                key: record.key,
                payload: Box::new(value),
            },
        );
        Ok(())
    }

    /// Synchronously reads the current type payload when it has source `T`.
    pub fn with_type<T: ItemTypeData, R>(
        &self,
        schema: ItemSchema<T>,
        item: ItemId,
        read: impl for<'data> FnOnce(&'data T) -> R,
    ) -> Result<Option<R>, ItemDataError> {
        let record = self.catalogue.validate_schema(schema)?;
        let Some(entry) = self.types.get(&item) else {
            return Ok(None);
        };
        if entry.type_id != record.type_id {
            return Err(ItemDataError::ItemTypeMismatch);
        }
        let value = entry
            .payload
            .downcast_ref::<T>()
            .expect("registered item type payload must downcast to schema type");
        Ok(Some(read(value)))
    }

    /// Returns the current type key for an item ID, if a type payload exists.
    pub fn type_key(&self, item: ItemId) -> Option<ItemDataKey> {
        self.types.get(&item).map(|entry| entry.key)
    }

    /// Stores or replaces one specialized payload source for an item ID.
    pub fn set_specialized<T: SpecializedData>(
        &mut self,
        schema: ItemSchema<T>,
        item: ItemId,
        value: T,
    ) -> Result<(), ItemDataError> {
        let record = self.catalogue.validate_schema(schema)?;
        if !value.validate() {
            return Err(ItemDataError::InvalidPayload);
        }
        self.specialized
            .entry(item)
            .or_default()
            .insert(record.type_id, Box::new(value));
        Ok(())
    }

    /// Removes only this registered source; absence is unchanged and returns false.
    pub fn remove_specialized<T: SpecializedData>(
        &mut self,
        schema: ItemSchema<T>,
        item: ItemId,
    ) -> Result<bool, ItemDataError> {
        let record = self.catalogue.validate_schema(schema)?;
        let Some(bucket) = self.specialized.get_mut(&item) else {
            return Ok(false);
        };
        let removed = bucket.remove(&record.type_id).is_some();
        if bucket.is_empty() {
            self.specialized.remove(&item);
        }
        Ok(removed)
    }

    /// Synchronously reads one specialized payload source for an item ID.
    pub fn with_specialized<T: SpecializedData, R>(
        &self,
        schema: ItemSchema<T>,
        item: ItemId,
        read: impl for<'data> FnOnce(&'data T) -> R,
    ) -> Result<Option<R>, ItemDataError> {
        let record = self.catalogue.validate_schema(schema)?;
        let Some(value) = self
            .specialized
            .get(&item)
            .and_then(|bucket| bucket.get(&record.type_id))
        else {
            return Ok(None);
        };
        let value = value
            .downcast_ref::<T>()
            .expect("registered specialized payload must downcast to schema type");
        Ok(Some(read(value)))
    }
}

struct TypeEntry {
    type_id: std::any::TypeId,
    key: ItemDataKey,
    payload: Box<dyn Any + Send + Sync>,
}
