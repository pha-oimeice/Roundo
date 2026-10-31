use crate::{
    AcquisitionRule, Amount, BankError, Currency, CurrencyKey, RuleCatalogue, RuleError,
    RuleSchema, RuleSource, StorageDirection, StorageRule, UseRule,
};
use std::{
    any::{Any, TypeId},
    collections::{HashMap, HashSet},
};

/// Catalogue-bound, mutable approval construction. No default allow exists.
/// Duplicate or foreign sources leave the configuration unchanged. Context is
/// owned even on Err and follows ordinary Drop; source effects are not sandboxed.
pub struct BankApprovalBuilder {
    stamp: u64,
    currencies: HashMap<TypeId, CurrencyApproval>,
    sources: HashSet<TypeId>,
}

/// Frozen bank configuration. Not Clone and not reusable balance-commit authority.
/// Each stored condition retains its typed token and owned, non-Clone context.
pub struct BankApproval {
    pub(crate) stamp: u64,
    pub(crate) currencies: HashMap<TypeId, CurrencyApproval>,
}

pub(crate) struct CurrencyApproval {
    pub(crate) key: CurrencyKey,
    pub(crate) acquisition: HashSet<TypeId>,
    pub(crate) uses: HashSet<TypeId>,
    storage: Vec<Box<dyn StorageCondition>>,
}
impl CurrencyApproval {
    fn new(key: CurrencyKey) -> Self {
        Self {
            key,
            acquisition: HashSet::new(),
            uses: HashSet::new(),
            storage: Vec::new(),
        }
    }

    pub(crate) fn evaluate<C: Currency>(
        &self,
        rules: &RuleCatalogue,
        before: &Amount<C>,
        delta: &Amount<C>,
        after: &Amount<C>,
        direction: StorageDirection,
    ) -> Result<(), BankError> {
        for binding in &self.storage {
            binding.evaluate(rules, before, delta, after, direction)?;
        }
        Ok(())
    }
}

trait StorageCondition: Send + Sync {
    fn evaluate(
        &self,
        rules: &RuleCatalogue,
        before: &dyn Any,
        delta: &dyn Any,
        after: &dyn Any,
        direction: StorageDirection,
    ) -> Result<(), BankError>;
}

struct StorageBinding<R: StorageRule> {
    schema: RuleSchema<R>,
    context: R::Context,
}
impl<R: StorageRule> StorageCondition for StorageBinding<R> {
    fn evaluate(
        &self,
        rules: &RuleCatalogue,
        before: &dyn Any,
        delta: &dyn Any,
        after: &dyn Any,
        direction: StorageDirection,
    ) -> Result<(), BankError> {
        let before = before
            .downcast_ref::<Amount<R::Currency>>()
            .expect("storage before currency invariant");
        let delta = delta
            .downcast_ref::<Amount<R::Currency>>()
            .expect("storage delta currency invariant");
        let after = after
            .downcast_ref::<Amount<R::Currency>>()
            .expect("storage after currency invariant");
        rules
            .evaluate_storage(self.schema, &self.context, before, delta, after, direction)
            .map_err(BankError::Rule)
    }
}

impl BankApprovalBuilder {
    pub fn new(rules: &RuleCatalogue) -> Self {
        Self {
            stamp: rules.stamp(),
            currencies: HashMap::new(),
            sources: HashSet::new(),
        }
    }

    fn check_catalogue(&self, rules: &RuleCatalogue) -> Result<(), BankError> {
        if self.stamp == rules.stamp() {
            Ok(())
        } else {
            Err(BankError::Rule(RuleError::WrongRuleCatalogue))
        }
    }

    fn check_source<R: RuleSource>(
        &self,
        rules: &RuleCatalogue,
        schema: RuleSchema<R>,
    ) -> Result<(), BankError> {
        self.check_catalogue(rules)?;
        rules
            .validate_rule_schema(schema)
            .map_err(BankError::Rule)?;
        if self.sources.contains(&TypeId::of::<R>()) {
            return Err(BankError::RuleAlreadyApproved);
        }
        Ok(())
    }

    pub fn approve_acquisition<R: AcquisitionRule>(
        &mut self,
        rules: &RuleCatalogue,
        schema: RuleSchema<R>,
    ) -> Result<(), BankError> {
        self.check_source(rules, schema)?;
        self.currencies
            .entry(TypeId::of::<R::Currency>())
            .or_insert_with(|| CurrencyApproval::new(schema.currency().key()))
            .acquisition
            .insert(TypeId::of::<R>());
        self.sources.insert(TypeId::of::<R>());
        Ok(())
    }

    pub fn approve_use<R: UseRule>(
        &mut self,
        rules: &RuleCatalogue,
        schema: RuleSchema<R>,
    ) -> Result<(), BankError> {
        self.check_source(rules, schema)?;
        self.currencies
            .entry(TypeId::of::<R::Currency>())
            .or_insert_with(|| CurrencyApproval::new(schema.currency().key()))
            .uses
            .insert(TypeId::of::<R>());
        self.sources.insert(TypeId::of::<R>());
        Ok(())
    }

    pub fn add_storage<R: StorageRule>(
        &mut self,
        rules: &RuleCatalogue,
        schema: RuleSchema<R>,
        context: R::Context,
    ) -> Result<(), BankError> {
        self.check_source(rules, schema)?;
        self.currencies
            .entry(TypeId::of::<R::Currency>())
            .or_insert_with(|| CurrencyApproval::new(schema.currency().key()))
            .storage
            .push(Box::new(StorageBinding::<R> { schema, context }));
        self.sources.insert(TypeId::of::<R>());
        Ok(())
    }

    /// Checks catalogue, nonempty configuration, then currencies by exact key.
    /// Each currency requires acquisition, use and at least one storage source.
    pub fn finish(self, rules: &RuleCatalogue) -> Result<BankApproval, BankError> {
        self.check_catalogue(rules)?;
        if self.currencies.is_empty() {
            return Err(BankError::EmptyApproval);
        }
        let mut ordered: Vec<_> = self.currencies.values().collect();
        ordered.sort_by_key(|currency| currency.key);
        for currency in ordered {
            if currency.acquisition.is_empty() || currency.uses.is_empty() {
                return Err(BankError::MissingOperationRule);
            }
            if currency.storage.is_empty() {
                return Err(BankError::MissingStorageRule);
            }
        }
        Ok(BankApproval {
            stamp: self.stamp,
            currencies: self.currencies,
        })
    }
}
