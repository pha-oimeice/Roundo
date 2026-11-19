//! World-local Player poses and administrative queries.
use super::{Player, PlayerId, SceneId};
use bevy::{
    ecs::system::SystemParam,
    prelude::{Component, Query, Transform, Vec3, With},
};

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
/// Presence retains lookup, ordering and pose validation.
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

#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlayerScene {
    pub scene_id: SceneId,
}
