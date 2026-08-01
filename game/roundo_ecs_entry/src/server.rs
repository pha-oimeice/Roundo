use avian3d::PhysicsPlugins;
use bevy::MinimalPlugins;
use bevy::app::App;
use bevy::mesh::MeshPlugin;
use bevy::prelude::{AssetPlugin, Fixed, Time};
use roundo_local_coordinate::{LocalCoordinateServerIpc, LocalCoordinateServerPlugin};
use roundo_marionette::{MarionetteServerPlugin, ServerMarionetteIpc};
use roundo_presence::{PresenceServerIpc, PresenceServerSettings, RoundoPresenceServerPlugin};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU32, Ordering};

const DEFAULT_TICK_RATE: u32 = 20;

static MARIONETTE_PLUGIN: LazyLock<MarionetteServerPlugin> =
    LazyLock::new(MarionetteServerPlugin::new);
static PRESENCE_PLUGIN: LazyLock<RoundoPresenceServerPlugin> =
    LazyLock::new(RoundoPresenceServerPlugin::new);
static LOCAL_COORDINATE_PLUGIN: LazyLock<LocalCoordinateServerPlugin> =
    LazyLock::new(LocalCoordinateServerPlugin::new);
static TICK_RATE: AtomicU32 = AtomicU32::new(DEFAULT_TICK_RATE);
static PRESENCE_RADIUS: AtomicU32 = AtomicU32::new(128.0_f32.to_bits());

pub fn server_marionette_ipc() -> ServerMarionetteIpc {
    MARIONETTE_PLUGIN.ipc()
}

pub fn server_presence_ipc() -> PresenceServerIpc {
    PRESENCE_PLUGIN.ipc()
}

pub fn server_local_coordinate_ipc() -> LocalCoordinateServerIpc {
    LOCAL_COORDINATE_PLUGIN.ipc()
}

pub fn configure_server_tick_rate(tick_rate: u32) {
    TICK_RATE.store(tick_rate.max(1), Ordering::Relaxed);
}

pub fn configure_server_presence_radius(radius: f32) {
    PRESENCE_RADIUS.store(radius.to_bits(), Ordering::Relaxed);
}

pub fn run_ecs_server() {
    let mut app = App::new();
    app.insert_resource(Time::<Fixed>::from_hz(f64::from(
        TICK_RATE.load(Ordering::Relaxed),
    )))
    .insert_resource(PresenceServerSettings::new(f32::from_bits(
        PRESENCE_RADIUS.load(Ordering::Relaxed),
    )));
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        MeshPlugin,
        PhysicsPlugins::default(),
        (*PRESENCE_PLUGIN).clone(),
        (*LOCAL_COORDINATE_PLUGIN).clone(),
        (*MARIONETTE_PLUGIN).clone(),
    ));
    app.run();
}
