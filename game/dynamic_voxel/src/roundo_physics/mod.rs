use bevy::app::PluginGroupBuilder;
use bevy::prelude::PluginGroup;

pub struct RoundoPhysicsPlugin;
impl PluginGroup for RoundoPhysicsPlugin {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
    }
}
