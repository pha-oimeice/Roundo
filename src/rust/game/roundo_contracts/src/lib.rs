//! Shared World-local game values.

mod attributes;
pub use attributes::{AttributeId, InvalidAttributeId};

macro_rules! identifier {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(transparent)]
        pub struct $name(pub u64);
    };
}

identifier!(ControllerId);
identifier!(LocalCoordinateId);
identifier!(PlayerId);

/// A Player's authorization level for one Controller.
///
/// The value is a contract-level capability description only. Controller
/// registration and exclusive current-control arbitration remain domain logic.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum ControllerAccessLevel {
    #[default]
    None,
    Read,
    ReadWrite,
}

/// Sequenced controller action; each action domain validates its own sequence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControllerCommand<Action> {
    /// Sequence compared by the local action domain.
    pub sequence: u64,
    pub action: Action,
}

/// Requested normalized movement axis in the controlled body's local frame.
///
/// The solver owns orientation, speed, and displacement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Movement3DAction {
    pub direction: [f32; 3],
}

/// Local view delta requested by a controller.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GazeIntent {
    pub yaw_delta: f32,
    pub pitch_delta: f32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SpawnTestCreatureAction;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum SceneId {
    S0 {
        room_id: u64,
    },
    #[default]
    S1,
}

/// Chunk identity scoped by owning local coordinate.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ChunkId {
    pub local_coordinate_id: LocalCoordinateId,
    /// Signed chunk-unit coordinate `[x, y, z]`, not a voxel position.
    pub coordinate: [i64; 3],
}
