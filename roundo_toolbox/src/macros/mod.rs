#[macro_export]
macro_rules! identifier {
    ($name:ident) => {
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
        pub struct $name(pub u64);
    };
}

#[macro_export]
macro_rules! print_size {
    ($t:ty) => {
        println!("{}: {}", stringify!($t), std::mem::size_of::<$t>());
    };
}

pub use crate::{identifier, print_size};
