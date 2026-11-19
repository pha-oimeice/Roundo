use roundo_inventory::ItemId;
use std::{
    collections::BTreeMap,
    fmt,
    sync::{Mutex, OnceLock},
};

/// An owned inclusive range with `first <= last`, including zero and `u128::MAX`.
/// Copying this structural value does not reserve identities or confer global
/// authority; see the crate's required host contract. Endpoints never change.
///
/// Private fields prevent bypassing range validation:
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ItemIdRange {
    first: u128,
    last: u128,
}

impl ItemIdRange {
    /// Validates a closed range without reserving it. The only failure is
    /// [`ItemIssueError::InvalidRange`] when `first > last`; singleton and full
    /// `u128` ranges are valid. No state is changed.
    pub const fn try_new(first: u128, last: u128) -> Result<Self, ItemIssueError> {
        if first > last {
            Err(ItemIssueError::InvalidRange)
        } else {
            Ok(Self { first, last })
        }
    }

    /// Returns the inclusive first endpoint of this structural range value.
    pub const fn first(self) -> u128 {
        self.first
    }

    /// Returns the inclusive last endpoint of this structural range value.
    pub const fn last(self) -> u128 {
        self.last
    }
}

/// Exclusive, non-copyable issuance cursor over one permanently claimed range.
/// Moving it preserves progress; dropping it never releases the claim or unused
/// tail. Exhaustion is permanent. Share only with host synchronization, not by
/// cloning state. No World, item type or global reservation proof is stored.
///
/// Issuer state cannot be cloned:
#[derive(Debug)]
pub struct ItemIssuer {
    next: Option<u128>,
    last: u128,
}

impl ItemIssuer {
    /// Permanently claims the entire host-reserved range in this module's one
    /// process-local registry, returning an exclusive cursor starting at `first`.
    /// The host must first fulfill the crate's global reservation/non-reuse
    /// contract, including external recording before claim and across restarts.
    /// This method does not allocate globally or verify external authority.
    ///
    /// Overlap, including a shared endpoint, returns
    /// [`ItemIssueError::RangeAlreadyClaimed`]. A poisoned global Mutex returns
    /// [`ItemIssueError::RegistryUnavailable`] without recovering it. Either Err
    /// leaves all claims and existing cursors unchanged. Success is immediately
    /// visible to later claims, even after this issuer is dropped.
    ///
    /// The Mutex blocks only during claim; concurrent overlapping claims have
    /// one winner with unspecified order. No callbacks, I/O, thread creation or
    /// ECS access occur. Time is O(log historical claims); permanent storage is
    /// O(historical claims), without automatic reclamation or a capacity limit.
    /// Panic/abort/OOM/external destructor effects are not rolled back.
    pub fn claim_host_reserved(range: ItemIdRange) -> Result<Self, ItemIssueError> {
        claim_with_registry(
            CLAIMS.get_or_init(|| Mutex::new(ClaimRegistry::default())),
            range,
        )
    }

    /// Immediately emits the next identity in ascending order, including both
    /// endpoints, without changing custody or associated data. The returned
    /// owned ID may be copied/imported without issuing another item.
    ///
    /// After the last identity, every call returns
    /// [`ItemIssueError::IdentityExhausted`] without changing the cursor. This is
    /// range exhaustion, not global `u128` exhaustion. `u128::MAX` never wraps.
    /// Uses exclusive access in O(1), without the registry lock or external work.
    pub fn issue(&mut self) -> Result<ItemId, ItemIssueError> {
        let value = self.next.ok_or(ItemIssueError::IdentityExhausted)?;
        self.next = if value == self.last {
            None
        } else {
            Some(
                value
                    .checked_add(1)
                    .expect("validated range has a successor"),
            )
        };
        Ok(ItemId::from_u128(value))
    }
}

/// Recoverable issuance errors; none leave a new claim or advance a cursor.
/// Panic/abort/OOM and external effects are outside this guarantee.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemIssueError {
    /// The requested first endpoint exceeds the last; only range construction
    /// returns this error.
    InvalidRange,
    /// A permanent process-local claim intersects the entire requested range,
    /// including shared endpoints, containment or repetition.
    RangeAlreadyClaimed,
    /// The claim Mutex is poisoned. Claims fail closed without poison recovery.
    RegistryUnavailable,
    /// This issuer has emitted its whole range, not necessarily all global IDs.
    IdentityExhausted,
}

impl fmt::Display for ItemIssueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRange => "item ID range first endpoint exceeds last endpoint",
            Self::RangeAlreadyClaimed => "item ID range overlaps a permanent process-local claim",
            Self::RegistryUnavailable => "item ID claim registry is poisoned",
            Self::IdentityExhausted => "item issuer range is exhausted",
        })
    }
}

impl std::error::Error for ItemIssueError {}

static CLAIMS: OnceLock<Mutex<ClaimRegistry>> = OnceLock::new();

#[derive(Debug, Default)]
struct ClaimRegistry {
    claims: BTreeMap<u128, u128>,
}

impl ClaimRegistry {
    fn claim(&mut self, range: ItemIdRange) -> Result<ItemIssuer, ItemIssueError> {
        if self
            .claims
            .range(..=range.first)
            .next_back()
            .is_some_and(|(_, last)| *last >= range.first)
            || self
                .claims
                .range(range.first..)
                .next()
                .is_some_and(|(first, _)| *first <= range.last)
        {
            return Err(ItemIssueError::RangeAlreadyClaimed);
        }
        self.claims.insert(range.first, range.last);
        Ok(ItemIssuer {
            next: Some(range.first),
            last: range.last,
        })
    }
}

fn claim_with_registry(
    registry: &Mutex<ClaimRegistry>,
    range: ItemIdRange,
) -> Result<ItemIssuer, ItemIssueError> {
    registry
        .lock()
        .map_err(|_| ItemIssueError::RegistryUnavailable)?
        .claim(range)
}
