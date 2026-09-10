//! Versioned wire contracts shared by game domains and network transports.
//!
//! This crate owns serialized identities, DTOs, logical stream identities, and
//! protocol versions. It intentionally contains no socket, runtime, TLS, or ECS
//! implementation so domain modules can depend on contracts without depending
//! on a particular transport.

use roundo_toolbox::{UpdateVersion, macros::identifier};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const GAME_PROTOCOL_VERSION: u16 = 14;
pub const RESOURCE_PROTOCOL_VERSION: u16 = 2;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[repr(u8)]
pub enum StreamId {
    Stream0 = 0,
    Stream1 = 1,
}

impl StreamId {
    pub const ALL: [Self; 2] = [Self::Stream0, Self::Stream1];

    pub const fn from_wire(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Stream0),
            1 => Some(Self::Stream1),
            _ => None,
        }
    }
}

identifier!(ConnectionId);
identifier!(LocalCoordinateId);
identifier!(PlayerId);
identifier!(JoinableWorldId);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct UserId(pub i32);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct SessionId(pub i32);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct UserSession {
    pub user_id: UserId,
    pub session_id: SessionId,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ProtocolErrorCode {
    SessionRejected,
    UnexpectedMessage,
    UnsupportedProtocolVersion,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionInfo {
    pub user_session: UserSession,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum ClientMessage {
    JoinPublicSession,
    Ready {
        stream: StreamId,
        protocol_version: u16,
    },
    Game(ClientGameMessage),
    Resource(ClientResourceMessage),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum ServerMessage {
    SessionEstablished { info: SessionInfo },
    Error { code: ProtocolErrorCode },
    Game(ServerGameMessage),
    Resource(ServerResourceMessage),
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct ControllerCommand<Action> {
    pub sequence: u64,
    pub action: Action,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct Movement3DAction {
    pub translation_delta: [f32; 3],
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RotationSync {
    pub rotation: [f32; 4],
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DestroyBlockControllerAction;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlaceBlockControllerAction {
    pub voxel_id: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum PlayerControllerCommand {
    Movement3D(ControllerCommand<Movement3DAction>),
    SyncRotation(RotationSync),
    DestroyBlock(ControllerCommand<DestroyBlockControllerAction>),
    PlaceBlock(ControllerCommand<PlaceBlockControllerAction>),
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum SceneId {
    S0 {
        room_id: u64,
    },
    #[default]
    S1,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct PlayerState {
    pub player_id: PlayerId,
    pub scene_id: SceneId,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct NearbyPlayer {
    pub player_id: PlayerId,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct NearbyJoinableWorld {
    pub world_id: JoinableWorldId,
    pub name: String,
    pub translation: [f32; 3],
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct PresenceSnapshot {
    pub own_player_id: PlayerId,
    pub players: Vec<NearbyPlayer>,
    pub joinable_worlds: Vec<NearbyJoinableWorld>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ChunkId {
    pub local_coordinate_id: LocalCoordinateId,
    pub coordinate: [i64; 3],
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ChunkVersion {
    pub local_coordinate_id: LocalCoordinateId,
    pub coordinate: [i64; 3],
    pub version: UpdateVersion,
}

impl ChunkVersion {
    pub const fn id(self) -> ChunkId {
        ChunkId {
            local_coordinate_id: self.local_coordinate_id,
            coordinate: self.coordinate,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SerializedPayload(Vec<u8>);

impl SerializedPayload {
    pub fn encode<Value>(value: &Value) -> Result<Self, postcard::Error>
    where
        Value: Serialize,
    {
        postcard::to_allocvec(value).map(Self)
    }

    pub fn decode<Value>(&self) -> Result<Value, postcard::Error>
    where
        Value: DeserializeOwned,
    {
        postcard::from_bytes(&self.0)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum ClientGameMessage {
    UsePlayerController { command: PlayerControllerCommand },
}

impl ClientGameMessage {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::UsePlayerController { .. } => "UsePlayerController",
        }
    }
}

impl From<ClientGameMessage> for ClientMessage {
    fn from(message: ClientGameMessage) -> Self {
        Self::Game(message)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum ClientResourceMessage {
    RequestLocalCoordinateChunks { chunks: Vec<ChunkId> },
    SetChunkViewDistance { chunks: u16 },
}

impl ClientResourceMessage {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::RequestLocalCoordinateChunks { .. } => "RequestLocalCoordinateChunks",
            Self::SetChunkViewDistance { .. } => "SetChunkViewDistance",
        }
    }
}

impl From<ClientResourceMessage> for ClientMessage {
    fn from(message: ClientResourceMessage) -> Self {
        Self::Resource(message)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum ServerGameMessage {
    PlayerState { state: PlayerState },
    PresenceSnapshot { snapshot: PresenceSnapshot },
}

impl ServerGameMessage {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::PlayerState { .. } => "PlayerState",
            Self::PresenceSnapshot { .. } => "PresenceSnapshot",
        }
    }
}

impl From<ServerGameMessage> for ServerMessage {
    fn from(message: ServerGameMessage) -> Self {
        Self::Game(message)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum ServerResourceMessage {
    LocalCoordinateSpawned {
        local_coordinate_id: LocalCoordinateId,
    },
    LocalCoordinateDespawned {
        local_coordinate_id: LocalCoordinateId,
    },
    LocalCoordinateChunkVersions {
        chunks: Vec<ChunkVersion>,
    },
    LocalCoordinateChunk {
        chunk: ChunkVersion,
        edge_length: u16,
        svo: SerializedPayload,
    },
    LocalCoordinateChunkUnloaded {
        local_coordinate_id: LocalCoordinateId,
        coordinate: [i64; 3],
    },
}

impl ServerResourceMessage {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::LocalCoordinateSpawned { .. } => "LocalCoordinateSpawned",
            Self::LocalCoordinateDespawned { .. } => "LocalCoordinateDespawned",
            Self::LocalCoordinateChunkVersions { .. } => "LocalCoordinateChunkVersions",
            Self::LocalCoordinateChunk { .. } => "LocalCoordinateChunk",
            Self::LocalCoordinateChunkUnloaded { .. } => "LocalCoordinateChunkUnloaded",
        }
    }
}

impl From<ServerResourceMessage> for ServerMessage {
    fn from(message: ServerResourceMessage) -> Self {
        Self::Resource(message)
    }
}
