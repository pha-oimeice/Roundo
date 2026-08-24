use bevy::prelude::*;

#[derive(Component, Default, Debug)]
pub struct MutexLock(Option<Entity>);
#[derive(Message, Debug)]
pub enum MutexLockEnum {
    Lock(Entity),
    Unlock,
}
#[derive(Message, Debug)]
pub struct MutexEntityMessage {
    pub lock: Option<Entity>,
    pub entity: Entity,
}

pub fn mutex_entity_handler(_commands: Commands, mut messages: MessageReader<MutexLockEnum>) {
    for message in messages.read() {
        match message {
            MutexLockEnum::Lock(e) => {}
            MutexLockEnum::Unlock => {}
            _ => {}
        }
    }
}
