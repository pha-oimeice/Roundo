//! Typed pure rule evaluation; no ledger or reusable commit authorization.
//!
//! Each source declares one of the three sealed rule categories and fixes its
//! Context and Currency. RuleSchema and StorageView fields are private; kinds
//! and currencies cannot be interchanged. Freeze consumes the builder and closes
//! registration.

use crate::{__private, Amount, Currency, CurrencyCatalogue, CurrencyError, CurrencySchema};
use std::{
    any::TypeId,
    collections::HashMap,
    fmt,
    marker::PhantomData,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_RULE_STAMP: AtomicU64 = AtomicU64::new(1);

mod kind_sealed {
    pub trait Sealed {}
}

/// Fixed category: only AcquisitionKind, UseKind and StorageKind.
///
/// The sealing trait is in a private module, so external sources cannot introduce
/// a fourth Kind. No runtime kind conversion exists.
pub trait RuleKind: kind_sealed::Sealed + Send + Sync + 'static {}

/// Acquisition rule category; has no instances.
pub enum AcquisitionKind {}
/// Use rule category; has no instances.
pub enum UseKind {}
/// Storage rule category; has no instances.
pub enum StorageKind {}

impl kind_sealed::Sealed for AcquisitionKind {}
impl kind_sealed::Sealed for UseKind {}
impl kind_sealed::Sealed for StorageKind {}
impl RuleKind for AcquisitionKind {}
impl RuleKind for UseKind {}
impl RuleKind for StorageKind {}

/// Exact source key, globally unique within one rule catalogue across kinds and currencies.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuleKey(&'static str);

impl RuleKey {
    #[doc(hidden)]
    pub const fn from_source(value: &'static str) -> Self {
        Self(value)
    }

    /// Returns the exact string; no normalization or naming restriction is imposed.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// Source-declared typed rule metadata.
///
/// Evaluators must be pure, deterministic and read-only over Context and quantities.
/// Source declaration markers and macros are not a sandbox: source effects, panic,
/// allocation failure and internal mutation are not rolled back. No Clone/Debug
/// contract is required of the source, its context or its currency quantity.
pub trait RuleSource: __private::RuleSealed + Send + Sync + 'static {
    type Currency: Currency;
    type Kind: RuleKind;
    type Context: Send + Sync + 'static;
    const KEY: RuleKey;
}

/// Source refusal, with no framework-defined gameplay reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuleRejection;

/// Computes a candidate acquisition delta, not authority to commit a balance.
pub trait AcquisitionRule: RuleSource<Kind = AcquisitionKind> {
    fn evaluate(
        context: &Self::Context,
        balance: &<Self::Currency as Currency>::Quantity,
    ) -> Result<<Self::Currency as Currency>::Quantity, RuleRejection>;
}

/// Computes a candidate use delta, not authority to commit a balance.
pub trait UseRule: RuleSource<Kind = UseKind> {
    fn evaluate(
        context: &Self::Context,
        balance: &<Self::Currency as Currency>::Quantity,
    ) -> Result<<Self::Currency as Currency>::Quantity, RuleRejection>;
}

/// Pure condition on a candidate view. Success is not future commit authorization.
pub trait StorageRule: RuleSource<Kind = StorageKind> {
    fn evaluate(
        context: &Self::Context,
        view: StorageView<'_, Self::Currency>,
    ) -> Result<(), RuleRejection>;
}

/// Declares a typed acquisition rule bound to a noncapturing function pointer.
#[macro_export]
macro_rules! declare_acquisition_rule {
    ($type:ty, $key:literal, $currency:ty, $context:ty, $evaluator:expr) => {
        impl $crate::__private::RuleSealed for $type {}
        impl $crate::RuleSource for $type {
            type Currency = $currency;
            type Kind = $crate::AcquisitionKind;
            type Context = $context;
            const KEY: $crate::RuleKey = $crate::RuleKey::from_source($key);
        }
        impl $crate::AcquisitionRule for $type {
            fn evaluate(
                context: &Self::Context,
                balance: &<Self::Currency as $crate::Currency>::Quantity,
            ) -> ::core::result::Result<
                <Self::Currency as $crate::Currency>::Quantity,
                $crate::RuleRejection,
            > {
                let evaluator: fn(
                    &$context,
                    &<$currency as $crate::Currency>::Quantity,
                ) -> ::core::result::Result<
                    <$currency as $crate::Currency>::Quantity,
                    $crate::RuleRejection,
                > = $evaluator;
                evaluator(context, balance)
            }
        }
    };
}

/// Declares a typed use rule bound to a noncapturing function pointer.
#[macro_export]
macro_rules! declare_use_rule {
    ($type:ty, $key:literal, $currency:ty, $context:ty, $evaluator:expr) => {
        impl $crate::__private::RuleSealed for $type {}
        impl $crate::RuleSource for $type {
            type Currency = $currency;
            type Kind = $crate::UseKind;
            type Context = $context;
            const KEY: $crate::RuleKey = $crate::RuleKey::from_source($key);
        }
        impl $crate::UseRule for $type {
            fn evaluate(
                context: &Self::Context,
                balance: &<Self::Currency as $crate::Currency>::Quantity,
            ) -> ::core::result::Result<
                <Self::Currency as $crate::Currency>::Quantity,
                $crate::RuleRejection,
            > {
                let evaluator: fn(
                    &$context,
                    &<$currency as $crate::Currency>::Quantity,
                ) -> ::core::result::Result<
                    <$currency as $crate::Currency>::Quantity,
                    $crate::RuleRejection,
                > = $evaluator;
                evaluator(context, balance)
            }
        }
    };
}

/// Declares a typed storage rule bound to a noncapturing function pointer.
#[macro_export]
macro_rules! declare_storage_rule {
    ($type:ty, $key:literal, $currency:ty, $context:ty, $evaluator:expr) => {
        impl $crate::__private::RuleSealed for $type {}
        impl $crate::RuleSource for $type {
            type Currency = $currency;
            type Kind = $crate::StorageKind;
            type Context = $context;
            const KEY: $crate::RuleKey = $crate::RuleKey::from_source($key);
        }
        impl $crate::StorageRule for $type {
            fn evaluate(
                context: &Self::Context,
                view: $crate::StorageView<'_, Self::Currency>,
            ) -> ::core::result::Result<(), $crate::RuleRejection> {
                let evaluator: for<'a> fn(
                    &$context,
                    $crate::StorageView<'a, $currency>,
                )
                    -> ::core::result::Result<(), $crate::RuleRejection> = $evaluator;
                evaluator(context, view)
            }
        }
    };
}

