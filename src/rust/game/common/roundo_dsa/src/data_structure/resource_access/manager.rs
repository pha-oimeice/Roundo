use std::{
    fmt,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use crate::data_structure::resource_access::{
    AccessError, Local, Owner, Shared, Storage,
    identity::{DomainIdentity, ResourceControl, ResourceId},
};

/// Fixed module capacity, not a payload byte budget or a business policy.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Maximum currently registered resources, across all payload types.
    pub resources: usize,
    /// Maximum retained independent grant records per resource. Token clones
    /// share a record; retained revoked tokens and administrative IDs still count.
    pub grants_per_resource: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            resources: usize::MAX,
            grants_per_resource: usize::MAX,
        }
    }
}

/// Module-only recovery error: the unregistered input is returned unchanged.
/// No owner, lookup entry or identity sequence was published/consumed.
pub struct CreateError<T> {
    /// Reason registration was rejected.
    pub error: AccessError,
    /// Original payload, never registered; its destruction is caller-owned.
    pub value: T,
}

impl<T> fmt::Debug for CreateError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateError")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl<T> fmt::Display for CreateError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}

impl<T> std::error::Error for CreateError<T> {}

#[derive(Default)]
pub(crate) struct Counts {
    pub(crate) registered: AtomicUsize,
    pub(crate) resources: AtomicUsize,
    pub(crate) grants: AtomicUsize,
}

/// Observable module storage, excluding copied IDs, allocator overhead and
/// caller-owned copies/handles inside the payload. No permission is conferred.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResourceStatistics {
    /// Entries whose unique Owner has not normally ended.
    pub registered_resources: usize,
    /// Live resource control blocks, including ones retained by stale grants.
    pub resource_control_blocks: usize,
    /// Live grant records, including ones retained by administrative IDs.
    pub grant_control_blocks: usize,
}

/// Read-only diagnostics that can outlive Manager, Owner and tokens.
/// Clones retain only counters, not domain identity, registry, grants or data.
#[derive(Clone)]
pub struct ResourceMonitor(Arc<Counts>);

impl ResourceMonitor {
    /// Immediate counters, exact after callers quiesce. Concurrent changes may
    /// span the individual loads; this is not a cross-counter atomic snapshot.
    pub fn statistics(&self) -> ResourceStatistics {
        ResourceStatistics {
            registered_resources: self.0.registered.load(Ordering::Acquire),
            resource_control_blocks: self.0.resources.load(Ordering::Acquire),
            grant_control_blocks: self.0.grants.load(Ordering::Acquire),
        }
    }
}

pub(crate) struct Domain<M: Storage> {
    pub(crate) identity: Arc<DomainIdentity>,
    // Strong centralized data storage, with no back-edge from entry to Domain.
    pub(crate) entries: M::Registry,
    pub(crate) next_resource: Mutex<Option<u64>>,
    pub(crate) limits: Limits,
    pub(crate) counts: Arc<Counts>,
}

pub(crate) struct Entry<T> {
    pub(crate) value: RwLock<Option<T>>,
}

/// Central typed data registry and exact process-local issuing domain.
///
/// Moving or dropping this facade does not invalidate existing owners: each
/// Owner retains the cleanup domain. Tokens retain no directory or payload.
/// There is no token-only payload lookup, shutdown callback or business policy.
/// Resource sequences never wrap; new creation fails after exhaustion.
pub struct ResourceAccessManager<M: Storage = Local> {
    pub(crate) domain: Arc<Domain<M>>,
}

impl Default for ResourceAccessManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceAccessManager {
    /// Creates an empty, single-threaded domain with unlimited configured capacity.
    pub fn new() -> Self {
        Self::with_limits(Limits::default())
    }

    /// Creates an empty domain with fixed capacities. Zero capacity is valid
    /// and rejects the corresponding operation without publishing any state.
    pub fn with_limits(limits: Limits) -> Self {
        Self::configured(limits)
    }

