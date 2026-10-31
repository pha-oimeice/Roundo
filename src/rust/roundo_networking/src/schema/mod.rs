//! Wire messages for the standalone echo protocol.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// Client payload correlated by a monotonically assigned sequence.
pub struct EchoTestRequest {
    pub sequence: u32,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// Server echo of the original sequence and text payload.
pub struct EchoTestResponse {
    pub sequence: u32,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ClientMessage {
    EchoTest(EchoTestRequest),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ServerMessage {
    EchoTest(EchoTestResponse),
}
