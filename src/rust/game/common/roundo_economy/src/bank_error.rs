use crate::{CurrencyError, RuleError};
use std::fmt;

/// Bank identity, configuration and account errors. Currency and rule failures
/// retain their original error semantics; source effects are not rolled back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BankError {
    WrongDomain,
    InvalidBank,
    OwnerNotLive,
    CurrencyNotApproved,
    RuleNotApproved,
    RuleAlreadyApproved,
    EmptyApproval,
    MissingOperationRule,
    MissingStorageRule,
    AccountAlreadyOpen,
    AccountNotOpen,
    IdentityExhausted,
    Currency(CurrencyError),
    Rule(RuleError),
}
impl fmt::Display for BankError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongDomain => f.write_str("bank identity belongs to another domain"),
            Self::InvalidBank => f.write_str("bank identity is invalid or stale"),
            Self::OwnerNotLive => f.write_str("bank owner is not live"),
            Self::CurrencyNotApproved => f.write_str("currency is not approved by this bank"),
            Self::RuleNotApproved => f.write_str("rule source is not approved by this bank"),
            Self::RuleAlreadyApproved => f.write_str("rule source is already approved"),
            Self::EmptyApproval => f.write_str("bank approval is empty"),
            Self::MissingOperationRule => f.write_str("currency lacks acquisition or use approval"),
            Self::MissingStorageRule => f.write_str("currency lacks a storage condition"),
            Self::AccountAlreadyOpen => f.write_str("account is already open or retained detached"),
            Self::AccountNotOpen => f.write_str("live account is not open"),
            Self::IdentityExhausted => f.write_str("bank identity space is exhausted"),
            Self::Currency(error) => error.fmt(f),
            Self::Rule(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for BankError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Currency(error) => Some(error),
            Self::Rule(error) => Some(error),
            _ => None,
        }
    }
}
