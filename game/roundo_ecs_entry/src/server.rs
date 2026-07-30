use avian3d::PhysicsPlugins;
use bevy::MinimalPlugins;
use bevy::app::App;
use bevy::mesh::MeshPlugin;
use bevy::prelude::{AssetPlugin, Fixed, Time};
use roundo_character::{CharacterServerIpc, RoundoCharacterServerPlugin};
use roundo_marionette::{MarionetteServerPlugin, ServerMarionetteIpc};
use static_voxel::{StaticVoxelServerIpc, StaticVoxelServerPlugin};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU32, Ordering};

const DEFAULT_TICK_RATE: u32 = 20;

static MARIONETTE_PLUGIN: LazyLock<MarionetteServerPlugin> =
    LazyLock::new(MarionetteServerPlugin::new);
static CHARACTER_PLUGIN: LazyLock<RoundoCharacterServerPlugin> =
    LazyLock::new(RoundoCharacterServerPlugin::new);
static STATIC_VOXEL_PLUGIN: LazyLock<StaticVoxelServerPlugin> =
    LazyLock::new(StaticVoxelServerPlugin::new);
static TICK_RATE: AtomicU32 = AtomicU32::new(DEFAULT_TICK_RATE);

pub fn server_marionette_ipc() -> ServerMarionetteIpc {
    MARIONETTE_PLUGIN.ipc()
}

pub fn server_character_ipc() -> CharacterServerIpc {
    CHARACTER_PLUGIN.ipc()
}

pub fn server_static_voxel_ipc() -> StaticVoxelServerIpc {
    STATIC_VOXEL_PLUGIN.ipc()
}

pub fn configure_server_tick_rate(tick_rate: u32) {
    TICK_RATE.store(tick_rate.max(1), Ordering::Relaxed);
}

pub fn run_ecs_server() {
    let mut app = App::new();
    app.insert_resource(Time::<Fixed>::from_hz(f64::from(
        TICK_RATE.load(Ordering::Relaxed),
    )));
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        MeshPlugin,
        PhysicsPlugins::default(),
        (*CHARACTER_PLUGIN).clone(),
        (*STATIC_VOXEL_PLUGIN).clone(),
        (*MARIONETTE_PLUGIN).clone(),
    ));
    app.run();
}
