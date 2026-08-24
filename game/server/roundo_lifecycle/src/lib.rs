use bevy::prelude::*;
mod anchor_tree;
mod graph;
pub mod lifecycle_tree;
mod mutex_lock;
mod my_logic;
mod timer;
mod validation;
mod world_graph;

pub struct MalkuthPlugin;
impl Plugin for MalkuthPlugin {
    fn build(&self, app: &mut App) {}
}
