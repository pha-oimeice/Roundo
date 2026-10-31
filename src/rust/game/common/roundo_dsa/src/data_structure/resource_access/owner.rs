use std::{
    collections::HashSet,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, ThreadId},
};

use crate::data_structure::resource_access::{
    AccessError, GrantId, Local, ModifyAccess, ProtectedScope, ReadAccess, ResourceId, Storage,
    identity::{GrantControl, ResourceControl},
    manager::{Domain, Entry},
};

/// Unique lifecycle and trusted grant-issuing endpoint for registered data.
///
/// Not Clone/Copy. Moving preserves identity; normal Drop invalidates every
/// grant, removes registration, then destroys the payload before returning.
/// Tokens cannot postpone this destruction. Forgetting an Owner can leak it,
/// as with ordinary Rust RAII; no durable or panic-side-effect rollback is claimed.
/// Owner has no payload getter and is not a business read/modify permission.
pub struct Owner<T: 'static, M: Storage = Local> {
    pub(crate) domain: Arc<Domain<M>>,
    pub(crate) entry: Arc<Entry<T>>,
    pub(crate) control: Arc<ResourceControl>,
    pub(crate) next_grant: Option<u64>,
}

impl<T, M: Storage> fmt::Debug for Owner<T, M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Owner")
            .field("id", self.id())
            .finish_non_exhaustive()
    }
}

impl<T, M: Storage> Owner<T, M> {
    /// Observing/cloning an ID confers no payload or grant-issuing access.
    pub fn id(&self) -> &ResourceId {
        &self.control.id
    }

    /// Publishes a new whole-resource read grant. Recipients can clone/redistribute
    /// it, but all copies share revocation. `Err` consumes no grant sequence and
    /// changes no permission or registration state. No policy callback is run.
    pub fn grant_read(&mut self) -> Result<ReadAccess<T, M>, AccessError> {
        self.grant().map(ReadAccess::new)
    }

    /// Publishes an independent whole-resource modification-qualification grant.
    /// It grants no `&T`, `&mut T`, standalone read token or issuing authority.
    /// Error/clone/redistribution semantics are the same as [`Self::grant_read`].
    pub fn grant_modify(&mut self) -> Result<ModifyAccess<T, M>, AccessError> {
        self.grant().map(ModifyAccess::new)
    }

    fn grant(&mut self) -> Result<GrantId, AccessError> {
        if self.control.grants.load(Ordering::Acquire) >= self.domain.limits.grants_per_resource {
            return Err(AccessError::GrantCapacity);
        }
        let sequence = self.next_grant.ok_or(AccessError::IdentityExhausted)?;
        let grant = GrantId(Arc::new(GrantControl {
            resource: Arc::clone(&self.control),
            sequence,
            revoked: AtomicBool::new(false),
        }));
        self.next_grant = sequence.checked_add(1);
        self.control.grants.fetch_add(1, Ordering::AcqRel);
        self.control.counts.grants.fetch_add(1, Ordering::AcqRel);
        Ok(grant)
    }

    /// Permanently revokes this exact resource's grant and all token copies.
    /// Returns `true` once and `false` for repeated revocation. Wrong-resource
    /// (including wrong-domain) IDs return `Err` without changing either grant.
    /// A retained ID is metadata, not a reusable issuing or business permission.
    /// Exclusive Owner borrowing forbids revocation inside its protected callback.
    pub fn revoke(&mut self, grant: &GrantId) -> Result<bool, AccessError> {
        if !Arc::ptr_eq(&self.control, &grant.0.resource) {
            return Err(AccessError::ResourceMismatch);
        }
        Ok(!grant.0.revoked.swap(true, Ordering::AcqRel))
    }

    /// Lends a non-issuing scope for one synchronous callback. This proves only
    /// lifetime/borrow protection: each operation still requires its typed token.
    ///
    /// Scope/payload references/evidence cannot escape the callback, including
    /// into an unpolled future. Same-thread, same-resource re-entry returns
    /// `OperationInProgress`; another resource is independent. Shared-mode read
    /// scopes on different threads can coexist; each scope stays thread-confined.
    /// Grant/revoke/move/drop of this Owner
    /// during scope use is rejected by borrowing. Callback panic unwinds protection
    /// but does not undo user side effects or interior mutations.
    pub fn protect<R>(
        &self,
        operation: impl for<'scope> FnOnce(ProtectedScope<'scope, T, M>) -> R,
    ) -> Result<R, AccessError> {
        let thread = thread::current().id();
        if !self
            .control
            .protected
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(thread)
        {
            return Err(AccessError::OperationInProgress);
        }
        let _guard = Protection {
            threads: &self.control.protected,
            thread,
        };
        Ok(operation(ProtectedScope::new(&self.entry, &self.control)))
    }
}

struct Protection<'a> {
    threads: &'a Mutex<HashSet<ThreadId>>,
    thread: ThreadId,
}

impl Drop for Protection<'_> {
    fn drop(&mut self) {
        self.threads
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.thread);
    }
}

impl<T, M: Storage> Drop for Owner<T, M> {
    fn drop(&mut self) {
        // No safe public path drops the Owner while its entry is borrowed.
        // Terminate first, detach data/registration next, call user Drop last.
        self.control.alive.store(false, Ordering::Release);
        let value = self
            .entry
            .value
            .write()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        let removed = M::directory(&self.domain.entries, |entries| {
            entries.remove(&self.id().sequence())
        });
        self.domain.counts.registered.fetch_sub(1, Ordering::AcqRel);
        drop(removed);
        drop(value);
    }
}
