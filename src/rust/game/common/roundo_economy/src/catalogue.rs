use crate::{Currency, CurrencyKey, Quantity};
use std::{
    any::{Any, TypeId},
    cmp::Ordering,
    collections::HashMap,
    fmt,
    marker::PhantomData,
    sync::atomic::{AtomicU64, Ordering as AtomicOrdering},
};

static NEXT_CATALOGUE_STAMP: AtomicU64 = AtomicU64::new(1);

/// Registration, identity, representation and observed quantity-law failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CurrencyError {
    IdentityExhausted,
    TypeAlreadyRegistered,
    DuplicateKey,
    InvalidQuantityModel,
    WrongCatalogue,
    InvalidAmount,
    QuantityLawViolation,
    AdditionUnavailable,
    SubtractionUnavailable,
}

impl fmt::Display for CurrencyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::IdentityExhausted => "currency catalogue identity space is exhausted",
            Self::TypeAlreadyRegistered => "currency type is already registered",
            Self::DuplicateKey => "currency key is already registered",
            Self::InvalidQuantityModel => "currency zero probe failed",
            Self::WrongCatalogue => "schema or amount belongs to another catalogue",
            Self::InvalidAmount => "input quantity is invalid for its source representation",
            Self::QuantityLawViolation => "source violated an observed quantity law",
            Self::AdditionUnavailable => "exact sum is not representable",
            Self::SubtractionUnavailable => "exact difference is not representable",
        })
    }
}

impl std::error::Error for CurrencyError {}

/// Open catalogue. Registration is consumed by `freeze`; no cloning or reset.
///
/// Schemas remain valid when the builder/catalogue moves. Identities are never
/// reused during this process. This object does not hold balances or accounts.
pub struct CurrencyBuilder {
    stamp: u64,
    registry: Registry,
}

impl CurrencyBuilder {
    /// Allocates a fresh private identity; exhaustion publishes no builder.
    pub fn try_new() -> Result<Self, CurrencyError> {
        Ok(Self {
            stamp: allocate_catalogue_stamp(&NEXT_CATALOGUE_STAMP)?,
            registry: Registry::default(),
        })
    }

    /// Checks type, then exact key, then the source's zero model and registers.
    ///
    /// Errors leave the entire registry and existing schemas unchanged; source
    /// effects, panic, allocation failure and internal mutability do not roll
    /// back. Conflict paths do not call source quantity functions.
    pub fn register<C: Currency>(&mut self) -> Result<CurrencySchema<C>, CurrencyError> {
        let type_id = TypeId::of::<C>();
        if self.registry.by_type.contains_key(&type_id) {
            return Err(CurrencyError::TypeAlreadyRegistered);
        }
        if self.registry.by_key.contains_key(&C::KEY) {
            return Err(CurrencyError::DuplicateKey);
        }
        let zero = C::Quantity::zero();
        zero_probe(&zero).map_err(|_| CurrencyError::InvalidQuantityModel)?;
        let record = RegistrationRecord {
            type_id,
            key: C::KEY,
            quantity_type: TypeId::of::<C::Quantity>(),
            zero: Box::new(zero),
        };
        self.registry.by_type.insert(type_id, record);
        self.registry.by_key.insert(C::KEY, type_id);
        Ok(CurrencySchema {
            stamp: self.stamp,
            marker: PhantomData,
        })
    }

    /// Permanently closes registration, including for an empty catalogue.
    /// Existing schema identities and cached zero witnesses are preserved.
    pub fn freeze(self) -> CurrencyCatalogue {
        CurrencyCatalogue {
            stamp: self.stamp,
            registry: self.registry,
        }
    }
}

/// Copyable typed registration token, minted only by its builder.
///
/// Only its originating catalogue accepts it. Copying does not rebind it;
/// moving or freezing that catalogue does not invalidate it.
pub struct CurrencySchema<C: Currency> {
    stamp: u64,
    marker: PhantomData<fn() -> C>,
}

