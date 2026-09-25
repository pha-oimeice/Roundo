//! Versioned wire contracts shared by game domains and network transports.
//!
//! This crate owns serialized identities, DTOs, logical stream identities, and
//! protocol versions. It intentionally contains no socket, runtime, TLS, or ECS
//! implementation so domain modules can depend on contracts without depending
//! on a particular transport.

use serde::{Deserialize, Serialize, de::DeserializeOwned};

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
        #[repr(transparent)]
        pub struct $name(pub u64);
    };
}

/// Exact stream-0 application version required during session setup.
pub const GAME_PROTOCOL_VERSION: u16 = 18;
/// Exact stream-1 application version required during session setup.
pub const RESOURCE_PROTOCOL_VERSION: u16 = 4;

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
identifier!(ControllerId);
identifier!(LocalCoordinateId);
identifier!(PlayerId);
identifier!(ChunkLoadingAnchorId);
identifier!(PredictionAnchorId);
identifier!(JoinableWorldId);
identifier!(SimulationTick);
identifier!(InputSequence);
identifier!(EnvironmentRevision);

/// Read-only client projection of one server-owned rendering anchor.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct RenderingAnchorState {
    pub id: ChunkLoadingAnchorId,
    pub owner: PlayerId,
    pub scene_id: SceneId,
    pub position: [f64; 3],
    pub radius_chunks: u16,
}

/// Read-only client projection of the server-owned physics-prediction interest.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct PredictionAnchorState {
    pub id: PredictionAnchorId,
    pub owner: PlayerId,
    pub scene_id: SceneId,
    pub position: [f64; 3],
    pub radius_chunks: u16,
}

/// Versioned sparse virtual-cell environment update carried by stream 1.
///
/// `fields` uses bit 0 for gravity, bit 1 for static friction, and bit 2 for
/// kinetic friction. Fields not selected by the mask use protocol defaults.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct VirtualChunkEnvironmentOverride {
    pub coordinate: [i64; 3],
    pub revision: EnvironmentRevision,
    pub fields: u8,
    pub gravity: [f32; 3],
    pub static_friction: f32,
    pub kinetic_friction: f32,
}

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

/// A Player's authorization level for one Controller.
///
/// The value is a contract-level capability description only. Controller
/// registration and exclusive current-control arbitration remain domain logic.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum ControllerAccessLevel {
    #[default]
    None,
    Read,
    ReadWrite,
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

/// Requested normalized movement axis in the controlled body's local frame.
///
/// The authoritative server owns orientation, speed, and displacement. The
/// wire type does not enforce finite components or unit length.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct Movement3DAction {
    pub direction: [f32; 3],
}

/// Local view delta requested by a controller.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct GazeIntent {
    pub yaw_delta: f32,
    pub pitch_delta: f32,
}

/// Pure wire locomotion state; it deliberately has no ECS representation.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum CreatureLocomotion {
    Grounded,
    #[default]
    Floating,
}

/// Complete environment sample used for an authoritative Creature tick.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct CreatureEnvironmentSample {
    pub gravity: [f32; 3],
    pub static_friction: f32,
    pub kinetic_friction: f32,
    pub revision: EnvironmentRevision,
}

/// Opaque semantic state committed by the authoritative Creature solver.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct CreatureMotionSnapshot {
    /// Authoritative fixed logic step in seconds. Rendering deltas never advance
    /// Creature simulation and clients replay against this exact step.
    pub fixed_dt_seconds: f32,
    pub simulation_tick: SimulationTick,
    pub translation: [f32; 3],
    pub body_orientation: [f32; 4],
    pub gaze_orientation: [f32; 4],
    pub velocity: [f32; 3],
    pub locomotion: CreatureLocomotion,
    pub environment: CreatureEnvironmentSample,
    pub acknowledged_movement: InputSequence,
    pub acknowledged_gaze: InputSequence,
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

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpawnTestCreatureAction;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum PlayerControllerCommand {
    Movement3D(ControllerCommand<Movement3DAction>),
    Gaze(ControllerCommand<GazeIntent>),
    DestroyBlock(ControllerCommand<DestroyBlockControllerAction>),
    PlaceBlock(ControllerCommand<PlaceBlockControllerAction>),
    SpawnTestCreature(ControllerCommand<SpawnTestCreatureAction>),
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

/// Monotonic Chunk content version advanced after each authoritative update.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct UpdateVersion(u64);

impl UpdateVersion {
    pub const INITIAL: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    pub const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }

    pub fn advance(&mut self) -> Self {
        *self = self.next();
        *self
    }
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

/// Read-only control state without disclosing another Player's identity.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ControllerControlState {
    Uncontrolled,
    ControlledBySelf,
    ControlledByOther,
}

/// Closed intent domain exposed for client-side Controller selection.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ControllerIntentDomain {
    Movement,
    Gaze,
}

/// One Controller visible in the current Player's access snapshot.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlayerControllerAccess {
    pub controller_id: ControllerId,
    pub intent_domain: ControllerIntentDomain,
    pub access_level: ControllerAccessLevel,
    pub control_state: ControllerControlState,
}

/// Server-authoritative projection of the Player represented by this connection.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlayerControllerAccessSnapshot {
    pub player_id: PlayerId,
    pub controllers: Vec<PlayerControllerAccess>,
}

/// The rejected request identity echoed without its input payload.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PlayerControllerOperation {
    RequestAccess,
    Acquire { controller_id: ControllerId },
    Release { controller_id: ControllerId },
    SubmitInput { controller_id: ControllerId },
}

