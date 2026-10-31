use std::{
    collections::HashSet,
    fmt,
    hash::Hash,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::ThreadId,
};

use crate::data_structure::resource_access::manager::Counts;

pub(crate) struct DomainIdentity;

/// Process-local identity, not an access or issuing capability.
///
/// Equality includes a retained, exact manager domain and a non-reused sequence.
/// Clones retain only the domain marker, never its registry or resource payload.
/// IDs are not serialized and have no meaning after process restart.
#[derive(Clone)]
pub struct ResourceId {
    pub(crate) domain: Arc<DomainIdentity>,
    pub(crate) sequence: u64,
}

impl ResourceId {
    /// Monotonic sequence within this domain; equal sequences across domains
    /// do not identify the same resource. Never use this number as permission.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
}

impl PartialEq for ResourceId {
    fn eq(&self, other: &Self) -> bool {
        self.sequence == other.sequence && Arc::ptr_eq(&self.domain, &other.domain)
    }
}

impl Eq for ResourceId {}

impl Hash for ResourceId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.domain).hash(state);
        self.sequence.hash(state);
    }
}

impl fmt::Debug for ResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResourceId")
            .field("domain", &Arc::as_ptr(&self.domain))
            .field("sequence", &self.sequence)
            .finish()
    }
}

pub(crate) struct ResourceControl {
    pub(crate) id: ResourceId,
    pub(crate) alive: AtomicBool,
    pub(crate) protected: Mutex<HashSet<ThreadId>>,
    pub(crate) grants: AtomicUsize,
    pub(crate) counts: Arc<Counts>,
}

impl Drop for ResourceControl {
    fn drop(&mut self) {
        self.counts.resources.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) struct GrantControl {
    pub(crate) resource: Arc<ResourceControl>,
    pub(crate) sequence: u64,
    pub(crate) revoked: AtomicBool,
}

impl Drop for GrantControl {
    fn drop(&mut self) {
        let resource = &self.resource;
        resource.grants.fetch_sub(1, Ordering::AcqRel);
        resource.counts.grants.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Administrative identity for one grant, not a read/modify token.
///
/// Obtained from a typed token. Clones retain the same grant metadata; retaining
/// this ID counts against the per-resource grant capacity, even after revocation.
/// It does not keep data alive and cannot issue or revive permissions. Release
/// unused IDs to reclaim metadata. Revocation still requires the exact Owner.
#[derive(Clone)]
pub struct GrantId(pub(crate) Arc<GrantControl>);

impl GrantId {
    /// Exact target resource; observing it confers no authority.
    pub fn resource_id(&self) -> &ResourceId {
        &self.0.resource.id
    }

    /// Non-reused sequence within this resource, not a standalone lookup key.
    pub fn sequence(&self) -> u64 {
        self.0.sequence
    }
}

impl PartialEq for GrantId {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for GrantId {}

impl fmt::Debug for GrantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GrantId")
            .field("resource", self.resource_id())
            .field("sequence", &self.sequence())
            .finish()
    }
}
