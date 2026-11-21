//! Network-data capability supplied by derive, without a registration catalogue.

/// Data opted into network serialization by `#[derive(DataTransferObject)]`.
/// Component/resource interfaces combine this marker with Bevy's trait bounds.
/// Typed receive interfaces also require owned deserialization.
pub trait DataTransferObject: serde::Serialize {
    /// Automatically available type name, independent of the transient wire ID.
    fn name(&self) -> &str {
        core::any::type_name::<Self>()
    }
}
