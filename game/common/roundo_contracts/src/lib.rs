//! Versioned wire contracts shared by game domains and network transports.
//!
//! This crate owns serialized identities, DTOs, logical stream identities, and
//! protocol versions. It intentionally contains no socket, runtime, TLS, or ECS
//! implementation so domain modules can depend on contracts without depending
//! on a particular transport.

use roundo_toolbox::{UpdateVersion, macros::identifier};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Exact stream-0 application version required during session setup.
pub const GAME_PROTOCOL_VERSION: u16 = 15;
/// Exact stream-1 application version required during session setup.
pub const RESOURCE_PROTOCOL_VERSION: u16 = 2;

/// Logical QUIC stream role encoded as one wire byte during stream pairing.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[repr(u8)]
pub enum StreamId {
    /// Session negotiation and latency-sensitive game traffic.
    Stream0 = 0,
    /// Chunk versions and resource payload traffic.
    Stream1 = 1,
}

impl StreamId {
    /// Both logical stream roles in wire-ID order.
    pub const ALL: [Self; 2] = [Self::Stream0, Self::Stream1];

    /// Decodes a wire role, returning `None` for unknown values.
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

/// Server-confirmed pairing of a user identity and game-session identity.
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
    IncompatibleResources,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionInfo {
    pub user_session: UserSession,
}

/// Digest of the immutable Mod resources whose compact identities cross the
/// network. Session establishment rejects peers with a different digest.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ResourceCatalogFingerprint(pub [u8; 32]);

/// Top-level client-to-server envelope shared by both logical streams.
///
/// Session layers constrain negotiation variants to their linear phase; service
/// routing accepts `Game` only on stream 0 and `Resource` only on stream 1.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum ClientMessage {
    JoinPublicSession {
        resource_fingerprint: ResourceCatalogFingerprint,
    },
    Ready {
        stream: StreamId,
        protocol_version: u16,
    },
    Game(ClientGameMessage),
    Resource(ClientResourceMessage),
}

/// Top-level server-to-client envelope shared by both logical streams.
///
/// `SessionEstablished` and `Error` are negotiation messages. Service routing
/// accepts `Game` only on stream 0 and `Resource` only on stream 1.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum ServerMessage {
    SessionEstablished { info: SessionInfo },
    Error { code: ProtocolErrorCode },
    Game(ServerGameMessage),
    Resource(ServerResourceMessage),
}

/// Sequenced controller action; each action domain validates its own sequence.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct ControllerCommand<Action> {
    /// Wrapping client-issued sequence compared by the receiving action domain.
    pub sequence: u64,
    pub action: Action,
}

/// Requested normalized world-space movement direction.
///
/// The authoritative server owns movement speed and displacement. The wire
/// type does not enforce finite components or unit length.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct Movement3DAction {
    pub direction: [f32; 3],
}

/// Requested player orientation in quaternion `[x, y, z, w]` order.
///
/// The wire type does not enforce finiteness, nonzero length, or normalization.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RotationSync {
    pub rotation: [f32; 4],
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DestroyBlockControllerAction;

/// Placement request carrying a compact voxel-registry ID.
///
/// Registration and placeability are validated by the authoritative server,
/// not by this wire type.
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

/// Authoritative player identity, scene, and world-space pose snapshot.
///
/// Rotation uses quaternion `[x, y, z, w]` order. Serialization itself does not
/// validate finite pose components.
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

/// Complete client projection of currently nearby players and joinable worlds.
///
/// Consumers reconcile their prior projection against the vectors; omission
/// means the corresponding marker is no longer present. Vector ordering is a
/// server publication choice and should not be used as identity.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct PresenceSnapshot {
    pub own_player_id: PlayerId,
    pub players: Vec<NearbyPlayer>,
    pub joinable_worlds: Vec<NearbyJoinableWorld>,
}

/// Chunk identity scoped by owning local coordinate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ChunkId {
    pub local_coordinate_id: LocalCoordinateId,
    /// Signed chunk-unit coordinate `[x, y, z]`, not a voxel position.
    pub coordinate: [i64; 3],
}

/// Server-issued content version for one [`ChunkId`].
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ChunkVersion {
    pub local_coordinate_id: LocalCoordinateId,
    pub coordinate: [i64; 3],
    pub version: UpdateVersion,
}

impl ChunkVersion {
    /// Drops version information while preserving coordinate identity.
    pub const fn id(self) -> ChunkId {
        ChunkId {
            local_coordinate_id: self.local_coordinate_id,
            coordinate: self.coordinate,
        }
    }
}

/// Opaque owned postcard bytes embedded inside a typed outer message.
///
/// This type imposes no size limit; transport framing applies its independent
/// frame limit to the complete outer message.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SerializedPayload(Vec<u8>);

impl SerializedPayload {
    /// Serializes a value into an owned postcard payload.
    pub fn encode<Value>(value: &Value) -> Result<Self, postcard::Error>
    where
        Value: Serialize,
    {
        postcard::to_allocvec(value).map(Self)
    }

    /// Deserializes the complete stored payload as `Value`.
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
