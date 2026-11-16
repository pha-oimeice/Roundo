use std::sync::Arc;

use roundo_dsa::data_structure::resource_access::{ReadAccess, Shared};

use crate::{AssetError, AssetInstanceId, AssetPrototype};

/// Non-owning read grant for one exact loaded prototype.
///
/// Clone redistributes the same grant and shares its revocation. A separate
/// manager load/handle request issues an independent grant. Handles cannot keep
/// an unloaded prototype alive or resolve a later load of the same name.
#[derive(Clone, Debug)]
pub struct AssetHandle {
    pub(crate) access: ReadAccess<Arc<AssetPrototype>, Shared>,
}

impl AssetHandle {
    /// Exact DSA manager-domain and instance identity, not an access capability.
    pub fn instance_id(&self) -> &AssetInstanceId {
        self.access.resource_id()
    }

    /// Instantaneous grant validity. Actual reads still go through the manager.
    pub fn check(&self) -> Result<(), AssetError> {
        self.access.check().map_err(Into::into)
    }
}
