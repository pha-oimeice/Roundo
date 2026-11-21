//! Native scalar attributes owned by Life's foundational architecture.
//!
//! These are independent typed values, not Life Event/State schemas or Factory
//! identity. Merely declaring them installs no values, readers, writers, control
//! or gameplay rules. Signed attributes retain the entire `i32` range.

mod access;
mod registry;
mod value;
pub use access::{AttributeReads, AttributeRef, AttributeWrites};
pub use registry::{
    AttributeAccessError, AttributeAdapter, AttributeComponent, AttributeDescriptor,
    AttributeRegistrationError, AttributeRegistry, AttributeRegistryBuilder, InvalidAttributeValue,
};
pub use roundo_contracts::AttributeId;
pub use value::{AttributeKind, AttributeValue, InvalidUnorm, Unorm};

/// Contributes this module's eight definitions to a startup registry. This does
/// not create values or modify Life's separate Event/State catalogue.
pub fn register_attributes(
    registry: &mut AttributeRegistryBuilder,
) -> Result<(), AttributeRegistrationError> {
    registry.register::<Health>()?;
    registry.register::<IntentValue>()?;
    registry.register::<ExecutionValue>()?;
    registry.register::<RecordValue>()?;
    registry.register::<ContinuityRate>()?;
    registry.register::<ChangeRate>()?;
    registry.register::<CycleRate>()?;
    registry.register::<EternityRate>()?;
    Ok(())
}

/// Compile-time identity for the health attribute (生命值).
pub enum HealthKind {}
impl AttributeKind for HealthKind {
    const ID: AttributeId = AttributeId::Health;
    type Value = i32;
}
/// Health value; zero and negative values have no built-in death semantics.
pub type Health = AttributeValue<HealthKind>;

/// Compile-time identity for the intent value attribute (意图值).
pub enum IntentValueKind {}
impl AttributeKind for IntentValueKind {
    const ID: AttributeId = AttributeId::IntentValue;
    type Value = i32;
}
/// Intent value, not a Command, Controller or authority amount.
pub type IntentValue = AttributeValue<IntentValueKind>;

/// Compile-time identity for the execution value attribute (执行值).
pub enum ExecutionValueKind {}
impl AttributeKind for ExecutionValueKind {
    const ID: AttributeId = AttributeId::ExecutionValue;
    type Value = i32;
}
/// Execution value, not a motion commit or permission to execute.
pub type ExecutionValue = AttributeValue<ExecutionValueKind>;

/// Compile-time identity for the record value attribute (记录值).
pub enum RecordValueKind {}
impl AttributeKind for RecordValueKind {
    const ID: AttributeId = AttributeId::RecordValue;
    type Value = i32;
}
/// Record value, not a Recorder policy or a retained Record payload.
pub type RecordValue = AttributeValue<RecordValueKind>;

/// Compile-time identity for the continuity rate attribute (连续率).
pub enum ContinuityRateKind {}
impl AttributeKind for ContinuityRateKind {
    const ID: AttributeId = AttributeId::ContinuityRate;
    type Value = Unorm;
}
/// Continuity rate; no update, retention or event-lifetime rule is implied.
pub type ContinuityRate = AttributeValue<ContinuityRateKind>;

/// Compile-time identity for the change rate attribute (变化率).
pub enum ChangeRateKind {}
impl AttributeKind for ChangeRateKind {
    const ID: AttributeId = AttributeId::ChangeRate;
    type Value = Unorm;
}
/// Change rate; no interpolation, mutation or time-derivative rule is implied.
pub type ChangeRate = AttributeValue<ChangeRateKind>;

/// Compile-time identity for the cycle rate attribute (周期率).
pub enum CycleRateKind {}
impl AttributeKind for CycleRateKind {
    const ID: AttributeId = AttributeId::CycleRate;
    type Value = Unorm;
}
/// Cycle rate; no scheduler period or clock interpretation is implied.
pub type CycleRate = AttributeValue<CycleRateKind>;

/// Compile-time identity for the eternity rate attribute (永恒率).
pub enum EternityRateKind {}
impl AttributeKind for EternityRateKind {
    const ID: AttributeId = AttributeId::EternityRate;
    type Value = Unorm;
}
/// Eternity rate; no persistence or immortality rule is implied.
pub type EternityRate = AttributeValue<EternityRateKind>;
