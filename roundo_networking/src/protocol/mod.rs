//! Versioned wire schema for the TLS game connection.

use serde::{Deserialize, Serialize};

pub const GAME_PROTOCOL_VERSION: u16 = 2;

macro_rules! identifier {
    ($name:ident) => {
        #[derive(
            Clone,
            Copy,
            Debug,
            Default,
            Deserialize,
            Eq,
            Hash,
            Ord,
            PartialEq,
            PartialOrd,
            Serialize,
        )]
        pub struct $name(pub u64);
    };
}

identifier!(ConnectionId);
identifier!(ControllerId);
identifier!(CharacterId);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct UserId(pub i32);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct SessionId(pub i32);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct UserSession {
    pub user_id: UserId,
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConnectionToken(String);

impl ConnectionToken {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ProtocolErrorCode {
    AuthenticationRejected,
    UnexpectedMessage,
    UnsupportedProtocolVersion,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AuthenticationInfo {
    pub user_session: UserSession,
}

/// Every message a client can send. Authentication and gameplay share one
/// versioned schema, while [`crate::session`] enforces their ordering.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum ClientMessage {
    Authenticate { connection_token: ConnectionToken },
    Ready { protocol_version: u16 },
    Game(ClientGameMessage),
}

/// Every message a server can send.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum ServerMessage {
    Authenticated { info: AuthenticationInfo },
    Error { code: ProtocolErrorCode },
    Game(ServerGameMessage),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum ControllerKind {
    Locomotion,
    View,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum ControllerAccessPolicy {
    Exclusive,
    Shared,
    ReadOnly,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum ControllerScope {
    CharacterLocomotion { character_id: CharacterId },
    View,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ControllerDescriptor {
    pub controller_id: ControllerId,
    pub kind: ControllerKind,
    pub access_policy: ControllerAccessPolicy,
    pub scope: ControllerScope,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct LocomotionInput {
    pub sequence: u64,
    pub world_direction: [f32; 3],
    pub jump: bool,
    pub sprint: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ViewInput {
    pub sequence: u64,
    pub translation: [f32; 3],
    pub yaw_delta: f32,
    pub pitch_delta: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum ControllerInput {
    Locomotion(LocomotionInput),
    View(ViewInput),
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ControllerCameraState {
    pub translation: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct CharacterSnapshot {
    pub character_id: CharacterId,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum ClientGameMessage {
    RequestCharacterControl {
        character_id: CharacterId,
    },
    RequestController {
        controller_id: ControllerId,
    },
    ReleaseController {
        controller_id: ControllerId,
    },
    ControllerInput {
        controller_id: ControllerId,
        input: ControllerInput,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum ServerGameMessage {
    CharacterSnapshot {
        snapshot: CharacterSnapshot,
    },
    CharacterControlGranted {
        character_id: CharacterId,
    },
    StaticVoxelChunk {
        coordinate: [i64; 3],
        edge_length: u16,
        voxels: Vec<u16>,
    },
    StaticVoxelChunkUnloaded {
        coordinate: [i64; 3],
    },
    ControllerGranted {
        controller: ControllerDescriptor,
    },
    ControllerRevoked {
        controller_id: ControllerId,
    },
    ViewCameraState {
        controller_id: ControllerId,
        state: ControllerCameraState,
    },
}
