//! The single native type payload shared by all Mod item definitions.

use roundo_mod_loader::ResourceName;

/// Owned definition identity and data; neither constitutes an item ID or grant.
///
/// Only known definitions can be materialized. Copies and replacement values
/// are independent data, not publication or permission to modify runtime storage.
#[derive(Clone, Debug, PartialEq)]
pub struct DeclaredItemData {
    definition: ResourceName,
    data: toml::Table,
}

impl DeclaredItemData {
    pub(crate) fn from_definition(definition: ResourceName, data: toml::Table) -> Self {
        Self { definition, data }
    }

    /// Returns the canonical initial-value definition, not an instance identity.
    pub fn definition(&self) -> &ResourceName {
        &self.definition
    }

    /// Reads the owned data without exposing mutable access.
    pub fn data(&self) -> &toml::Table {
        &self.data
    }

    /// Returns an independent value with this definition and replacement data.
    /// Does not mutate this value, runtime storage, identity, custody or grants.
    pub fn with_replaced_data(&self, data: toml::Table) -> Self {
        Self {
            definition: self.definition.clone(),
            data,
        }
    }
}

crate::declare_item_type_data!(
    DeclaredItemData,
    "roundo.items.declared.v1",
    |_: &DeclaredItemData| true
);
