//! Read-only custody projection. IDs are decimal strings at the JSON boundary;
//! no Factory or Inventory handles are disclosed or accepted from a player.
use crate::{ControllerBindingId, ControllerId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InventoryRequest {
    pub request_id: u64,
    pub controller_id: ControllerId,
    pub binding: ControllerBindingId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InventoryReply {
    pub request: InventoryRequest,
    pub result: Result<Vec<BodyInventories>, InventoryReadError>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryReadError {
    NotBound,
    Unavailable,
    TooLarge,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryBodyRole {
    Intent,
    Execution,
    Record,
}

/// Direct custody only. An empty outer list means this Body has no inventory;
/// an empty inner list means an existing inventory currently holds no items.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BodyInventories {
    pub role: InventoryBodyRole,
    pub inventories: Vec<Vec<String>>,
}