impl<C: Currency> CurrencySchema<C> {
    /// Returns the declared exact key; does not grant balance authority.
    pub fn key(self) -> CurrencyKey {
        C::KEY
    }
}

impl<C: Currency> Copy for CurrencySchema<C> {}

impl<C: Currency> Clone for CurrencySchema<C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<C: Currency> fmt::Debug for CurrencySchema<C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CurrencySchema")
            .field("key", &C::KEY)
            .field("stamp", &self.stamp)
            .finish_non_exhaustive()
    }
}

/// Owned, catalogue-bound quantity validated by finite source probes.
///
/// Not a balance, issuance/spending authorization or a permanent proof that a
/// source will remain valid. No mutable/raw extraction or arithmetic bypass is
/// provided. Moving this value preserves its identity; operations revalidate.
pub struct Amount<C: Currency> {
    stamp: u64,
    quantity: C::Quantity,
}

impl<C: Currency> Amount<C> {
    /// Borrows the source representation read-only for this borrow's lifetime.
    pub fn as_quantity(&self) -> &C::Quantity {
        &self.quantity
    }
}

impl<C: Currency> fmt::Debug for Amount<C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Amount")
            .field("key", &C::KEY)
            .field("stamp", &self.stamp)
            .finish_non_exhaustive()
    }
}

/// Frozen registry and pure typed quantity calculations, without a ledger.
///
/// All errors preserve registry, cached witnesses and existing Amount fields.
/// Source effects/internal mutation are not sandboxed or rolled back. Calls are
/// finite, but their latency and allocation depend on the source. All quantities
/// are owned except shared input borrows; no source Clone/Debug is required.
pub struct CurrencyCatalogue {
    stamp: u64,
    registry: Registry,
}

impl CurrencyCatalogue {
    /// Checks schema identity, then input validity and finite identity laws.
    ///
    /// Consumes `value` even on error (ordinary Drop, no return of the input).
    /// Invalid input is `InvalidAmount`; observed law failure is
    /// `QuantityLawViolation`. A foreign schema is rejected before source calls.
    pub fn amount<C: Currency>(
        &self,
        schema: CurrencySchema<C>,
        value: C::Quantity,
    ) -> Result<Amount<C>, CurrencyError> {
        self.validate_schema(schema)?;
        let zero = self.cached_zero::<C>();
        input_valid(&value, zero)?;
        Ok(self.wrap::<C>(value))
    }

    /// Calls the source for a fresh zero after checking schema identity.
    /// All invalid outputs or mismatch with the cached zero are law violations.
    pub fn zero<C: Currency>(&self, schema: CurrencySchema<C>) -> Result<Amount<C>, CurrencyError> {
        self.validate_schema(schema)?;
        let cached = self.cached_zero::<C>();
        let value = C::Quantity::zero();
        output_valid(&value, cached)?;
        identity_valid(&value, cached)?;
        require_equal(&value, cached)?;
        Ok(self.wrap::<C>(value))
    }

    /// Checks both identities before either quantity, then exact sum laws.
    /// Both directions returning None means `AdditionUnavailable`; asymmetric
    /// availability, invalid output or failed inverse is a law violation.
    /// Signed inputs/results are allowed; ordering must agree with operand signs.
    /// This calculation does not decide whether a transaction is permitted.
    pub fn checked_add<C: Currency>(
        &self,
        left: &Amount<C>,
        right: &Amount<C>,
    ) -> Result<Amount<C>, CurrencyError> {
        let zero = self.validate_pair(left, right)?;
        let forward = left.quantity.checked_add(&right.quantity);
        let reverse = right.quantity.checked_add(&left.quantity);
        let (sum, witness) = match (forward, reverse) {
            (None, None) => return Err(CurrencyError::AdditionUnavailable),
            (Some(sum), Some(witness)) => (sum, witness),
            _ => return Err(CurrencyError::QuantityLawViolation),
        };
        output_valid(&sum, zero)?;
        output_valid(&witness, zero)?;
        require_equal(&sum, &witness)?;
        require_order(
            checked_order(&sum, &left.quantity)? == checked_order(&right.quantity, zero)?,
        )?;
        require_order(
            checked_order(&sum, &right.quantity)? == checked_order(&left.quantity, zero)?,
        )?;
        identity_valid(&sum, zero)?;
        check_witness(sum.checked_sub(&left.quantity), &right.quantity, zero)?;
        check_witness(sum.checked_sub(&right.quantity), &left.quantity, zero)?;
        Ok(self.wrap::<C>(sum))
    }

