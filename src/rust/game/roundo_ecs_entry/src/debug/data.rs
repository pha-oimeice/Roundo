#[derive(Clone, Debug, PartialEq)]

pub enum DebugOperation {
    List {
        offset: u32,
    },
    Inspect {
        body: String,
    },
    CreateBody {
        values: DebugBodyValues,
    },
    /// Creates the explicit diagnostic recipe: three State policies and two
    /// Event interfaces, without roles, controllers, or registered participants.
    CreateLife,
    SetAttribute {
        body: String,
        attribute: u8,
        value: DebugValue,
    },
    Relation {
        body: String,
        life: String,
        data_type: String,
        action: DebugRelationAction,
    },
    Inventory(DebugInventoryOperation),
    Item(DebugItemOperation),
    /// Issues one new identity, composes the explicitly selected sources, and
    /// assigns direct custody to an existing inventory (including Public).
    CreateItem {
        inventory: String,
        placement_reach: Option<f32>,
        destruction_reach: Option<f32>,
    },
}

/// Native custody operations; item IDs are existing identities, never issued here.
#[derive(Clone, Debug, PartialEq)]

pub enum DebugInventoryOperation {
    List {
        body: Option<String>,
        offset: u32,
    },
    Inspect {
        inventory: String,
        offset: u32,
    },
    Create {
        body: String,
    },
    Remove {
        inventory: String,
    },
    Transfer {
        source: String,
        target: String,
        item: String,
    },
    Spill {
        inventory: String,
        item: String,
    },
}

/// Diagnostic routing names bind to existing exact native item instances.
/// They are not business IDs, serialized grants or Player authorization.
#[derive(Clone, Debug, PartialEq)]

pub enum DebugItemOperation {
    List {
        inventory: String,
        offset: u32,
    },
    /// Rejects an item that has left this inventory or whose instance has ended.
    Remove {
        inventory: String,
        selector: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]

pub struct DebugBodyValues {
    pub health: i32,
    pub intent: i32,
    pub execution: i32,
    pub record: i32,
    pub continuity: f32,
    pub change: f32,
    pub cycle: f32,
    pub eternity: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]

pub enum DebugValue {
    Integer(i32),
    Unorm(f32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]

pub enum DebugRelationAction {
    DeclareReader,
    DeclareWriter,
    ApproveReader,
    ApproveWriter,
    RegisterReader,
    RegisterWriter,
    UnregisterReader,
    UnregisterWriter,
    /// Only an absent State Slot can be created, with an explicitly approved
    /// immutable origin and an uninitialized value allowed by its native policy.
    CreateSlot,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DebugRequest {
    pub request_id: String,
    pub operation: DebugOperation,
}

#[derive(bevy::prelude::Message, Clone, Debug, PartialEq)]
pub struct DebugReply {
    pub request_id: String,
    pub result: Result<DebugData, DebugFailure>,
}

#[derive(Clone, Debug, PartialEq)]

pub enum DebugData {
    Objects {
        bodies: Vec<String>,
        /// Read-only descriptions for this page; selectors remain the operation targets.
        body_summaries: Vec<DebugBodySummary>,
        lives: Vec<String>,
        total_bodies: u32,
        total_lives: u32,
        next_offset: Option<u32>,
        data_types: Vec<DebugDataType>,
        truncated: bool,
    },
    Body {
        body: String,

        summary: Option<DebugBodySummary>,
        attributes: Vec<DebugAttribute>,
        relations: Vec<DebugRelation>,
        relations_truncated: bool,
    },
    Created {
        body: Option<String>,
        life: Option<String>,
    },
    Done,
    Inventories {
        inventories: Vec<DebugInventorySummary>,
        total: u32,
        next_offset: Option<u32>,
        truncated: bool,
    },
    Inventory {
        summary: DebugInventorySummary,
        /// Full u128 decimal strings, numerically ordered; direct members only.
        items: Vec<String>,
        total_items: u32,
        next_offset: Option<u32>,
    },
    InventoryCreated {
        inventory: String,
    },
    ItemCreated {
        /// Full u128 decimal identity; never a JavaScript number.
        item: String,
        inventory: DebugInventorySummary,
        /// Read back from the stored sources, not echoed from the request.
        placement_reach: Option<f32>,
        destruction_reach: Option<f32>,
    },
    Items {
        inventory: DebugInventorySummary,
        /// Direct members, ordered by numeric business ID.
        items: Vec<DebugItemSummary>,
        total_items: u32,
        next_offset: Option<u32>,
    },
    /// Emitted only after the native destruction and host cleanup have completed.
    ItemRemoved {
        item: String,
        selector: String,
    },
}

/// A displayed business ID paired with a diagnostic name for its exact instance.
#[derive(Clone, Debug, PartialEq)]
pub struct DebugItemSummary {
    /// Full u128 decimal text; never convert to a JavaScript number.
    pub item: String,
    pub selector: String,
}

/// Inventory routing selector and freshly checked native ownership facts.
#[derive(Clone, Debug, PartialEq)]
pub struct DebugInventorySummary {
    pub inventory: String,
    pub owner: DebugBodySummary,
    pub is_public: bool,
    /// Direct member count, not Capacity or Occupancy.
    pub item_count: u32,
}

/// Observed native role tags, not inferred behavior, approval or control rights.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]

pub enum DebugBodyRole {
    Intent,
    Execution,
    Record,
}

/// Current diagnostic facts, not a new identity, ownership or membership authority.
#[derive(Clone, Debug, PartialEq)]
pub struct DebugBodySummary {
    pub body: String,
    pub roles: Vec<DebugBodyRole>,
    pub is_public: bool,
    /// The live motion recipe's association, not exclusive Life membership or data access.
    pub recipe_life: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DebugDataType {
    pub key: String,
    pub shape: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DebugAttribute {
    pub id: u8,
    pub name: String,
    pub value: Option<DebugValue>,
    pub writable: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DebugRelation {
    pub life: String,
    pub data_type: String,
    pub shape: String,
    pub reader_declared: bool,
    pub writer_declared: bool,
    pub reader_registered: bool,
    pub writer_registered: bool,
    pub immutable_origin: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DebugFailure {
    pub code: String,
    pub message: String,
}
impl DebugFailure {
    pub fn new(code: &str, message: impl ToString) -> Self {
        Self {
            code: code.into(),
            message: message.to_string(),
        }
    }
}