/// Stable rejection for Player/Controller operations.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ControllerOperationError {
    ControllerNotFound,
    AccessDenied,
    ControlledByOther,
    NotCurrentController,
    InputOutsideDomain,
}

/// Directed Controller input admitted only after server-side ownership checks.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum DirectedControllerInput {
    Movement3D(ControllerCommand<Movement3DAction>),
    Gaze(ControllerCommand<GazeIntent>),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum ClientGameMessage {
    RequestPlayerControllerAccess,
    AcquireController {
        controller_id: ControllerId,
    },
    ReleaseController {
        controller_id: ControllerId,
    },
    SubmitControllerInput {
        controller_id: ControllerId,
        input: DirectedControllerInput,
    },
    /// Legacy connection-addressed input. Kept only for existing clients during
    /// migration; new Controller paths must use `SubmitControllerInput`.
    UsePlayerController {
        command: PlayerControllerCommand,
    },
}

impl ClientGameMessage {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::RequestPlayerControllerAccess => "RequestPlayerControllerAccess",
            Self::AcquireController { .. } => "AcquireController",
            Self::ReleaseController { .. } => "ReleaseController",
            Self::SubmitControllerInput { .. } => "SubmitControllerInput",
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
    PlayerControllerAccessSnapshot {
        snapshot: PlayerControllerAccessSnapshot,
    },
    ControllerOperationRejected {
        operation: PlayerControllerOperation,
        error: ControllerOperationError,
    },
    ControllerAcquired {
        controller_id: ControllerId,
    },
    ControllerReleased {
        controller_id: ControllerId,
    },
    CreatureMotionSnapshot {
        snapshot: CreatureMotionSnapshot,
    },
    PlayerState {
        state: PlayerState,
    },
    PresenceSnapshot {
        snapshot: PresenceSnapshot,
    },
}

impl ServerGameMessage {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::PlayerControllerAccessSnapshot { .. } => "PlayerControllerAccessSnapshot",
            Self::ControllerOperationRejected { .. } => "ControllerOperationRejected",
            Self::ControllerAcquired { .. } => "ControllerAcquired",
            Self::ControllerReleased { .. } => "ControllerReleased",
            Self::CreatureMotionSnapshot { .. } => "CreatureMotionSnapshot",
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
    RenderingAnchorSpawned {
        anchor: RenderingAnchorState,
    },
    RenderingAnchorUpdated {
        anchor: RenderingAnchorState,
    },
    RenderingAnchorDespawned {
        anchor_id: ChunkLoadingAnchorId,
    },
    PredictionAnchorSpawned {
        anchor: PredictionAnchorState,
    },
    PredictionAnchorUpdated {
        anchor: PredictionAnchorState,
    },
    PredictionAnchorDespawned {
        anchor_id: PredictionAnchorId,
    },
    VirtualChunkEnvironmentOverrides {
        overrides: Vec<VirtualChunkEnvironmentOverride>,
    },
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
            Self::RenderingAnchorSpawned { .. } => "RenderingAnchorSpawned",
            Self::RenderingAnchorUpdated { .. } => "RenderingAnchorUpdated",
            Self::RenderingAnchorDespawned { .. } => "RenderingAnchorDespawned",
            Self::PredictionAnchorSpawned { .. } => "PredictionAnchorSpawned",
            Self::PredictionAnchorUpdated { .. } => "PredictionAnchorUpdated",
            Self::PredictionAnchorDespawned { .. } => "PredictionAnchorDespawned",
            Self::VirtualChunkEnvironmentOverrides { .. } => "VirtualChunkEnvironmentOverrides",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_version_advances_and_wraps_without_a_sentinel() {
        let mut version = UpdateVersion::INITIAL;
        assert_eq!(version.advance(), UpdateVersion::new(1));
        assert_eq!(version.value(), 1);
        assert_eq!(UpdateVersion::new(u64::MAX).next(), UpdateVersion::INITIAL);
    }

    #[test]
    fn directed_controller_messages_round_trip_and_use_stable_kinds() {
        let message = ClientGameMessage::SubmitControllerInput {
            controller_id: ControllerId(7),
            input: DirectedControllerInput::Gaze(ControllerCommand {
                sequence: 3,
                action: GazeIntent {
                    yaw_delta: 0.5,
                    pitch_delta: -0.25,
                },
            }),
        };
        let bytes = postcard::to_allocvec(&message).unwrap();
        assert_eq!(
            postcard::from_bytes::<ClientGameMessage>(&bytes).unwrap(),
            message
        );
        assert_eq!(message.kind(), "SubmitControllerInput");
        assert_eq!(
            ServerGameMessage::ControllerOperationRejected {
                operation: PlayerControllerOperation::Acquire {
                    controller_id: ControllerId(7),
                },
                error: ControllerOperationError::ControllerNotFound,
            }
            .kind(),
            "ControllerOperationRejected"
        );
    }

    #[test]
    fn access_snapshot_hides_other_player_identity() {
        let snapshot = PlayerControllerAccessSnapshot {
            player_id: PlayerId(3),
            controllers: vec![PlayerControllerAccess {
                controller_id: ControllerId(9),
                intent_domain: ControllerIntentDomain::Movement,
                access_level: ControllerAccessLevel::ReadWrite,
                control_state: ControllerControlState::ControlledByOther,
            }],
        };
        let encoded = postcard::to_allocvec(&snapshot).unwrap();
        assert_eq!(
            postcard::from_bytes::<PlayerControllerAccessSnapshot>(&encoded).unwrap(),
            snapshot
        );
    }
}
