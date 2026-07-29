use bevy::ecs::entity::Entities;
use bevy::prelude::*;

#[derive(Component, Default, Debug)]
pub struct ParentAnchor(Option<Entity>);
#[derive(Component, Default, Debug)]
pub struct IsRoot;
/// 使特定Entity获得Anchor特性。如果parent_anchor为None，则该Entity为root，否则为子节点。
#[derive(Message, Debug)]
pub struct AnchorMessage {
    pub parent_anchor: Option<Entity>,
    pub entity: Entity,
}

/// 规则：当收到AnchorMessage时，如果parent_anchor为None，则该Entity为root，否则为子节点。
pub fn create_anchor(mut commands: Commands, mut messages: MessageReader<AnchorMessage>) {
    for message in messages.read() {
        let parent_anchor = message.parent_anchor;
        let entity = message.entity;
        if parent_anchor.is_none() {
            commands.entity(entity).insert(IsRoot);
            continue;
        }
        commands.entity(entity).insert(ParentAnchor(parent_anchor));
    }
}

pub fn update_anchor(
    mut commands: Commands,
    entities: &Entities,
    mut query: Query<(Entity, &mut ParentAnchor), Without<IsRoot>>,
) {
    for (e, mut parent) in query.iter_mut() {
        if let Some(p) = parent.0 {
            if !entities.contains(p) {
                parent.0 = None;
            }
        }
    }
}

/// 规则：节点树当中的非根节点的父亲节点（Entity）如果为None，则销毁，每tick执行一次。 \
/// Root节点不被检查。
pub fn anchor_validation(
    mut commands: Commands,
    entities: &Entities,
    mut query: Query<(Entity, &mut ParentAnchor), Without<IsRoot>>,
) {
    for (e, mut parent) in query.iter_mut() {
        if parent.0.is_none() {
            commands.entity(e).despawn();
            continue;
        }
        if let Some(p) = parent.0 {
            if !entities.contains(p) {
                parent.0 = None;
            }
        }
    }
}
