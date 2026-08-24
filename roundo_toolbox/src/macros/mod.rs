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

#[macro_export]
macro_rules! print_size {
    ($t:ty) => {
        println!("{}: {}", stringify!($t), std::mem::size_of::<$t>());
    };
}

pub use crate::{identifier, print_size};
