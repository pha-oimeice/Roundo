//! Shared game-domain data and IPC contracts.
//!
//! These values are consumed by local domain modules. Application network
//! envelopes, session negotiation and traffic lanes are deliberately absent.
//! There is no socket, runtime, TLS, or ECS implementation here.

mod attributes;
pub mod debug;
pub mod inventory;
pub use attributes::{AttributeId, InvalidAttributeId};

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

/// Versioned sparse virtual-cell environment update carried by the resource lane.
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
    SetGaze(ControllerCommand<SetGazeIntent>),
    Movement3D(ControllerCommand<Movement3DAction>),
    Gaze(ControllerCommand<GazeIntent>),
}
