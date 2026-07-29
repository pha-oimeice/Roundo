use bevy::prelude::{Entity, Message};

#[derive(Message, Debug, Clone)]
pub enum LocalCoordinateTestMessage {
    /// Create a maze local coordinate
    CreateDummyMaze {
        entity: Entity,
        seed: u64,
        radius: i16,
    },
}
#[derive(Message, Debug, Clone)]
pub struct TestCreateDummyMaze {
    pub entity: Entity,
    pub seed: u64,
    pub radius: i16,
}
impl TestCreateDummyMaze {
    pub fn new(entity: Entity, seed: u64, radius: i16) -> Self {
        Self {
            entity,
            seed,
            radius,
        }
    }
}
