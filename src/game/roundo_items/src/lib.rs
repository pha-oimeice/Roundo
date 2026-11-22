//! Deterministic item identity issuance from host-reserved inclusive ranges.
//!
//! This capability creates only `roundo_inventory::ItemId` values: no concrete
//! item/type data, Entity, inventory, gameplay rule, World binding or persistence.
//! [`ItemIdRange`] validates structure, not global authority. Applications without
//! a globally reserved range cannot issue identities through this capability.
//!
//! # Required host contract
//!
//! Before [`ItemIssuer::claim_host_reserved`], the host must confirm with its
//! global authority that the **entire** range has never been issued or reserved
//! for another issuer, and exclusively reserve it. All other processes, restarts
//! and authorities must use disjoint ranges that are never reclaimed.
//!
//! Claims here are in-memory only. Before claiming, the host must complete its
//! external non-reuse record; after exit or restart it must never reuse this
//! process's old ranges, including unissued tails. Starting automatically at zero
//! is not valid. Correct reservation and recording are host responsibilities;
//! this framework does not implement database transactions or global allocation,
//! networking, persistence, random IDs or a particular namespace encoding.
//!
//! External `roundo_inventory::ItemId::from_u128` imports a business number; copying
//! it does not issue a new item or prove an exact runtime instance. The issuer's
//! non-reuse policy is not a lifecycle requirement: after complete destruction,
//! runtime publication can reuse a number with a fresh [`ItemInstanceId`]. The
//! host must prevent collisions among live items and reject stale number-only
//! operations. This issuer does not automatically recycle destroyed numbers.
//!
//! Under these prerequisites, issuance from different globally reserved ranges
//! cannot produce equal IDs; each issuer emits its range in order exactly once.
//! All issuers in this module in one process share permanent claims, rejecting
//! overlap even after drop, including unissued tails. This is deterministic,
//! not a UUID collision probability. Moves preserve the cursor; sharing an issuer
//! requires host synchronization (for example a Mutex), not copied issuer state.
//!
//! Host violations or independent allocators bypassing this registry are not
//! sandboxed. Rust cannot prove global claim facts at compile time; a range is
//! not a global uniqueness proof. Delivering this capability does not establish
//! production global reservations or verify cross-process/restart correctness.
//!
//! Claims take O(log historical claims) time and O(historical claims) retained
//! storage; history is never automatically reclaimed or capacity-limited.
//! Issuance takes O(1), without holding the claim lock. Recoverable errors leave
//! claims and issuer cursors unchanged; panic, abort, OOM and arbitrary external
//! destructor effects are outside this atomicity guarantee.
//!
//! # Mod item definitions
//!
//! [`ItemDefinitions`] parses immutable initial-value TOML directories or loads
//! the optional `game_data/item/registry.toml` from already discovered Mods.
//! Loading does not issue IDs, create instances, install runtime resources or
//! grant access. Resource-type selection remains explicit in the caller's Mod
//! catalog; production Server selection is not changed by this capability.
//!
//! [`ItemDefinitions::materialize`] produces independent [`DeclaredItemData`]
//! values sharing one native Type/schema. Definition names are not instance IDs.
//! [`DeclaredItemData::with_replaced_data`] only creates an owned copy; it does
//! not change runtime data or confer permission to do so.
//! [`ItemRuntimeSetup`] appends this native source to a host-provided builder,
//! preserving host registrations and schemas in an initially empty store. It
//! does not itself install resources or publish items. [`install_items`] installs
//! empty World-bound capabilities; trusted [`ItemPublisher`] systems publish
//! native data and authorization targets without issuing identities or grants.
//!
//! # Authorized runtime data
//!
//! An ItemId or schema is not a grant. Trusted host systems publish data and
//! issue [`ItemReadAccess`]/[`ItemModifyAccess`]; recipients pass the exact grant
//! to [`ItemReads`] or [`ItemWrites`]. Clones share revocation, not independent
//! authority. Owned copies carry no runtime permission.
//!
//! Publisher, administration, lifecycle and writes have conflicting resource
//! access; use separate ordinary systems in an explicit order. Publication and
//! initialization are not a single transaction. [`ItemLifecycle::destroy`] checks
//! exact instance identity and commits terminal custody/data cleanup together;
//! the host still coordinates its spatial representations and cached grants.
//!
//!
//! # Typed item data catalogue
//!
//! The data catalogue is built once with source-declared Rust payload types,
//! frozen into an [`ItemDataStore`], and then used with copyable [`ItemSchema`]
//! values. The store associates external [`roundo_inventory::ItemId`] values with
//! one current type payload and any number of independent specialized payloads.
//! It does not issue IDs, claim custody, follow a body lifecycle, or expose
//! `Any`/mutable payload access.
//!
//! Reads are synchronous callbacks over `&T`; callers copy the fields they need.
//! Failed `set_*` calls consume and drop the submitted payload rather than
//! returning it. The framework validates its own catalogue, shape and key
//! invariants, while source validators and payload interior mutability remain
//! trusted host code.

mod access;
mod catalogue;
mod data;
mod declared;
mod definitions;
mod ecs_adapter;
mod issuer;
mod runtime_setup;
mod store;

pub use access::{
    ItemAccessError, ItemDestructionError, ItemInstallError, ItemModifyAccess, ItemReadAccess,
};
pub use ecs_adapter::{
    ItemAdministration, ItemLifecycle, ItemPublisher, ItemReads, ItemWrites, install_items,
};
pub use issuer::{ItemIdRange, ItemIssueError, ItemIssuer};
/// The existing resource-access identity, not a new allocator or serialized ItemId.
pub use roundo_infrastructure::dsa::data_structure::resource_access::ResourceId as ItemInstanceId;

pub use catalogue::{ItemDataBuilder, ItemDataError, ItemSchema};
pub use data::{
    __private, ItemData, ItemDataKey, ItemDataShape, ItemTypeData, ItemTypeShape, SpecializedData,
    SpecializedShape,
};
pub use declared::DeclaredItemData;
pub use definitions::{
    ItemDefinitionLoadError, ItemDefinitionLoadErrorKind, ItemDefinitions, UnknownItemDefinition,
};
pub use runtime_setup::ItemRuntimeSetup;
pub use store::ItemDataStore;

pub(crate) use catalogue::FrozenCatalogue;
