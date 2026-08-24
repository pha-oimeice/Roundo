mod systems;

use crate::{NearbyPlayer, Player, PlayerId, PlayerState, PresenceSnapshot, SceneId};
use bevy::prelude::{
    App, Changed, Commands, Component, Entity, FixedUpdate, IntoScheduleConfigs, Plugin, Quat,
    Query, Res, ResMut, Resource, SystemSet, Transform, Vec3, With,
};
use roundo_marionette::{MarionetteServerSet, NetworkControllerTarget, PlayerControllers};
use roundo_networking::ConnectionId;
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::HashMap;

pub const DEFAULT_PRESENCE_RADIUS: f32 = 128.0;
pub const DEFAULT_S0_ROOM_SIZE: [f32; 3] = [32.0; 3];
pub const DEFAULT_S1_EDGE_LENGTH: f32 = 16_384.0;
const S1_SPAWN_TRANSLATION: Vec3 = Vec3::new(0.0, 2.0, 0.0);

pub type PresenceServerIpc =
    CrossbeamThreadPipeEndpointA<PresenceServerCommand, PresenceServerEvent>;

#[derive(Clone)]
pub struct RoundoPresenceServerPlugin {
    pipe: CrossbeamThreadPipe<PresenceServerCommand, PresenceServerEvent>,
}

impl RoundoPresenceServerPlugin {
    pub fn new() -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
        }
    }

    pub fn ipc(&self) -> PresenceServerIpc {
        self.pipe.endpoint_a()
    }
}

impl Default for RoundoPresenceServerPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum PresenceServerSet {
    Commands,
    SceneConstraints,
    PlayerStates,
    Snapshots,
}

#[derive(Resource, Clone, Copy, Debug)]
pub struct PresenceServerSettings {
    pub radius: f32,
}

impl PresenceServerSettings {
    pub fn new(radius: f32) -> Self {
        Self {
            radius: valid_positive(radius, DEFAULT_PRESENCE_RADIUS),
        }
    }
}

impl Default for PresenceServerSettings {
    fn default() -> Self {
        Self::new(DEFAULT_PRESENCE_RADIUS)
    }
}

fn valid_positive(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        fallback
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TorusSpace {
    size: [f32; 3],
}

impl TorusSpace {
    pub fn new(size: [f32; 3]) -> Option<Self> {
        size.iter()
            .all(|extent| extent.is_finite() && *extent > 0.0)
            .then_some(Self { size })
    }

    pub const fn size(self) -> [f32; 3] {
        self.size
    }

    pub fn wrap(self, translation: Vec3) -> Vec3 {
        Vec3::new(
            translation.x.rem_euclid(self.size[0]),
            translation.y.rem_euclid(self.size[1]),
            translation.z.rem_euclid(self.size[2]),
        )
    }

    fn distance_squared(self, first: Vec3, second: Vec3) -> f32 {
        (0..3)
            .map(|axis| {
                let difference = (first[axis] - second[axis]).abs();
                difference.min(self.size[axis] - difference).powi(2)
            })
            .sum()
    }
}

#[derive(Resource)]
pub struct ServerSceneWorlds {
    s0_rooms: HashMap<u64, TorusSpace>,
    s1: TorusSpace,
}

impl ServerSceneWorlds {
    pub fn resize_s0_room(&mut self, room_id: u64, size: [f32; 3]) -> bool {
        let Some(space) = TorusSpace::new(size) else {
            return false;
        };
        self.s0_rooms.insert(room_id, space);
        true
    }

    pub fn remove_s0_room(&mut self, room_id: u64) -> bool {
        self.s0_rooms.remove(&room_id).is_some()
    }

    pub fn space(&self, scene_id: SceneId) -> Option<TorusSpace> {
        match scene_id {
            SceneId::S0 { room_id } => self.s0_rooms.get(&room_id).copied(),
            SceneId::S1 => Some(self.s1),
        }
    }
}

impl Default for ServerSceneWorlds {
    fn default() -> Self {
        Self {
            s0_rooms: HashMap::new(),
            s1: TorusSpace::new([DEFAULT_S1_EDGE_LENGTH; 3])
                .expect("the default S1 dimensions are valid"),
        }
    }
}

#[derive(Clone, Debug)]
pub enum PresenceServerCommand {
    Connect { connection_id: ConnectionId },
    Disconnect { connection_id: ConnectionId },
}

#[derive(Clone, Debug)]
pub enum PresenceServerEvent {
    PlayerJoined {
        connection_id: ConnectionId,
        player_id: PlayerId,
    },
    PlayerStateChanged {
        connection_id: ConnectionId,
        state: PlayerState,
    },
    Snapshot {
        connection_id: ConnectionId,
        snapshot: PresenceSnapshot,
    },
}

#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ServerPlayer;

#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlayerScene {
    pub scene_id: SceneId,
}
