//! Storage modes share the same ownership, identity and grant implementation.

use std::{
    any::Any,
    cell::RefCell,
    collections::HashMap,
    marker::PhantomData,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// Original single-threaded storage mode; accepts arbitrary `'static` payloads.
#[derive(Default)]
pub struct Local(PhantomData<Rc<()>>);

/// Thread-safe storage mode for `Send + Sync` payloads, including ECS data.
/// Different threads may read concurrently. Same-thread recursive protection
/// rejects instead of waiting; scopes themselves remain thread-confined.
#[derive(Default)]
pub struct Shared;

/// Sealed choice of directory storage, not a caller-provided permission policy.
pub trait Storage: private::Backend {}
impl Storage for Local {}
impl Storage for Shared {}

pub(super) mod private {
    use super::*;

    pub trait Backend {
        type Erased;
        type Registry: Default;
        fn directory<R>(
            registry: &Self::Registry,
            operation: impl FnOnce(&mut HashMap<u64, Self::Erased>) -> R,
        ) -> R;
    }

    impl Backend for Local {
        type Erased = Arc<dyn Any>;
        type Registry = RefCell<HashMap<u64, Self::Erased>>;
        fn directory<R>(
            registry: &Self::Registry,
            operation: impl FnOnce(&mut HashMap<u64, Self::Erased>) -> R,
        ) -> R {
            operation(&mut registry.borrow_mut())
        }
    }

    impl Backend for Shared {
        type Erased = Arc<dyn Any + Send + Sync>;
        type Registry = Mutex<HashMap<u64, Self::Erased>>;
        fn directory<R>(
            registry: &Self::Registry,
            operation: impl FnOnce(&mut HashMap<u64, Self::Erased>) -> R,
        ) -> R {
            operation(&mut registry.lock().unwrap_or_else(|error| error.into_inner()))
        }
    }
}