/// Candidate change category; not a bank operation or authorization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageDirection {
    Initialize,
    Acquire,
    Use,
}

/// Read-only candidate view. Arithmetic relationships are not proved here;
/// the bank must generate real candidates and re-evaluate all applicable rules.
pub struct StorageView<'a, C: Currency> {
    before: &'a C::Quantity,
    delta: &'a C::Quantity,
    after: &'a C::Quantity,
    direction: StorageDirection,
}

impl<'a, C: Currency> Copy for StorageView<'a, C> {}
impl<'a, C: Currency> Clone for StorageView<'a, C> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<'a, C: Currency> StorageView<'a, C> {
    pub fn before(self) -> &'a C::Quantity {
        self.before
    }
    pub fn delta(self) -> &'a C::Quantity {
        self.delta
    }
    pub fn after(self) -> &'a C::Quantity {
        self.after
    }
    pub fn direction(self) -> StorageDirection {
        self.direction
    }
}

/// Rule registration/identity/refusal errors, preserving currency error semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuleError {
    IdentityExhausted,
    TypeAlreadyRegistered,
    DuplicateKey,
    WrongRuleCatalogue,
    Rejected,
    Currency(CurrencyError),
}
impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IdentityExhausted => f.write_str("rule catalogue identity space is exhausted"),
            Self::TypeAlreadyRegistered => f.write_str("rule type is already registered"),
            Self::DuplicateKey => f.write_str("rule key is already registered"),
            Self::WrongRuleCatalogue => f.write_str("rule schema belongs to another catalogue"),
            Self::Rejected => f.write_str("rule source rejected the candidate"),
            Self::Currency(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for RuleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Currency(error) => Some(error),
            _ => None,
        }
    }
}

