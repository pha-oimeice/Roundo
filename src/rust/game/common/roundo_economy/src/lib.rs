//! Typed currency registration and validated exact signed quantity calculations.
//!
//! Typed quantity calculations, pure rule evaluation and trusted local banks.
//! [`Banks`] validates the current World and exact Life Body on every live access.
//! There are no concrete currencies/models or persistence. Configuration and pure
//! evaluation are not reusable balance-commit authority. Negative balances/deltas
//! are allowed by signed sources; transaction permission belongs to approved rules,
//! not a built-in nonnegative balance requirement. Finite probes do not
//! prove source laws and are not a host sandbox.

//! Schemas fix the currency and quantity representation. Amounts and schemas
//! have private fields; Amount has no Clone, Default or standard-addition bypass.
//! Sources require a unique declaration. Freeze consumes registration and the
//! frozen catalogue cannot register further sources.
//!

mod bank_approval;
mod bank_error;
mod bank_identity;
mod bank_store;
mod catalogue;
mod currency;
mod life_adapter;
mod quantity;
mod rules;

pub use bank_approval::{BankApproval, BankApprovalBuilder};
pub use bank_error::BankError;
pub use bank_identity::{BankHandle, BankOwner};
pub use catalogue::{Amount, CurrencyBuilder, CurrencyCatalogue, CurrencyError, CurrencySchema};
pub use currency::{Currency, CurrencyKey};
pub use life_adapter::{Banks, EconomyCounts, EconomyInstallError, install_economy};
pub use quantity::Quantity;
pub use rules::{
    AcquisitionKind, AcquisitionRule, RuleBuilder, RuleCatalogue, RuleError, RuleKey, RuleKind,
    RuleRejection, RuleSchema, RuleSource, StorageDirection, StorageKind, StorageRule, StorageView,
    UseKind, UseRule,
};

#[doc(hidden)]
pub use currency::__private;
