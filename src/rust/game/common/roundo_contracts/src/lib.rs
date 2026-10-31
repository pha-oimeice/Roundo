//! Versioned wire contracts shared by game domains and network transports.
//!
//! This crate owns serialized identities, DTOs, logical stream identities, and
//! protocol versions. It intentionally contains no socket, runtime, TLS, or ECS
//! implementation so domain modules can depend on contracts without depending
//! on a particular transport.

// Derives use the same absolute contracts path inside and outside this crate.
extern crate self as roundo_contracts;

mod attributes;
pub mod debug;
pub mod inventory;
pub use attributes::{AttributeId, InvalidAttributeId};

use roundo_proc_macros::{WireEnvelope, WireMessage};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Controller handoff DTOs; authority changes require the server's gate and settlement.
pub mod control_handoff;

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
/// Version 28 replaces direct block triggers with explicit item-instance use.
pub const GAME_PROTOCOL_VERSION: u16 = 28;
/// Exact stream-1 application version required during session setup.
/// Version 5 removes client Chunk requests; payload delivery is server-owned push.
pub const RESOURCE_PROTOCOL_VERSION: u16 = 5;

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

/// Application message classification shared by envelope routing.
///
/// The stream is a protocol role, not a delivery, authorization, or retry policy.
/// Negotiation envelopes deliberately do not implement this trait.
pub trait ApplicationMessage {
    const STREAM: StreamId;
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
// Opaque binding identity issued by the future atomic gate, not derived from
// sequence, tick, connection, Life, or Factory handle.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
#[repr(transparent)]
pub struct ControllerBindingId(pub u128);
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
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, WireEnvelope)]
pub enum ClientMessage {
    #[wire(negotiation)]
    JoinPublicSession {
        resource_fingerprint: ResourceCatalogFingerprint,
    },
    #[wire(negotiation)]
    Ready {
        stream: StreamId,
        protocol_version: u16,
    },
    #[wire(application)]
    Game(ClientGameMessage),
    #[wire(application)]
    Resource(ClientResourceMessage),
}

/// Top-level server-to-client envelope shared by both logical streams.
///
/// `SessionEstablished` and `Error` are negotiation messages. Service routing
/// accepts `Game` only on stream 0 and `Resource` only on stream 1.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, WireEnvelope)]
pub enum ServerMessage {
    #[wire(negotiation)]
    SessionEstablished { info: SessionInfo },
    #[wire(negotiation)]
    Error { code: ProtocolErrorCode },
    #[wire(application)]
    Game(ServerGameMessage),
    #[wire(application)]
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

/// Acknowledgement scoped to exactly one Controller binding. `None` carries
/// no binding identity and cannot acknowledge input for an active binding.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ControllerInputAcknowledgement {
    pub binding_id: Option<ControllerBindingId>,
    pub sequence: InputSequence,
}

/// Opaque semantic state committed by the authoritative Creature solver.
///
/// The DTO is a stable wire-value whitelist, not an ECS/Factory dump; local Life
/// handles, entities, registrations, and State history never enter this shape.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct CreatureMotionSnapshot {
    /// Authoritative fixed logic step in seconds. Rendering deltas never advance
    /// Creature simulation and clients replay against this exact step.
    pub fixed_dt_seconds: f32,
    pub simulation_tick: SimulationTick,
    /// Logical Creature AABB bottom-center position, not its center or center of mass.
    pub translation: [f32; 3],
    pub body_orientation: [f32; 4],
    pub gaze_orientation: [f32; 4],
    pub velocity: [f32; 3],
    pub locomotion: CreatureLocomotion,
    pub environment: CreatureEnvironmentSample,
    pub acknowledged_movement: ControllerInputAcknowledgement,
    pub acknowledged_gaze: ControllerInputAcknowledgement,
}

/// Absolute world-space look selection, quaternion [x, y, z, w].
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct SetGazeIntent {
    pub orientation: [f32; 4],
}

/// Instance identity only; neither an item prototype nor permission to use it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct ItemInstanceId(pub u128);

impl Serialize for ItemInstanceId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            serializer.serialize_str(&self.0.to_string())
        } else {
            serializer.serialize_u128(self.0)
        }
    }
}

impl<'de> Deserialize<'de> for ItemInstanceId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            String::deserialize(deserializer)?
                .parse::<u128>()
                .map(Self)
                .map_err(serde::de::Error::custom)
        } else {
            u128::deserialize(deserializer).map(Self)
        }
    }
}

/// Code-implemented use methods. Item data permits a method but contains no code.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ItemUseMethod {
    DestroyBlock,
    PlaceBlock,
}

/// Selects an exact item and a method, never a client hit or world-edit result.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ItemUseIntent {
    pub item: ItemInstanceId,
    pub method: ItemUseMethod,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpawnTestCreatureAction;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum PlayerControllerCommand {
    SetGaze(ControllerCommand<SetGazeIntent>),
    Movement3D(ControllerCommand<Movement3DAction>),
    Gaze(ControllerCommand<GazeIntent>),
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
    BlockInteraction,
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
    /// The vertical slice's explicitly supplied item, not an equipment slot or grant.
    pub block_item: Option<ItemInstanceId>,
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
    SetGaze(ControllerCommand<SetGazeIntent>),
    UseItem(ControllerCommand<ItemUseIntent>),
    Movement3D(ControllerCommand<Movement3DAction>),
    Gaze(ControllerCommand<GazeIntent>),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, WireMessage)]
#[wire(envelope = ClientMessage::Game, stream = StreamId::Stream0)]
pub enum ClientGameMessage {
    Inventory(inventory::InventoryRequest),
    Debug(debug::DebugRequest),
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
    /// Marks the frozen producer's ordered prefix; does not prove settlement.
    DrainBarrier(control_handoff::DrainBarrier),
    /// Requests a ticketed transition; enqueueing does not grant control.
    ControlRequest(control_handoff::ControlRequest),
    Ready(control_handoff::Ready),
    /// Legacy connection-addressed input; not proof of a Controller binding.
    /// Bound Controller input uses `SubmitControllerInput`.
    UsePlayerController {
        command: PlayerControllerCommand,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, WireMessage)]
#[wire(envelope = ClientMessage::Resource, stream = StreamId::Stream1)]
pub enum ClientResourceMessage {
    SetChunkViewDistance { chunks: u16 },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, WireMessage)]
#[wire(envelope = ServerMessage::Game, stream = StreamId::Stream0)]
pub enum ServerGameMessage {
    Inventory(inventory::InventoryReply),
    Debug(debug::DebugReply),
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
    /// Requests producer freeze; sending is not proof of cleanup or settlement.
    Freeze(control_handoff::Freeze),
    /// Acquire-only invitation; neither a terminal nor a grant.
    ControlPreparing(control_handoff::ControlPreparing),
    ControlTerminal(control_handoff::ControlTerminal),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, WireMessage)]
#[wire(envelope = ServerMessage::Resource, stream = StreamId::Stream1)]
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
