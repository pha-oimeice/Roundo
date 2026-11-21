use std::fmt;

mod sealed {
    pub trait Shape {}
}

#[doc(hidden)]
pub mod __private {
    pub trait Sealed {}
}

/// Marker for the two item data categories accepted by this crate.
pub trait ItemDataShape: sealed::Shape + Send + Sync + 'static {}

/// The single per-item type payload category.
#[derive(Debug)]
pub enum ItemTypeShape {}
impl sealed::Shape for ItemTypeShape {}
impl ItemDataShape for ItemTypeShape {}

/// The multi-source per-item specialized payload category.
#[derive(Debug)]
pub enum SpecializedShape {}
impl sealed::Shape for SpecializedShape {}
impl ItemDataShape for SpecializedShape {}

/// A source-declared exact item data key.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ItemDataKey(&'static str);

impl ItemDataKey {
    #[doc(hidden)]
    pub const fn from_source(value: &'static str) -> Self {
        Self(value)
    }

    /// Returns the exact source-declared string.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for ItemDataKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

/// A source-declared payload type with one compile-time shape and one exact key.
pub trait ItemData: __private::Sealed + Send + Sync + 'static {
    type Shape: ItemDataShape;
    const KEY: ItemDataKey;

    /// Source-defined validity check performed immediately before storing.
    fn validate(&self) -> bool;
}

/// Payloads for the single item type slot.
pub trait ItemTypeData: ItemData<Shape = ItemTypeShape> {}
impl<T: ItemData<Shape = ItemTypeShape>> ItemTypeData for T {}

/// Payloads for independent specialized slots.
pub trait SpecializedData: ItemData<Shape = SpecializedShape> {}
impl<T: ItemData<Shape = SpecializedShape>> SpecializedData for T {}

#[macro_export]
macro_rules! declare_item_type_data {
    ($type:ty, $key:literal, $validator:expr) => {
        impl $crate::__private::Sealed for $type {}
        impl $crate::ItemData for $type {
            type Shape = $crate::ItemTypeShape;
            const KEY: $crate::ItemDataKey = $crate::ItemDataKey::from_source($key);

            fn validate(&self) -> bool {
                let validate: fn(&$type) -> bool = $validator;
                validate(self)
            }
        }
    };
}

#[macro_export]
macro_rules! declare_specialized_data {
    ($type:ty, $key:literal, $validator:expr) => {
        impl $crate::__private::Sealed for $type {}
        impl $crate::ItemData for $type {
            type Shape = $crate::SpecializedShape;
            const KEY: $crate::ItemDataKey = $crate::ItemDataKey::from_source($key);

            fn validate(&self) -> bool {
                let validate: fn(&$type) -> bool = $validator;
                validate(self)
            }
        }
    };
}
