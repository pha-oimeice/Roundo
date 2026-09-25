//! Composes one fixed-tick authoritative ECS server and its bound IPC endpoints.

use crate::{
    authoritative_creature::AuthoritativeCreaturePlugin, player_control::PlayerControlServerPlugin,
};
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

const DEFAULT_TICK_RATE: u32 = 20;
const DEFAULT_PRESENCE_RADIUS: f32 = 128.0;

/// Process-supplied settings captured when one server ECS App is assembled.
#[derive(Clone, Copy, Debug)]
pub struct ServerEcsConfig {
    /// FixedUpdate frequency in hertz; zero is normalized to one.
    pub tick_rate: u32,
    /// Presence distance in world units; Presence normalizes non-finite/non-positive values.
    pub presence_radius: f32,
}

impl Default for ServerEcsConfig {
    fn default() -> Self {
        Self {
            tick_rate: DEFAULT_TICK_RATE,
            presence_radius: DEFAULT_PRESENCE_RADIUS,
        }
    }
}

/// Network-facing endpoints bound to one concrete server ECS App.
#[derive(Clone)]
pub struct ServerEcsEndpoints {
    /// Controller command endpoint connected to this runtime's Marionette plugin.
    pub marionette: ServerMarionetteIpc,
    /// Player lifecycle/snapshot endpoint connected to this runtime's Presence plugin.
    pub presence: PresenceServerIpc,
    /// Resource-stream endpoint connected to this runtime's Local Coordinate plugin.
    pub local_coordinate: LocalCoordinateServerIpc,
    /// Player identity, Access, and Controller-Control endpoint.
    pub player_control: crate::PlayerControlServerIpc,
}

/// One fully assembled authoritative App and exactly its plugin endpoints.
pub struct ServerEcsRuntime {
    app: App,
    endpoints: ServerEcsEndpoints,
}

impl ServerEcsRuntime {
    pub fn new(voxels: AtomicVoxelRegistry, config: ServerEcsConfig) -> Self {
        let presence = RoundoPresenceServerPlugin::new();
        let marionette = MarionetteServerPlugin::new();
        let local_coordinate = LocalCoordinateServerPlugin::new();
        let player_control = PlayerControlServerPlugin::new();
        let endpoints = ServerEcsEndpoints {
            marionette: marionette.ipc(),
            presence: presence.ipc(),
            local_coordinate: local_coordinate.ipc(),
            player_control: player_control.ipc(),
        };
        let mut app = App::new();
        app.insert_resource(voxels)
            .insert_resource(Time::<Fixed>::from_hz(f64::from(config.tick_rate.max(1))))
            .insert_resource(PresenceServerSettings::new(config.presence_radius));
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            MeshPlugin,
            PhysicsPlugins::default(),
            RoundoPortalPlugin,
            marionette,
            player_control,
            presence,
            local_coordinate,
            AuthoritativeCreaturePlugin::new(endpoints.local_coordinate.clone()),
            BlockInteractionPlugin::default(),
        ));
        Self { app, endpoints }
    }

    /// Separates the App from endpoints that address exactly its plugin instances.
    ///
    /// Dropping the App disconnects these channels; later runtimes create new queues.
    pub fn into_parts(self) -> (App, ServerEcsEndpoints) {
        (self.app, self.endpoints)
    }
}

/// Builds an App when external IPC access is not required.
pub fn create_ecs_server_app(voxels: AtomicVoxelRegistry) -> App {
    ServerEcsRuntime::new(voxels, ServerEcsConfig::default())
        .into_parts()
        .0
}

/// Builds and runs a server backed by the built-in voxel registry.
pub fn run_ecs_server() {
    create_ecs_server_app(AtomicVoxelRegistry::builtin()).run();
}
