//! Trusted local banks with immediate exact Life validation.
//!
//! Live and detached reads lend immutable Quantity only within a synchronous
//! callback. Registration and accounts require their exact currency schema;
//! acquisition and use require their declared Kind and Context. Owner and Bank
//! fields are private, and the ledger resource has no public import.

use crate::{
    AcquisitionRule, BankApproval, BankError, BankHandle, BankOwner, Currency, CurrencySchema,
    RuleCatalogue, RuleSchema, UseRule, bank_store::BankStore,
};
use bevy::{
    ecs::{system::SystemParam, world::WorldId},
    prelude::{App, On, ResMut},
};
use roundo_life::{ConceptBodyHandle, FactoryInvalidation, LifeInstances, LifeRuntimePlugin};
use std::fmt;

/// Recoverable installation failures, before resource or observer publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EconomyInstallError {
    MissingLife,
    AlreadyInstalled,
    IdentityExhausted,
}
impl fmt::Display for EconomyInstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::MissingLife => "economy requires LifeRuntimePlugin",
            Self::AlreadyInstalled => "economy is already installed",
            Self::IdentityExhausted => "economy identity exhausted",
        })
    }
}
impl std::error::Error for EconomyInstallError {}

/// Installs an empty bank capability after Life, without resetting its clock,
/// configuration or history. Life's catalogue may be open or closed.
///
/// Does not install Life, declare sources, create Bodies, banks or accounts.
/// An existing store is never replaced. All recoverable errors precede resource
/// and single cleanup observer publication; the consumed rules drop normally.
/// Cleanup retains exact dead Body balances, not event provenance credentials.
pub fn install_economy(app: &mut App, rules: RuleCatalogue) -> Result<(), EconomyInstallError> {
    install_with(app, rules, BankStore::try_new)
}

// Private fail-closed allocator injection, not a recovery/reinstallation API.
fn install_with(
    app: &mut App,
    rules: RuleCatalogue,
    allocator: impl FnOnce(WorldId, RuleCatalogue) -> Result<BankStore, BankError>,
) -> Result<(), EconomyInstallError> {
    if !app.is_plugin_added::<LifeRuntimePlugin>() {
        return Err(EconomyInstallError::MissingLife);
    }
    if app.world().contains_resource::<BankStore>() {
        return Err(EconomyInstallError::AlreadyInstalled);
    }
    let store = match allocator(app.world().id(), rules) {
        Ok(store) => store,
        Err(BankError::IdentityExhausted) => return Err(EconomyInstallError::IdentityExhausted),
        Err(error) => panic!("bank constructor invariant: {error}"),
    };
    store
        .check_world(app.world().id())
        .expect("bank installation World invariant");
    app.insert_resource(store)
        .add_observer(detach_invalidated_owner);
    Ok(())
}

fn detach_invalidated_owner(
    event: On<FactoryInvalidation>,
    world: WorldId,
    life: LifeInstances,
    store: Option<ResMut<BankStore>>,
) {
    let Some(mut store) = store else { return };
    if store.check_world(world).is_err() {
        return;
    }
    let FactoryInvalidation::ConceptBody(body) = *event else {
        return;
    };
    // Notifications are copyable and carry no World/epoch provenance.
    if life.resolve_body(body).is_ok() {
        return;
    }
    store.detach_owner(body);
}

/// O(1) structural counts, not evidence of liveness or disclosure authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EconomyCounts {
    pub banks: usize,
    pub live_accounts: usize,
    pub detached_accounts: usize,
}

/// Trusted local bank access, not Player authorization or disclosure authority.
///
/// Every call checks the current World first. Normal account access then checks
/// bank structure, owner domain, schema identity and exact current Body liveness,
/// before approval, account state or any Quantity/evaluator calls. A Body need
/// not belong to a Life. Copying bank/owner identities proves neither liveness
/// nor authorization. Reads exclusively borrow the same resource as writes.
///
/// Explicit opening creates validated zero, never an initial nonzero issuance.
/// Acquisition/use re-evaluate sources and all applicable storage conditions
/// synchronously. Signed sources allow negative balances and deltas; sign and
/// overdraft policy belong to those rules, not a built-in zero floor. Acquisition
/// adds its rule's delta; use subtracts it, including when that delta is negative.
/// Recoverable errors preserve balances/indexes, not source
/// internal mutation, Drop/external effects, panic/OOM or sandbox guarantees.
/// No raw setter, caller delta/candidate, transfer, replayable plan or recovery
/// operation exists. Detached balances are retained, not purged or inherited.
#[derive(SystemParam)]
pub struct Banks<'w, 's> {
    world: WorldId,
    life: LifeInstances<'w, 's>,
    store: ResMut<'w, BankStore>,
}
impl Banks<'_, '_> {
    fn live(&self, body: ConceptBodyHandle) -> Result<(), BankError> {
        self.life
            .resolve_body(body)
            .map(|_| ())
            .map_err(|_| BankError::OwnerNotLive)
    }