    /// Registers an entire `T` and publishes its unique Owner, without grants.
    ///
    /// `Err` returns the untouched input and preserves registry/identity state.
    /// HashMap storage reservation is fallible; Arc allocation OOM follows
    /// the standard allocator's abort contract, not a promised recoverable Err.
    /// No user code or payload Drop is called during registry publication.
    pub fn create<T: 'static>(&self, value: T) -> Result<Owner<T>, CreateError<T>> {
        self.create_reserved(value, 1)
    }

    // Separate reservation size permits deterministic capacity-overflow injection.
    fn create_reserved<T: 'static>(
        &self,
        value: T,
        reservation: usize,
    ) -> Result<Owner<T>, CreateError<T>> {
        self.register(value, reservation, |entry| entry)
    }
}

impl ResourceAccessManager<Shared> {
    /// Creates a thread-safe directory. Only `Send + Sync` payloads can enter it.
    pub fn shared() -> Self {
        Self::with_shared_limits(Limits::default())
    }

    /// Thread-safe directory with fixed resource and retained-grant capacities.
    pub fn with_shared_limits(limits: Limits) -> Self {
        Self::configured(limits)
    }

    /// Registers thread-safe data with the same ownership and failure contract
    /// as the local mode. No cross-resource transaction or business policy runs.
    pub fn create<T: Send + Sync + 'static>(
        &self,
        value: T,
    ) -> Result<Owner<T, Shared>, CreateError<T>> {
        self.register(value, 1, |entry| entry)
    }
}

impl<M: Storage> ResourceAccessManager<M> {
    fn configured(limits: Limits) -> Self {
        Self {
            domain: Arc::new(Domain {
                identity: Arc::new(DomainIdentity),
                entries: Default::default(),
                next_resource: Mutex::new(Some(1)),
                limits,
                counts: Arc::new(Counts::default()),
            }),
        }
    }

    fn register<T: 'static>(
        &self,
        value: T,
        reservation: usize,
        erase: impl FnOnce(Arc<Entry<T>>) -> M::Erased,
    ) -> Result<Owner<T, M>, CreateError<T>> {
        let domain = &self.domain;
        M::directory(&domain.entries, |entries| {
            let mut next_resource = domain
                .next_resource
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let error = if entries.len() >= domain.limits.resources {
                Some(AccessError::ResourceCapacity)
            } else if next_resource.is_none() {
                Some(AccessError::IdentityExhausted)
            } else if entries.try_reserve(reservation).is_err() {
                Some(AccessError::RegistryAllocation)
            } else {
                None
            };
            if let Some(error) = error {
                return Err(CreateError { error, value });
            }
            let sequence = next_resource.expect("checked before publication");
            let control = Arc::new(ResourceControl {
                id: ResourceId {
                    domain: Arc::clone(&domain.identity),
                    sequence,
                },
                alive: AtomicBool::new(true),
                protected: Mutex::new(Default::default()),
                grants: AtomicUsize::new(0),
                counts: Arc::clone(&domain.counts),
            });
            let entry = Arc::new(Entry {
                value: RwLock::new(Some(value)),
            });
            entries.insert(sequence, erase(entry.clone()));
            *next_resource = sequence.checked_add(1);
            domain.counts.registered.fetch_add(1, Ordering::AcqRel);
            domain.counts.resources.fetch_add(1, Ordering::AcqRel);
            Ok(Owner {
                domain: Arc::clone(domain),
                entry,
                control,
                next_grant: Some(1),
            })
        })
    }

    /// Diagnostics do not keep any payload or authorization metadata alive.
    pub fn monitor(&self) -> ResourceMonitor {
        ResourceMonitor(Arc::clone(&self.domain.counts))
    }

    /// Validates live domain/instance membership, without granting data access.
    /// Old IDs, another manager's IDs and plain sequence numbers cannot resolve.
    pub fn contains(&self, id: &ResourceId) -> bool {
        Arc::ptr_eq(&self.domain.identity, &id.domain)
            && M::directory(&self.domain.entries, |entries| {
                entries.contains_key(&id.sequence)
            })
    }
}
