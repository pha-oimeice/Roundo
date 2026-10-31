//! Pure runtime preparation, preserving the host's existing catalogue.

use crate::{
    DeclaredItemData, ItemDataBuilder, ItemDataError, ItemDataStore, ItemDefinitions, ItemSchema,
};

/// Consumed preparation inputs for a future runtime installation.
///
/// The host first registers its own Type/Specialized sources and retains their
/// schemas, then passes that same builder here. This appends only the native
/// declared source and freezes an empty store; it does not install ECS resources,
/// publish items or grant access. Pre-registering `DeclaredItemData` is an error.
/// Failure consumes the inputs without affecting any App. No populated store
/// can be imported, and external callers cannot retrieve the internal store.
pub struct ItemRuntimeSetup {
    definitions: ItemDefinitions,
    store: ItemDataStore,
    declared_schema: ItemSchema<DeclaredItemData>,
}

impl ItemRuntimeSetup {
    pub(crate) fn into_parts(
        self,
    ) -> (ItemDefinitions, ItemDataStore, ItemSchema<DeclaredItemData>) {
        (self.definitions, self.store, self.declared_schema)
    }

    /// Appends the native source to the host catalogue, then freezes on success.
    ///
    /// Duplicate type/key errors are returned unchanged from registration.
    pub fn try_new(
        definitions: ItemDefinitions,
        mut builder: ItemDataBuilder,
    ) -> Result<Self, ItemDataError> {
        let declared_schema = builder.register::<DeclaredItemData>()?;
        Ok(Self {
            definitions,
            store: builder.freeze(),
            declared_schema,
        })
    }
}
