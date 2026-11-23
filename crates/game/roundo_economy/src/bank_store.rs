//! Private ledger only. No Life liveness check or public balance authority.
//! Recoverable failures preserve rows and indexes, not source effects/panic/OOM.

use crate::{
    AcquisitionRule, BankApproval, BankError, BankHandle, BankOwner, Currency, CurrencySchema,
    RuleCatalogue, RuleError, RuleSchema, RuleSource, StorageDirection, UseRule,
    bank_approval::CurrencyApproval, bank_identity::BankDomain,
};
use bevy::{ecs::world::WorldId, prelude::Resource};
use roundo_life::ConceptBodyHandle;
use std::{any::TypeId, collections::HashMap};

mod account_ledger;
use account_ledger::AccountLedger;

type Result<T> = std::result::Result<T, BankError>;

struct Bank {
    generation: u64,
    approval: BankApproval,
}
#[derive(Resource)]
pub(crate) struct BankStore {
    rules: RuleCatalogue,
    domain: BankDomain,
    next_slot: u64,
    banks: HashMap<u64, Bank>,
    ledger: AccountLedger,
}
impl BankStore {
    pub(crate) fn try_new(world: WorldId, rules: RuleCatalogue) -> Result<Self> {
        Ok(Self {
            rules,
            domain: BankDomain::try_new(world)?,
            next_slot: 1,
            banks: HashMap::new(),
            ledger: AccountLedger::default(),
        })
    }
    pub(crate) fn domain(&self) -> BankDomain {
        self.domain
    }
    pub(crate) fn check_world(&self, world: WorldId) -> Result<()> {
        self.domain.check_world(world)
    }
    pub(crate) fn counts(&self) -> (usize, usize, usize) {
        let (live, detached) = self.ledger.counts();
        (self.banks.len(), live, detached)
    }
    fn bank(domain: BankDomain, banks: &HashMap<u64, Bank>, bank: BankHandle) -> Result<&Bank> {
        if bank.domain() != domain {
            return Err(BankError::WrongDomain);
        }
        banks
            .get(&bank.slot())
            .filter(|entry| entry.generation == bank.generation())
            .ok_or(BankError::InvalidBank)
    }
    fn check_owner(&self, owner: BankOwner) -> Result<()> {
        if owner.domain() == self.domain {
            Ok(())
        } else {
            Err(BankError::WrongDomain)
        }
    }
    pub(crate) fn preflight_currency<C: Currency>(
        &self,
        bank: BankHandle,
        owner: BankOwner,
        schema: CurrencySchema<C>,
    ) -> Result<()> {
        Self::bank(self.domain, &self.banks, bank)?;
        self.check_owner(owner)?;
        self.rules
            .currencies()
            .validate_schema(schema)
            .map_err(BankError::Currency)
    }
    pub(crate) fn preflight_rule<R: RuleSource>(
        &self,
        bank: BankHandle,
        owner: BankOwner,
        schema: RuleSchema<R>,
    ) -> Result<()> {
        Self::bank(self.domain, &self.banks, bank)?;
        self.check_owner(owner)?;
        self.rules
            .validate_rule_schema(schema)
            .map_err(BankError::Rule)
    }
    fn currency<C: Currency>(
        domain: BankDomain,
        banks: &HashMap<u64, Bank>,
        bank: BankHandle,
    ) -> Result<&CurrencyApproval> {
        Self::bank(domain, banks, bank)?
            .approval
            .currencies
            .get(&TypeId::of::<C>())
            .ok_or(BankError::CurrencyNotApproved)
    }
    pub(crate) fn create(&mut self, approval: BankApproval) -> Result<BankHandle> {
        if approval.stamp != self.rules.stamp() {
            return Err(BankError::Rule(RuleError::WrongRuleCatalogue));
        }
        let next = self
            .next_slot
            .checked_add(1)
            .ok_or(BankError::IdentityExhausted)?;
        let handle = BankHandle::new(self.domain, self.next_slot, 1);
        assert!(
            !self.banks.contains_key(&handle.slot()),
            "bank slot never reused"
        );
        self.banks.insert(
            handle.slot(),
            Bank {
                generation: 1,
                approval,
            },
        );
        self.next_slot = next;
        Ok(handle)
    }