    /// Selects the exact currently live Body in this trusted World.
    /// A bare handle carries no World origin: binding does not authenticate its
    /// original provenance or Player permissions, only current World selection.
    pub fn bind_owner(&self, body: ConceptBodyHandle) -> Result<BankOwner, BankError> {
        self.store.check_world(self.world)?;
        self.live(body)?;
        Ok(BankOwner::new(self.store.domain(), body))
    }

    /// Creates only a bank from immutable catalogue-bound approval, no accounts.
    /// World precedes approval provenance and identity allocation checks.
    pub fn create(&mut self, approval: BankApproval) -> Result<BankHandle, BankError> {
        self.store.check_world(self.world)?;
        self.store.create(approval)
    }

    /// World -> bank -> owner -> schema -> live Body -> approval/account/zero
    /// and Initialize storage evaluation. No implicit opening or issuance.
    pub fn open_account<C: Currency>(
        &mut self,
        bank: BankHandle,
        owner: BankOwner,
        schema: CurrencySchema<C>,
    ) -> Result<(), BankError> {
        self.store.check_world(self.world)?;
        self.store.preflight_currency(bank, owner, schema)?;
        self.live(owner.body())?;
        self.store.open_account(bank, owner, schema)
    }

    /// Same identity/live order as opening. Synchronously borrows revalidated
    /// Quantity read-only, without Clone. HRTB prevents escaping its reference;
    /// callers may return fields they themselves copy, but cannot obtain &mut.
    pub fn with_balance<C: Currency, T>(
        &self,
        bank: BankHandle,
        owner: BankOwner,
        schema: CurrencySchema<C>,
        read: impl for<'q> FnOnce(&'q C::Quantity) -> T,
    ) -> Result<T, BankError> {
        self.store.check_world(self.world)?;
        self.store.preflight_currency(bank, owner, schema)?;
        self.live(owner.body())?;
        self.store.with_balance(bank, owner, schema, read)
    }

    /// World -> bank -> owner -> rule and associated currency identity -> live
    /// Body -> approval/account -> fresh evaluation/checked sum/all storage.
    pub fn acquire<R: AcquisitionRule>(
        &mut self,
        bank: BankHandle,
        owner: BankOwner,
        schema: RuleSchema<R>,
        context: &R::Context,
    ) -> Result<(), BankError> {
        self.store.check_world(self.world)?;
        self.store.preflight_rule(bank, owner, schema)?;
        self.live(owner.body())?;
        self.store.acquire(bank, owner, schema, context)
    }

    /// Same order as acquire, with checked subtraction, never a caller delta.
    /// Signed results may cross zero; approved rules decide permission.
    pub fn use_currency<R: UseRule>(
        &mut self,
        bank: BankHandle,
        owner: BankOwner,
        schema: RuleSchema<R>,
        context: &R::Context,
    ) -> Result<(), BankError> {
        self.store.check_world(self.world)?;
        self.store.preflight_rule(bank, owner, schema)?;
        self.live(owner.body())?;
        self.store.use_currency(bank, owner, schema, context)
    }

    /// World -> core identity/approval/query/Quantity validation, without live
    /// Body resolution or implicit detach. A delayed cleanup still returns None.
    /// The synchronous HRTB callback borrows only shared Quantity, as live reads.
    pub fn with_detached_balance<C: Currency, T>(
        &self,
        bank: BankHandle,
        owner: BankOwner,
        schema: CurrencySchema<C>,
        read: impl for<'q> FnOnce(&'q C::Quantity) -> T,
    ) -> Result<Option<T>, BankError> {
        self.store.check_world(self.world)?;
        self.store.with_detached_balance(bank, owner, schema, read)
    }

    /// World -> O(1) stored counts; no Body scan or cleanup. Delayed dead
    /// registrations still count as live_accounts until notification cleanup.
    pub fn counts(&self) -> Result<EconomyCounts, BankError> {
        self.store.check_world(self.world)?;
        let (banks, live_accounts, detached_accounts) = self.store.counts();
        Ok(EconomyCounts {
            banks,
            live_accounts,
            detached_accounts,
        })
    }
}