/// Open registry owning its currency catalogue, consumed by freeze.
pub struct RuleBuilder {
    stamp: u64,
    currencies: CurrencyCatalogue,
    registry: Registry,
}
impl RuleBuilder {
    /// Allocates an independent rule identity; currencies are consumed even on Err.
    pub fn try_new(currencies: CurrencyCatalogue) -> Result<Self, RuleError> {
        Ok(Self {
            stamp: allocate_rule_stamp(&NEXT_RULE_STAMP)?,
            currencies,
            registry: Registry::default(),
        })
    }

    /// Checks type, global key, then currency identity. Never calls an evaluator.
    /// Err preserves all registry fields; no half-registration is published.
    pub fn register<R: RuleSource>(
        &mut self,
        currency: CurrencySchema<R::Currency>,
    ) -> Result<RuleSchema<R>, RuleError> {
        let type_id = TypeId::of::<R>();
        if self.registry.by_type.contains_key(&type_id) {
            return Err(RuleError::TypeAlreadyRegistered);
        }
        if self.registry.by_key.contains_key(&R::KEY) {
            return Err(RuleError::DuplicateKey);
        }
        self.currencies
            .validate_schema(currency)
            .map_err(RuleError::Currency)?;
        self.registry.by_type.insert(
            type_id,
            RegistrationRecord {
                rule_type: type_id,
                key: R::KEY,
                kind_type: TypeId::of::<R::Kind>(),
                currency_type: TypeId::of::<R::Currency>(),
                context_type: TypeId::of::<R::Context>(),
            },
        );
        self.registry.by_key.insert(R::KEY, type_id);
        Ok(RuleSchema {
            stamp: self.stamp,
            currency,
            marker: PhantomData,
        })
    }

    /// Closes registration, including an empty catalogue; no default rules exist.
    pub fn freeze(self) -> RuleCatalogue {
        RuleCatalogue {
            stamp: self.stamp,
            currencies: self.currencies,
            registry: self.registry,
        }
    }
}

/// Opaque typed rule token. Copying/moving does not change its catalogue identity.
pub struct RuleSchema<R: RuleSource> {
    stamp: u64,
    currency: CurrencySchema<R::Currency>,
    marker: PhantomData<fn() -> R>,
}
impl<R: RuleSource> Copy for RuleSchema<R> {}
impl<R: RuleSource> Clone for RuleSchema<R> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<R: RuleSource> fmt::Debug for RuleSchema<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuleSchema")
            .field("key", &R::KEY)
            .field("stamp", &self.stamp)
            .field("currency", &self.currency)
            .finish_non_exhaustive()
    }
}
impl<R: RuleSource> RuleSchema<R> {
    pub fn key(self) -> RuleKey {
        R::KEY
    }
    pub fn currency(self) -> CurrencySchema<R::Currency> {
        self.currency
    }
}

/// Frozen pure rule evaluation, without ledger, bank, Life checks or commit authority.
///
/// All identities are checked before source quantity functions. Valid inputs are
/// rechecked on each evaluation, then the evaluator runs exactly once. Errors
/// preserve catalogue and existing Amount framework fields, not source effects.
pub struct RuleCatalogue {
    stamp: u64,
    currencies: CurrencyCatalogue,
    registry: Registry,
}
impl RuleCatalogue {
    pub fn currencies(&self) -> &CurrencyCatalogue {
        &self.currencies
    }

    /// Returns a validated candidate delta. It cannot authorize a later commit.
    pub fn evaluate_acquisition<R: AcquisitionRule>(
        &self,
        schema: RuleSchema<R>,
        context: &R::Context,
        balance: &Amount<R::Currency>,
    ) -> Result<Amount<R::Currency>, RuleError> {
        self.validate_rule_schema(schema)?;
        self.currencies
            .validate_amount_identity(balance)
            .map_err(RuleError::Currency)?;
        self.currencies
            .compare(balance, balance)
            .map_err(RuleError::Currency)?;
        let output =
            R::evaluate(context, balance.as_quantity()).map_err(|_| RuleError::Rejected)?;
        Ok(self
            .currencies
            .amount(schema.currency(), output)
            .map_err(RuleError::Currency)?)
    }