    /// Checks both identities and input laws, then calculates the exact difference.
    /// A greater right operand may yield a negative result; there is no built-in
    /// balance floor. An unavailable representation is `SubtractionUnavailable`,
    /// not a business rejection. Transaction permission belongs to bank rules.
    pub fn checked_sub<C: Currency>(
        &self,
        left: &Amount<C>,
        right: &Amount<C>,
    ) -> Result<Amount<C>, CurrencyError> {
        let zero = self.validate_pair(left, right)?;
        let order = checked_order(&right.quantity, &left.quantity)?;
        let difference = left
            .quantity
            .checked_sub(&right.quantity)
            .ok_or(CurrencyError::SubtractionUnavailable)?;
        output_valid(&difference, zero)?;
        require_order(
            checked_order(&difference, &left.quantity)? == checked_order(zero, &right.quantity)?,
        )?;
        identity_valid(&difference, zero)?;
        check_witness(
            difference.checked_add(&right.quantity),
            &left.quantity,
            zero,
        )?;
        check_witness(
            right.quantity.checked_add(&difference),
            &left.quantity,
            zero,
        )?;
        if order == Ordering::Equal {
            require_equal(&difference, zero)?;
        }
        Ok(self.wrap::<C>(difference))
    }

    /// Checks both identities, both quantities and local Eq/Ord consistency.
    /// No cross-catalogue or unchecked standard-ordering bypass is provided.
    pub fn compare<C: Currency>(
        &self,
        left: &Amount<C>,
        right: &Amount<C>,
    ) -> Result<Ordering, CurrencyError> {
        self.validate_pair(left, right)?;
        checked_order(&left.quantity, &right.quantity)
    }

    pub(crate) fn validate_schema<C: Currency>(
        &self,
        schema: CurrencySchema<C>,
    ) -> Result<(), CurrencyError> {
        if schema.stamp != self.stamp {
            return Err(CurrencyError::WrongCatalogue);
        }
        self.record::<C>();
        Ok(())
    }

    pub(crate) fn validate_amount_identity<C: Currency>(
        &self,
        amount: &Amount<C>,
    ) -> Result<(), CurrencyError> {
        if amount.stamp != self.stamp {
            return Err(CurrencyError::WrongCatalogue);
        }
        self.record::<C>();
        Ok(())
    }

    fn record<C: Currency>(&self) -> &RegistrationRecord {
        let type_id = TypeId::of::<C>();
        let record = self
            .registry
            .by_type
            .get(&type_id)
            .expect("matching stamp requires registered currency");
        assert_eq!(record.type_id, type_id);
        assert_eq!(record.key, C::KEY);
        assert_eq!(record.quantity_type, TypeId::of::<C::Quantity>());
        assert_eq!(self.registry.by_key.get(&C::KEY), Some(&type_id));
        assert!(record.zero.is::<C::Quantity>());
        record
    }

    fn cached_zero<C: Currency>(&self) -> &C::Quantity {
        self.record::<C>()
            .zero
            .downcast_ref::<C::Quantity>()
            .expect("registered zero witness must have associated quantity type")
    }

