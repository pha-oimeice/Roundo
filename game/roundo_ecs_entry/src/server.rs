//! Composes the fixed-tick authoritative ECS server and domain IPC endpoints.

use crate::server_integration::ServerDomainIntegrationPlugin;
use avian3d::PhysicsPlugins;
use bevy::{
    MinimalPlugins,
    app::App,
    asset::AssetPlugin,
    mesh::MeshPlugin,
    prelude::{Fixed, Time},
};
use roundo_block_interaction::BlockInteractionPlugin;
use roundo_local_coordinate::{
    AtomicVoxelRegistry, LocalCoordinateServerIpc, LocalCoordinateServerPlugin,
};
use roundo_marionette::{MarionetteServerPlugin, ServerMarionetteIpc};
use roundo_portal::RoundoPortalPlugin;
use roundo_presence::{PresenceServerIpc, PresenceServerSettings, RoundoPresenceServerPlugin};
use std::sync::{
    LazyLock,
    atomic::{AtomicU32, Ordering},
};

const DEFAULT_TICK_RATE: u32 = 20;

// Plugin singletons expose stable IPC endpoints before App construction.
static PRESENCE_PLUGIN: LazyLock<RoundoPresenceServerPlugin> =
    LazyLock::new(RoundoPresenceServerPlugin::new);
static MARIONETTE_PLUGIN: LazyLock<MarionetteServerPlugin> =
    LazyLock::new(MarionetteServerPlugin::new);
static LOCAL_COORDINATE_PLUGIN: LazyLock<LocalCoordinateServerPlugin> =
    LazyLock::new(LocalCoordinateServerPlugin::new);
static TICK_RATE: AtomicU32 = AtomicU32::new(DEFAULT_TICK_RATE);
static PRESENCE_RADIUS: AtomicU32 = AtomicU32::new(128.0_f32.to_bits());

/// Returns a clone of the network-facing presence endpoint.
///
/// All clones share the singleton plugin's queues; constructing an [`App`] does
/// not replace or disconnect previously returned endpoints.
pub fn server_presence_ipc() -> PresenceServerIpc {
    PRESENCE_PLUGIN.ipc()
}

/// Returns a clone of the network-facing controller endpoint.
///
/// All clones share the singleton plugin's queues; constructing an [`App`] does
/// not replace or disconnect previously returned endpoints.
pub fn server_marionette_ipc() -> ServerMarionetteIpc {
    MARIONETTE_PLUGIN.ipc()
}

/// Returns a clone of the network-facing coordinate endpoint.
///
/// All clones share the singleton plugin's queues; constructing an [`App`] does
/// not replace or disconnect previously returned endpoints.
pub fn server_local_coordinate_ipc() -> LocalCoordinateServerIpc {
    LOCAL_COORDINATE_PLUGIN.ipc()
}

/// Configures the fixed-update frequency of subsequently created apps.
///
/// Zero is normalized to one tick per second. Existing apps are unaffected;
/// concurrent calls use last-store-wins semantics.
pub fn configure_server_tick_rate(tick_rate: u32) {
    TICK_RATE.store(tick_rate.max(1), Ordering::Relaxed);
}

/// Configures the presence radius of subsequently created apps.
///
/// [`PresenceServerSettings::new`] replaces non-finite or non-positive values
/// with its default when an app is created. Existing apps are unaffected;
/// concurrent calls use last-store-wins semantics.
pub fn configure_server_presence_radius(radius: f32) {
    PRESENCE_RADIUS.store(radius.to_bits(), Ordering::Relaxed);
}

/// Builds an authoritative app with all server domains installed.
///
/// The app takes ownership of `voxels` and snapshots the process-wide tick rate
/// and presence radius at construction time. It reuses the singleton domain
/// plugins, so their IPC queues are shared with the endpoint accessors above.
pub fn create_ecs_server_app(voxels: AtomicVoxelRegistry) -> App {
    let mut app = App::new();
    app.insert_resource(voxels)
        .insert_resource(Time::<Fixed>::from_hz(f64::from(
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
        RoundoPortalPlugin,
        (*MARIONETTE_PLUGIN).clone(),
        (*PRESENCE_PLUGIN).clone(),
        (*LOCAL_COORDINATE_PLUGIN).clone(),
        ServerDomainIntegrationPlugin,
        BlockInteractionPlugin::default(),
    ));
    app
}

/// Builds and runs a server backed by the built-in voxel registry.
///
/// This blocks the current thread until the Bevy app exits.
pub fn run_ecs_server() {
    create_ecs_server_app(AtomicVoxelRegistry::builtin()).run();
}
