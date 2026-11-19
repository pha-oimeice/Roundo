//! Compile-time native attribute names. Domain implementations depend on this
//! enum; this module never depends on their ECS components or value stores.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
use std::fmt;

/// The shared native attribute ID itself, not a registry-allocated handle.
///
/// IDs are independent of World and registration order. A variant's existence
/// does not install an implementation or grant access. Existing numeric values
/// must not be reassigned; registration/World identity is a separate concern.
///
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum AttributeId {
    Health = 1,
    IntentValue = 2,
    ExecutionValue = 3,
    RecordValue = 4,
    ContinuityRate = 5,
    ChangeRate = 6,
    CycleRate = 7,
    EternityRate = 8,
    Capacity = 9,
    Occupancy = 10,
}

impl AttributeId {
    pub const ALL: [Self; 10] = [
        Self::Health,
        Self::IntentValue,
        Self::ExecutionValue,
        Self::RecordValue,
        Self::ContinuityRate,
        Self::ChangeRate,
        Self::CycleRate,
        Self::EternityRate,
        Self::Capacity,
        Self::Occupancy,
    ];

    /// Native identifier name, not a localized label or independently managed key.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Health => "health",
            Self::IntentValue => "intent_value",
            Self::ExecutionValue => "execution_value",
            Self::RecordValue => "record_value",
            Self::ContinuityRate => "continuity_rate",
            Self::ChangeRate => "change_rate",
            Self::CycleRate => "cycle_rate",
            Self::EternityRate => "eternity_rate",
            Self::Capacity => "capacity",
            Self::Occupancy => "occupancy",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidAttributeId(pub u8);

impl TryFrom<u8> for AttributeId {
    type Error = InvalidAttributeId;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Health),
            2 => Ok(Self::IntentValue),
            3 => Ok(Self::ExecutionValue),
            4 => Ok(Self::RecordValue),
            5 => Ok(Self::ContinuityRate),
            6 => Ok(Self::ChangeRate),
            7 => Ok(Self::CycleRate),
            8 => Ok(Self::EternityRate),
            9 => Ok(Self::Capacity),
            10 => Ok(Self::Occupancy),
            _ => Err(InvalidAttributeId(value)),
        }
    }
}

impl fmt::Display for AttributeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
impl fmt::Display for InvalidAttributeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown attribute id {}", self.0)
    }
}
impl std::error::Error for InvalidAttributeId {}

// Serialize the explicit ID, not serde's declaration-order enum ordinal.
impl Serialize for AttributeId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(*self as u8)
    }
}
impl<'de> Deserialize<'de> for AttributeId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::try_from(u8::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}