    fn validate_pair<C: Currency>(
        &self,
        left: &Amount<C>,
        right: &Amount<C>,
    ) -> Result<&C::Quantity, CurrencyError> {
        self.validate_amount_identity(left)?;
        self.validate_amount_identity(right)?;
        let zero = self.cached_zero::<C>();
        input_valid(&left.quantity, zero)?;
        input_valid(&right.quantity, zero)?;
        Ok(zero)
    }

    fn wrap<C: Currency>(&self, quantity: C::Quantity) -> Amount<C> {
        Amount {
            stamp: self.stamp,
            quantity,
        }
    }
}

#[derive(Default)]
struct Registry {
    by_key: HashMap<CurrencyKey, TypeId>,
    by_type: HashMap<TypeId, RegistrationRecord>,
}

struct RegistrationRecord {
    type_id: TypeId,
    key: CurrencyKey,
    quantity_type: TypeId,
    zero: Box<dyn Any + Send + Sync>,
}

fn allocate_catalogue_stamp(counter: &AtomicU64) -> Result<u64, CurrencyError> {
    counter
        .try_update(AtomicOrdering::SeqCst, AtomicOrdering::SeqCst, |current| {
            if current == u64::MAX {
                None
            } else {
                Some(current + 1)
            }
        })
        .map_err(|_| CurrencyError::IdentityExhausted)
}

fn require_order(condition: bool) -> Result<(), CurrencyError> {
    if condition {
        Ok(())
    } else {
        Err(CurrencyError::QuantityLawViolation)
    }
}

fn checked_order<Q: Quantity>(left: &Q, right: &Q) -> Result<Ordering, CurrencyError> {
    let forward = left.cmp(right);
    let reverse = right.cmp(left);
    let equal_forward = left.eq(right);
    let equal_reverse = right.eq(left);
    require_order(
        forward == reverse.reverse()
            && equal_forward == equal_reverse
            && equal_forward == (forward == Ordering::Equal),
    )?;
    Ok(forward)
}

fn require_equal<Q: Quantity>(left: &Q, right: &Q) -> Result<(), CurrencyError> {
    require_order(checked_order(left, right)? == Ordering::Equal)
}

fn basic_valid<Q: Quantity>(quantity: &Q, zero: &Q) -> Result<bool, CurrencyError> {
    if !quantity.validate() {
        return Ok(false);
    }
    // Zero is an additive identity, not a minimum. Still probe Eq/Ord consistency.
    checked_order(quantity, zero)?;
    Ok(true)
}

fn output_valid<Q: Quantity>(quantity: &Q, zero: &Q) -> Result<(), CurrencyError> {
    require_order(basic_valid(quantity, zero)?)
}

fn check_witness<Q: Quantity>(
    witness: Option<Q>,
    expected: &Q,
    zero: &Q,
) -> Result<(), CurrencyError> {
    let value = witness.ok_or(CurrencyError::QuantityLawViolation)?;
    output_valid(&value, zero)?;
    require_equal(&value, expected)
}

fn identity_valid<Q: Quantity>(quantity: &Q, zero: &Q) -> Result<(), CurrencyError> {
    check_witness(quantity.checked_add(zero), quantity, zero)?;
    check_witness(zero.checked_add(quantity), quantity, zero)?;
    check_witness(quantity.checked_sub(zero), quantity, zero)?;
    check_witness(quantity.checked_sub(quantity), zero, zero)
}

fn input_valid<Q: Quantity>(quantity: &Q, zero: &Q) -> Result<(), CurrencyError> {
    if !basic_valid(quantity, zero)? {
        return Err(CurrencyError::InvalidAmount);
    }
    identity_valid(quantity, zero)
}

fn zero_probe<Q: Quantity>(zero: &Q) -> Result<(), CurrencyError> {
    output_valid(zero, zero)?;
    require_equal(zero, zero)?;
    check_witness(zero.checked_add(zero), zero, zero)?;
    check_witness(zero.checked_sub(zero), zero, zero)
}
