use std::{
    fmt,
    marker::PhantomData,
    sync::{Arc, atomic::Ordering},
};

use crate::data_structure::resource_access::{
    AccessError, GrantId, Local, ResourceId, Storage, identity::ResourceControl, manager::Entry,
};

fn check(grant: &GrantId) -> Result<(), AccessError> {
    if !grant.0.resource.alive.load(Ordering::Acquire) {
        Err(AccessError::ResourceEnded)
    } else if grant.0.revoked.load(Ordering::Acquire) {
        Err(AccessError::GrantRevoked)
    } else {
        Ok(())
    }
}

/// Non-owning read capability for an entire exact typed instance.
/// Clone redistributes the same grant (without requiring `T: Clone`). Resource
/// end or grant revocation permanently invalidates all copies. No payload strong
/// reference, arbitrary-ID constructor or conversion to Modify is provided.
pub struct ReadAccess<T: 'static, M: Storage = Local> {
    grant: GrantId,
    marker: PhantomData<(fn(T) -> T, M)>,
}

/// Non-owning whole-instance modification qualification, not read permission.
/// Clone redistributes the same grant; it grants no payload access, generic
/// mutation operation or conversion to Read. Resource/grant invalidation is the
/// same as [`ReadAccess`]. Domains must still check their current business policy.
pub struct ModifyAccess<T: 'static, M: Storage = Local> {
    grant: GrantId,
    marker: PhantomData<(fn(T) -> T, M)>,
}

macro_rules! token_impl {
    ($token:ident) => {
        impl<T, M: Storage> $token<T, M> {
            pub(crate) fn new(grant: GrantId) -> Self {
                Self {
                    grant,
                    marker: PhantomData,
                }
            }

            /// Retains this grant's administrative metadata, never the payload.
            /// The exact Owner is still required for revocation.
            pub fn grant_id(&self) -> GrantId {
                self.grant.clone()
            }

            /// Target identity alone grants no data access or issuing authority.
            pub fn resource_id(&self) -> &ResourceId {
                self.grant.resource_id()
            }

            /// Instantaneous diagnostic validity, not a guard or lasting permit.
            /// Actual access must use a matching protected scope; resource end
            /// takes precedence over revocation when both are terminal.
            pub fn check(&self) -> Result<(), AccessError> {
                check(&self.grant)
            }
        }

        impl<T, M: Storage> Clone for $token<T, M> {
            fn clone(&self) -> Self {
                Self::new(self.grant.clone())
            }
        }

        impl<T, M: Storage> fmt::Debug for $token<T, M> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($token))
                    .field("grant", &self.grant)
                    .finish()
            }
        }
    };
}

token_impl!(ReadAccess);
token_impl!(ModifyAccess);

/// Borrowed lifetime protection with no Owner, registry or payload getter.
/// Cannot be constructed by recipients, cloned, retained or used to issue grants.
/// Provided only by [`crate::data_structure::resource_access::Owner::protect`]. It is not itself a permission.
pub struct ProtectedScope<'scope, T: 'static, M: Storage = Local> {
    entry: &'scope Entry<T>,
    control: &'scope Arc<ResourceControl>,
    operation: std::cell::Cell<bool>,
    marker: PhantomData<(M, std::rc::Rc<()>)>,
}

impl<'scope, T, M: Storage> ProtectedScope<'scope, T, M> {
    pub(crate) fn new(entry: &'scope Entry<T>, control: &'scope Arc<ResourceControl>) -> Self {
        Self {
            entry,
            control,
            operation: std::cell::Cell::new(false),
            marker: PhantomData,
        }
    }

    /// Reads only after validating exact domain/instance and the current grant.
    /// Reference lifetime is limited to the synchronous callback. Same-scope
    /// recursive read/modify operations reject instead of nesting or waiting.
    /// Copies, interior mutability and callback side effects are domain-owned;
    /// `&T` is not a promise to sandbox arbitrary `T`. Errors never run callback.
    pub fn with_read<R>(
        &self,
        access: &ReadAccess<T, M>,
        read: impl for<'read> FnOnce(&'read T) -> R,
    ) -> Result<R, AccessError> {
        self.validate(&access.grant)?;
        let _guard = self.begin()?;
        let value = self
            .entry
            .value
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let payload = value.as_ref().ok_or(AccessError::ResourceEnded)?;
        Ok(read(payload))
    }

    /// Confirms modification qualification and lends non-replayable evidence.
    /// No payload is disclosed or mutated by this interface. The domain adapter
    /// implements the operation and any business validation/rollback itself.
    /// Evidence cannot escape the callback or be replaced by a copied ID. Errors
    /// never call the operation; panic does not roll back its external effects.
    pub fn with_modify_permission<R>(
        &self,
        access: &ModifyAccess<T, M>,
        operation: impl for<'operation> FnOnce(ModifyPermission<'operation, T>) -> R,
    ) -> Result<R, AccessError> {
        self.validate(&access.grant)?;
        let _guard = self.begin()?;
        Ok(operation(ModifyPermission {
            resource: &self.control.id,
            marker: PhantomData,
        }))
    }

    fn validate(&self, grant: &GrantId) -> Result<(), AccessError> {
        if !Arc::ptr_eq(self.control, &grant.0.resource) {
            return Err(AccessError::ResourceMismatch);
        }
        check(grant)
    }

    fn begin(&self) -> Result<Operation<'_>, AccessError> {
        if self.operation.replace(true) {
            return Err(AccessError::OperationInProgress);
        }
        Ok(Operation(&self.operation))
    }
}

struct Operation<'a>(&'a std::cell::Cell<bool>);

impl Drop for Operation<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// Short-lived, non-Clone modification qualification for one exact resource.
/// A domain may consume it within the protected callback and verify its target.
/// There is no public constructor, payload getter, read token or lasting permit.
/// Copied resource identity is only an ID and cannot replay this evidence.
pub struct ModifyPermission<'operation, T: 'static> {
    resource: &'operation ResourceId,
    marker: PhantomData<fn(T) -> T>,
}

impl<T> ModifyPermission<'_, T> {
    /// Exact target for a domain adapter to compare with its operation target.
    pub fn resource_id(&self) -> &ResourceId {
        self.resource
    }
}
