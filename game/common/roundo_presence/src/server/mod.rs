//! Server presence plugin, IPC contracts, and ECS marker components.

mod systems;

use crate::{NearbyPlayer, Player, PlayerId, PlayerState, PresenceSnapshot, SceneId};
use bevy::{
    ecs::system::SystemParam,
    prelude::{
        App, Changed, Commands, Component, Entity, FixedUpdate, IntoScheduleConfigs, Plugin, Quat,
        Query, Res, ResMut, Resource, SystemSet, Transform, Vec3, With,
    },
};
use roundo_contracts::ConnectionId;
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::HashMap;

pub const DEFAULT_PRESENCE_RADIUS: f32 = 128.0;
const S1_SPAWN_TRANSLATION: Vec3 = Vec3::new(0.0, 2.0, 0.0);

/// Network-facing endpoint for presence commands and events.
pub type PresenceServerIpc =
    CrossbeamThreadPipeEndpointA<PresenceServerCommand, PresenceServerEvent>;

#[derive(Clone)]
/// Installs authoritative presence systems on the fixed schedule.
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
/// Fixed-update ordering boundaries for presence publication.
pub enum PresenceServerSet {
    Commands,
    PlayerStates,
    Snapshots,
}

#[derive(Resource, Clone, Copy, Debug)]
/// Runtime bounds for nearby-player and world discovery.
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

#[derive(Clone, Debug)]
/// Connection lifecycle changes consumed by the presence domain.
pub enum PresenceServerCommand {
    Connect { connection_id: ConnectionId },
    Disconnect { connection_id: ConnectionId },
}

#[derive(Clone, Debug)]
/// Identity, transform, and snapshot updates returned to networking.
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
/// Marks entities owned by the authoritative presence server.
pub struct ServerPlayer;

/// Process-owned administrative projection of one authoritative Player.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ServerPlayerSnapshot {
    pub player_id: PlayerId,
    pub scene_id: SceneId,
    pub translation: [f32; 3],
}

/// Domain-owned administrative interface used by server command adapters.
///
/// The command transport never receives direct mutable access to Player ECS
/// components; Presence retains lookup, ordering, and pose validation.
#[derive(SystemParam)]
pub struct PresenceAdmin<'w, 's> {
    players: Query<
        'w,
        's,
        (
            &'static Player,
            &'static PlayerScene,
            &'static mut Transform,
        ),
        With<ServerPlayer>,
    >,
}

impl PresenceAdmin<'_, '_> {
    pub fn players(&mut self) -> Vec<ServerPlayerSnapshot> {
        let mut players = self
            .players
            .iter_mut()
            .map(|(player, scene, transform)| ServerPlayerSnapshot {
                player_id: player.id,
                scene_id: scene.scene_id,
                translation: transform.translation.to_array(),
            })
            .collect::<Vec<_>>();
        players.sort_unstable_by_key(|player| player.player_id);
        players
    }

    pub fn player(&mut self, player_id: PlayerId) -> Option<ServerPlayerSnapshot> {
        self.players()
            .into_iter()
            .find(|player| player.player_id == player_id)
    }

    /// Replaces an authoritative Player position after rejecting non-finite input.
    pub fn teleport(&mut self, player_id: PlayerId, translation: [f32; 3]) -> Option<[f32; 3]> {
        if !translation.into_iter().all(f32::is_finite) {
            return None;
        }
        let (_, _, mut transform) = self
            .players
            .iter_mut()
            .find(|(player, _, _)| player.id == player_id)?;
        transform.translation = Vec3::from_array(translation);
        Some(transform.translation.to_array())
    }
}

/// Transport identity associated with a server-side Presence Player.
#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlayerConnection(pub ConnectionId);

#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlayerScene {
    pub scene_id: SceneId,
}