    /// Returns a validated candidate delta; neither spending nor subtraction occurs.
    pub fn evaluate_use<R: UseRule>(
        &self,
        schema: RuleSchema<R>,
        context: &R::Context,
        balance: &Amount<R::Currency>,
    ) -> Result<Amount<R::Currency>, RuleError> {
        self.validate_rule_schema(schema)?;
        self.currencies
            .validate_amount_identity(balance)
            .map_err(RuleError::Currency)?;
        self.currencies
            .compare(balance, balance)
            .map_err(RuleError::Currency)?;
        let output =
            R::evaluate(context, balance.as_quantity()).map_err(|_| RuleError::Rejected)?;
        Ok(self
            .currencies
            .amount(schema.currency(), output)
            .map_err(RuleError::Currency)?)
    }

    /// Checks all three identities before validating quantities in before/delta/after
    /// order. Does not check arithmetic relationships or confer commit authority.
    pub fn evaluate_storage<R: StorageRule>(
        &self,
        schema: RuleSchema<R>,
        context: &R::Context,
        before: &Amount<R::Currency>,
        delta: &Amount<R::Currency>,
        after: &Amount<R::Currency>,
        direction: StorageDirection,
    ) -> Result<(), RuleError> {
        self.validate_rule_schema(schema)?;
        self.currencies
            .validate_amount_identity(before)
            .map_err(RuleError::Currency)?;
        self.currencies
            .validate_amount_identity(delta)
            .map_err(RuleError::Currency)?;
        self.currencies
            .validate_amount_identity(after)
            .map_err(RuleError::Currency)?;
        self.currencies
            .compare(before, before)
            .map_err(RuleError::Currency)?;
        self.currencies
            .compare(delta, delta)
            .map_err(RuleError::Currency)?;
        self.currencies
            .compare(after, after)
            .map_err(RuleError::Currency)?;
        R::evaluate(
            context,
            StorageView {
                before: before.as_quantity(),
                delta: delta.as_quantity(),
                after: after.as_quantity(),
                direction,
            },
        )
        .map_err(|_| RuleError::Rejected)
    }

    pub(crate) fn stamp(&self) -> u64 {
        self.stamp
    }

    pub(crate) fn validate_rule_schema<R: RuleSource>(
        &self,
        schema: RuleSchema<R>,
    ) -> Result<(), RuleError> {
        if schema.stamp != self.stamp {
            return Err(RuleError::WrongRuleCatalogue);
        }
        let type_id = TypeId::of::<R>();
        let record = self
            .registry
            .by_type
            .get(&type_id)
            .expect("matching stamp requires registered rule");
        assert_eq!(record.rule_type, type_id);
        assert_eq!(record.key, R::KEY);
        assert_eq!(record.kind_type, TypeId::of::<R::Kind>());
        assert_eq!(record.currency_type, TypeId::of::<R::Currency>());
        assert_eq!(record.context_type, TypeId::of::<R::Context>());
        assert_eq!(self.registry.by_key.get(&R::KEY), Some(&type_id));
        self.currencies
            .validate_schema(schema.currency())
            .map_err(RuleError::Currency)?;
        Ok(())
    }
}

#[derive(Default)]
struct Registry {
    by_type: HashMap<TypeId, RegistrationRecord>,
    by_key: HashMap<RuleKey, TypeId>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct RegistrationRecord {
    rule_type: TypeId,
    key: RuleKey,
    kind_type: TypeId,
    currency_type: TypeId,
    context_type: TypeId,
}
fn allocate_rule_stamp(counter: &AtomicU64) -> Result<u64, RuleError> {
    counter
        .try_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
            if current == u64::MAX {
                None
            } else {
                Some(current + 1)
            }
        })
        .map_err(|_| RuleError::IdentityExhausted)
}
