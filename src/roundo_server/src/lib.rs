use bevy::{
    MinimalPlugins,
    app::{App, Plugin, Startup},
    log::info,
};

pub struct RoundoServerPlugin;

impl Plugin for RoundoServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MinimalPlugins)
            .add_systems(Startup, || info!("Roundo server host started"));
    }
}
