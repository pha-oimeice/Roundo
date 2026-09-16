/// Defines a transparent, serializable identifier newtype.
#[macro_export]
macro_rules! identifier {
    ($name:ident) => {
        $crate::identifier!($name, u64);
    };
    ($name:ident, $value:ty) => {
        #[derive(
            Clone,
            Copy,
            Debug,
            Default,
            Deserialize,
            Eq,
            Hash,
            Ord,
            PartialEq,
            PartialOrd,
            Serialize,
        )]
        #[repr(transparent)]
        pub struct $name(pub $value);
    };
}

/// Prints a type's in-memory size for development diagnostics.
#[macro_export]
macro_rules! print_size {
    ($t:ty) => {
        println!("{}: {}", stringify!($t), std::mem::size_of::<$t>());
    };
}

// Re-export macros through the module path as well as the crate root.
pub use crate::{identifier, print_size};