    pub(crate) fn open_account<C: Currency>(
        &mut self,
        bank: BankHandle,
        owner: BankOwner,
        schema: CurrencySchema<C>,
    ) -> Result<()> {
        self.preflight_currency(bank, owner, schema)?;
        let config = Self::currency::<C>(self.domain, &self.banks, bank)?;
        let rules = &self.rules;
        self.ledger.open(bank, owner, || {
            let zero = rules
                .currencies()
                .zero(schema)
                .map_err(BankError::Currency)?;
            config.evaluate(rules, &zero, &zero, &zero, StorageDirection::Initialize)?;
            Ok(zero)
        })
    }

    pub(crate) fn with_balance<C: Currency, T>(
        &self,
        bank: BankHandle,
        owner: BankOwner,
        schema: CurrencySchema<C>,
        callback: impl for<'a> FnOnce(&'a C::Quantity) -> T,
    ) -> Result<T> {
        self.preflight_currency(bank, owner, schema)?;
        Self::currency::<C>(self.domain, &self.banks, bank)?;
        let value = self
            .ledger
            .live::<C>(bank, owner)
            .ok_or(BankError::AccountNotOpen)?;
        self.rules
            .currencies()
            .compare(value, value)
            .map_err(BankError::Currency)?;
        Ok(callback(value.as_quantity()))
    }

    pub(crate) fn acquire<R: AcquisitionRule>(
        &mut self,
        bank: BankHandle,
        owner: BankOwner,
        schema: RuleSchema<R>,
        context: &R::Context,
    ) -> Result<()> {
        self.preflight_rule(bank, owner, schema)?;
        let config = Self::currency::<R::Currency>(self.domain, &self.banks, bank)?;
        if !config.acquisition.contains(&TypeId::of::<R>()) {
            return Err(BankError::RuleNotApproved);
        }
        let rules = &self.rules;
        self.ledger.update_live(bank, owner, |before| {
            let delta = rules
                .evaluate_acquisition(schema, context, before)
                .map_err(BankError::Rule)?;
            let after = rules
                .currencies()
                .checked_add(before, &delta)
                .map_err(BankError::Currency)?;
            config.evaluate(rules, before, &delta, &after, StorageDirection::Acquire)?;
            Ok((after, delta))
        })
    }
    pub(crate) fn use_currency<R: UseRule>(
        &mut self,
        bank: BankHandle,
        owner: BankOwner,
        schema: RuleSchema<R>,
        context: &R::Context,
    ) -> Result<()> {
        self.preflight_rule(bank, owner, schema)?;
        let config = Self::currency::<R::Currency>(self.domain, &self.banks, bank)?;
        if !config.uses.contains(&TypeId::of::<R>()) {
            return Err(BankError::RuleNotApproved);
        }
        let rules = &self.rules;
        self.ledger.update_live(bank, owner, |before| {
            let delta = rules
                .evaluate_use(schema, context, before)
                .map_err(BankError::Rule)?;
            let after = rules
                .currencies()
                .checked_sub(before, &delta)
                .map_err(BankError::Currency)?;
            config.evaluate(rules, before, &delta, &after, StorageDirection::Use)?;
            Ok((after, delta))
        })
    }
    pub(crate) fn with_detached_balance<C: Currency, T>(
        &self,
        bank: BankHandle,
        owner: BankOwner,
        schema: CurrencySchema<C>,
        callback: impl for<'a> FnOnce(&'a C::Quantity) -> T,
    ) -> Result<Option<T>> {
        self.preflight_currency(bank, owner, schema)?;
        Self::currency::<C>(self.domain, &self.banks, bank)?;
        let Some(value) = self.ledger.detached::<C>(bank, owner) else {
            return Ok(None);
        };
        self.rules
            .currencies()
            .compare(value, value)
            .map_err(BankError::Currency)?;
        Ok(Some(callback(value.as_quantity())))
    }
    pub(crate) fn detach_owner(&mut self, body: ConceptBodyHandle) {
        self.ledger.detach_owner(body);
    }
}
