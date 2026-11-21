//! Private account transitions; identity, liveness and rule checks belong to callers.

use super::Result;
use crate::{Amount, BankError, BankHandle, BankOwner, Currency};
use roundo_life::ConceptBodyHandle;
use std::{
    any::{Any, TypeId},
    collections::{HashMap, HashSet},
};

pub(super) type Accounts = HashMap<AccountKey, Box<dyn Any + Send + Sync>>;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct AccountKey {
    pub(super) slot: u64,
    pub(super) generation: u64,
    pub(super) body: ConceptBodyHandle,
    pub(super) currency: TypeId,
}
impl AccountKey {
    fn new<C: Currency>(bank: BankHandle, owner: BankOwner) -> Self {
        Self {
            slot: bank.slot(),
            generation: bank.generation(),
            body: owner.body(),
            currency: TypeId::of::<C>(),
        }
    }
}

#[derive(Default)]
pub(super) struct AccountLedger {
    live_accounts: Accounts,
    detached_accounts: Accounts,
    owners: HashMap<ConceptBodyHandle, HashSet<AccountKey>>,
}
impl AccountLedger {
    pub(super) fn open<C: Currency>(
        &mut self,
        bank: BankHandle,
        owner: BankOwner,
        prepare_zero: impl FnOnce() -> Result<Amount<C>>,
    ) -> Result<()> {
        let key = AccountKey::new::<C>(bank, owner);
        if self.live_accounts.contains_key(&key) || self.detached_accounts.contains_key(&key) {
            return Err(BankError::AccountAlreadyOpen);
        }
        let zero = prepare_zero()?;
        self.live_accounts.insert(key, Box::new(zero));
        self.owners.entry(owner.body()).or_default().insert(key);
        Ok(())
    }

    pub(super) fn update_live<C: Currency>(
        &mut self,
        bank: BankHandle,
        owner: BankOwner,
        prepare_after: impl FnOnce(&Amount<C>) -> Result<(Amount<C>, Amount<C>)>,
    ) -> Result<()> {
        let key = AccountKey::new::<C>(bank, owner);
        let before =
            Self::amount::<C>(&self.live_accounts, &key).ok_or(BankError::AccountNotOpen)?;
        let (after, delta) = prepare_after(before)?;
        // Preserve the original replacement statement's old-box drop before delta.
        self.live_accounts.insert(key, Box::new(after));
        drop(delta);
        Ok(())
    }

    fn amount<'a, C: Currency>(accounts: &'a Accounts, key: &AccountKey) -> Option<&'a Amount<C>> {
        accounts.get(key).map(|value| {
            value
                .downcast_ref::<Amount<C>>()
                .expect("account currency invariant")
        })
    }

    pub(super) fn live<C: Currency>(
        &self,
        bank: BankHandle,
        owner: BankOwner,
    ) -> Option<&Amount<C>> {
        Self::amount::<C>(&self.live_accounts, &AccountKey::new::<C>(bank, owner))
    }

    pub(super) fn detached<C: Currency>(
        &self,
        bank: BankHandle,
        owner: BankOwner,
    ) -> Option<&Amount<C>> {
        Self::amount::<C>(&self.detached_accounts, &AccountKey::new::<C>(bank, owner))
    }

    pub(super) fn counts(&self) -> (usize, usize) {
        (self.live_accounts.len(), self.detached_accounts.len())
    }

    pub(super) fn detach_owner(&mut self, body: ConceptBodyHandle) {
        let Some(keys) = self.owners.remove(&body) else {
            return;
        };
        for key in keys {
            assert!(
                !self.detached_accounts.contains_key(&key),
                "detached key cannot be overwritten"
            );
            let amount = self
                .live_accounts
                .remove(&key)
                .expect("reverse index requires live account");
            self.detached_accounts.insert(key, amount);
        }
    }
}
