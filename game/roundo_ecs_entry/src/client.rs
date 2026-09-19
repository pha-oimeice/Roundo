//! Composes the standalone Bevy client and its bound IPC endpoints.

use bevy::{
    DefaultPlugins,
    app::{App, PostStartup},
    prelude::{
        Entity, IVec2, PluginGroup, Query, Res, ResMut, Update, Window, WindowPlugin,
        WindowPosition, With, default,
    },
    window::WindowMode,
};
use roundo_local_coordinate::{
    AtomicVoxelRegistry, LocalCoordinateClientIpc, LocalCoordinateClientPlugin,
};
use roundo_marionette::{
    ClientMarionetteIpc, ClientPlacedVoxelId, ClientPlayerController, MarionetteClientPlugin,
};
use roundo_presence::{
    ClientPlayerMarker, ClientPresenceIpc, LocalPlayerIdentity, Player, RoundoPresenceClientPlugin,
};
use roundo_rendering::{DebugCamera, RoundoRenderingPlugin};

/// Network-facing endpoints bound to one concrete client ECS App.
#[derive(Clone)]
pub struct ClientEcsEndpoints {
    /// Controller command/event endpoint connected to this runtime's Marionette plugin.
    pub marionette: ClientMarionetteIpc,
    /// Presence snapshot endpoint connected to this runtime's Presence plugin.
    pub presence: ClientPresenceIpc,
    /// Resource-stream endpoint connected to this runtime's Local Coordinate plugin.
    pub local_coordinate: LocalCoordinateClientIpc,
}

/// One fully assembled client App and the endpoints of exactly its plugins.
pub struct ClientEcsRuntime {
    app: App,
    endpoints: ClientEcsEndpoints,
}

impl ClientEcsRuntime {
    /// Builds the client App and captures IPC endpoints before plugin ownership
    /// moves into Bevy. No process-global singleton queues are used.
    pub fn new(voxels: AtomicVoxelRegistry) -> Self {
        let presence = RoundoPresenceClientPlugin::new();
        let local_coordinate = LocalCoordinateClientPlugin::new();
        let marionette = MarionetteClientPlugin::new();
        let endpoints = ClientEcsEndpoints {
            marionette: marionette.ipc(),
            presence: presence.ipc(),
            local_coordinate: local_coordinate.ipc(),
        };
        let placed_voxel = ClientPlacedVoxelId(voxels.default_placed_voxel().0);
        let mut app = App::new();
        app.insert_resource(voxels)
            .insert_resource(placed_voxel)
            .add_plugins((
                DefaultPlugins.set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Roundo".to_string(),
                        position: WindowPosition::At(IVec2::new(0, 0)),
                        mode: WindowMode::Windowed,
                        ..default()
                    }),
                    ..default()
                }),
                RoundoRenderingPlugin,
                presence,
                local_coordinate,
                marionette,
            ));
        app.add_systems(PostStartup, bind_player_controller_to_debug_camera)
            .add_systems(Update, sync_local_player_marker_visibility);
        Self { app, endpoints }
    }

    /// Separates the assembled App from its matching network endpoints.
    ///
    /// Endpoint clones continue to address only these plugin instances. Dropping
    /// the App disconnects them; creating another runtime never retargets them.
    pub fn into_parts(self) -> (App, ClientEcsEndpoints) {
        (self.app, self.endpoints)
    }
}

/// Builds the client App when external IPC access is not required.
pub fn create_ecs_client_app(voxels: AtomicVoxelRegistry) -> App {
    ClientEcsRuntime::new(voxels).into_parts().0
}

/// Runs a client backed by the built-in voxel registry.
pub fn run_ecs_client() {
    create_ecs_client_app(AtomicVoxelRegistry::builtin()).run();
}

fn bind_player_controller_to_debug_camera(
    debug_cameras: Query<Entity, With<DebugCamera>>,
    mut player_controller: ResMut<ClientPlayerController>,
) {
    if let Some(debug_camera) = debug_cameras.iter().next() {
        player_controller.bind_camera(debug_camera);
    }
}

fn sync_local_player_marker_visibility(
    controller: Res<ClientPlayerController>,
    local_identity: Res<LocalPlayerIdentity>,
    mut players: Query<(&Player, &mut ClientPlayerMarker)>,
) {
    for (player, mut marker) in &mut players {
        marker.visible =
            Some(player.id) != local_identity.player_id() || controller.is_spirit_walking();
    }
}
