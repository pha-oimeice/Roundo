//! Shared storage-domain attributes for Inventory / Items / Economy composition.
//!
//! Inventory owns the one definition of these kinds; callers reuse them instead
//! of declaring competing aliases in each domain. They are not member counts,
//! balance amounts, physical sizes or automatic custody constraints. Declaring
//! or attaching one does not change Inventories, item data or bank rules.

use roundo_life::attributes::{
    AttributeId, AttributeKind, AttributeRegistrationError, AttributeRegistryBuilder,
    AttributeValue,
};

/// Contributes the shared storage-domain definitions without modifying custody,
/// balances, item payloads or any entity's component composition.
pub fn register_attributes(
    registry: &mut AttributeRegistryBuilder,
) -> Result<(), AttributeRegistrationError> {
    registry.register::<Capacity>()?;
    registry.register::<Occupancy>()?;
    Ok(())
}

/// Compile-time identity for capacity (容量).
pub enum CapacityKind {}
impl AttributeKind for CapacityKind {
    const ID: AttributeId = AttributeId::Capacity;
    type Value = i32;
}

/// Signed capacity attribute. The full `i32` range is representable; nonnegative
/// capacity or occupancy limits are gameplay rules, not type-level restrictions.
///
///
pub type Capacity = AttributeValue<CapacityKind>;

/// Compile-time identity for occupancy (占用量).
pub enum OccupancyKind {}
impl AttributeKind for OccupancyKind {
    const ID: AttributeId = AttributeId::Occupancy;
    type Value = i32;
}
/// Signed occupancy attribute; it is not derived automatically from inventory
/// length, item data, weight or an economic balance.
pub type Occupancy = AttributeValue<OccupancyKind>;
