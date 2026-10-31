//! Explicit diagnostic protocol, separate from gameplay control capabilities.
//!
//! Selectors are server-runtime-issued diagnostic names, never serialized
//! Factory handles or ECS entities. Client caches are scoped to the active
//! connection epoch. Opening a UI does not authorize an operation.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
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
    /// Creates the explicit diagnostic recipe: three State policies and three
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
}

/// Native custody operations; item IDs are existing identities, never issued here.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
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

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DebugValue {
    Integer(i32),
    Unorm(f32),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DebugRequest {
    pub request_id: String,
    pub operation: DebugOperation,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, JsonSchema)]
pub struct DebugReply {
    pub request_id: String,
    pub result: Result<DebugData, DebugFailure>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DebugData {
    Objects {
        bodies: Vec<String>,
        /// Read-only descriptions for this page; selectors remain the operation targets.
        #[serde(default)]
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
        #[serde(default)]
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
}

/// Inventory routing selector and freshly checked native ownership facts.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, JsonSchema)]
pub struct DebugInventorySummary {
    pub inventory: String,
    pub owner: DebugBodySummary,
    pub is_public: bool,
    /// Direct member count, not Capacity or Occupancy.
    pub item_count: u32,
}

/// Observed native role tags, not inferred behavior, approval or control rights.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DebugBodyRole {
    Intent,
    Execution,
    Record,
}

/// Current diagnostic facts, not a new identity, ownership or membership authority.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, JsonSchema)]
pub struct DebugBodySummary {
    pub body: String,
    pub roles: Vec<DebugBodyRole>,
    pub is_public: bool,
    /// The live motion recipe's association, not exclusive Life membership or data access.
    pub recipe_life: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, JsonSchema)]
pub struct DebugDataType {
    pub key: String,
    pub shape: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, JsonSchema)]
pub struct DebugAttribute {
    pub id: u8,
    pub name: String,
    pub value: Option<DebugValue>,
    pub writable: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, JsonSchema)]
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, JsonSchema)]
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
