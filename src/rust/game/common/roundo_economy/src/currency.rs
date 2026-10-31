use crate::Quantity;

#[doc(hidden)]
pub mod __private {
    pub trait Sealed {}
    pub trait RuleSealed {}
}

/// Exact source-declared key, unique within a catalogue.
///
/// No naming convention or nonempty-string requirement is imposed.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CurrencyKey(&'static str);

impl CurrencyKey {
    #[doc(hidden)]
    pub const fn from_source(value: &'static str) -> Self {
        Self(value)
    }

    /// Returns the exact source string without normalization.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// Source-declared currency with exactly one quantity model and key.
///
/// The hidden declaration marker is not a sandbox against host-written code.
/// No concrete currency, issuance policy or spending authority is provided.
pub trait Currency: __private::Sealed + Send + Sync + 'static {
    type Quantity: Quantity;
    const KEY: CurrencyKey;
}

/// Declares one currency's key and associated quantity model.
///
/// Repeating a declaration for the same type produces conflicting impls.
#[macro_export]
macro_rules! declare_currency {
    ($type:ty, $key:literal, $quantity:ty) => {
        impl $crate::__private::Sealed for $type {}
        impl $crate::Currency for $type {
            type Quantity = $quantity;
            const KEY: $crate::CurrencyKey = $crate::CurrencyKey::from_source($key);
        }
    };
}
