//! Static attribute specialization and its normalized scalar value.

use super::AttributeId;
use bevy::prelude::Component;
use std::{fmt, marker::PhantomData};

/// Compile-time attribute definition: a shared enum ID and one native value type.
/// New native names must be declared in AttributeId before implementing a kind.
///
/// Markers do not occupy runtime storage. Implementing this trait declares a
/// native type; it does not register Life data, authorize writes or install values.
pub trait AttributeKind: Send + Sync + 'static {
    const ID: AttributeId;
    type Value: Send + Sync + 'static;
}

/// One native attribute value specialized by a distinct marker type.
///
/// The payload has the same size/alignment as `Kind::Value`. There is no dynamic
/// lookup, implicit conversion between kinds, arithmetic or default initialization.
/// ECS writes still require the trusted host's ordinary access policy.
///
///
/// Same underlying integer does not make two attributes interchangeable:
///
/// An attribute's underlying value type cannot be changed by its caller:
#[repr(transparent)]
#[derive(Component)]
pub struct AttributeValue<Kind: AttributeKind> {
    value: Kind::Value,
    kind: PhantomData<fn() -> Kind>,
}

impl<Kind: AttributeKind> AttributeValue<Kind> {
    pub const ID: AttributeId = Kind::ID;

    pub const fn new(value: Kind::Value) -> Self {
        Self {
            value,
            kind: PhantomData,
        }
    }

    pub const fn get(&self) -> &Kind::Value {
        &self.value
    }

    /// Replaces one value of the same attribute kind. No domain rule, observer,
    /// resource registration or cross-attribute transaction is implied.
    pub fn set(&mut self, value: Kind::Value) {
        self.value = value;
    }

    pub fn into_inner(self) -> Kind::Value {
        self.value
    }
}

impl<Kind: AttributeKind> Clone for AttributeValue<Kind>
where
    Kind::Value: Clone,
{
    fn clone(&self) -> Self {
        Self::new(self.value.clone())
    }
}

impl<Kind: AttributeKind> Copy for AttributeValue<Kind> where Kind::Value: Copy {}

impl<Kind: AttributeKind> fmt::Debug for AttributeValue<Kind>
where
    Kind::Value: fmt::Debug,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple(std::any::type_name::<Kind>())
            .field(&self.value)
            .finish()
    }
}

impl<Kind: AttributeKind> PartialEq for AttributeValue<Kind>
where
    Kind::Value: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<Kind: AttributeKind> Eq for AttributeValue<Kind> where Kind::Value: Eq {}

impl<Kind: AttributeKind> PartialOrd for AttributeValue<Kind>
where
    Kind::Value: PartialOrd,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.value.partial_cmp(&other.value)
    }
}

impl<Kind: AttributeKind> Ord for AttributeValue<Kind>
where
    Kind::Value: Ord,
{
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.value.cmp(&other.value)
    }
}

/// A finite normalized scalar in the closed interval `[0, 1]`, stored as `f32`.
///
/// Construction rejects invalid input rather than clamping, saturating or
/// quantizing it. The raw field is private; no mutable raw-float access is exposed.
/// This is not a fixed-width GPU UNORM encoding or a probability/behavior rule.
///
///
/// The normalized scalar's raw constructor is not a validation bypass:
///
/// A normalized attribute cannot be constructed from an unchecked float:
///
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Unorm(f32);

impl Unorm {
    pub const ZERO: Self = Self(0.0);
    pub const ONE: Self = Self(1.0);

    pub fn new(value: f32) -> Result<Self, InvalidUnorm> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(InvalidUnorm);
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> f32 {
        self.0
    }
}

impl TryFrom<f32> for Unorm {
    type Error = InvalidUnorm;

    fn try_from(value: f32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Unorm> for f32 {
    fn from(value: Unorm) -> Self {
        value.get()
    }
}

/// A non-finite value or a value outside `[0, 1]` was supplied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidUnorm;

impl fmt::Display for InvalidUnorm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("unorm requires a finite value in [0, 1]")
    }
}

impl std::error::Error for InvalidUnorm {}
