//! Composes the standalone Bevy client and exposes its IPC endpoints.

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
use std::sync::LazyLock;

// Plugin singletons keep IPC endpoints stable before the App is constructed.
static MARIONETTE_PLUGIN: LazyLock<MarionetteClientPlugin> =
    LazyLock::new(MarionetteClientPlugin::new);
static PRESENCE_PLUGIN: LazyLock<RoundoPresenceClientPlugin> =
    LazyLock::new(RoundoPresenceClientPlugin::new);
static LOCAL_COORDINATE_PLUGIN: LazyLock<LocalCoordinateClientPlugin> =
    LazyLock::new(LocalCoordinateClientPlugin::new);

/// Returns the controller bridge endpoint owned by the client plugin.
pub fn client_marionette_ipc() -> ClientMarionetteIpc {
    MARIONETTE_PLUGIN.ipc()
}

/// Returns the presence bridge endpoint owned by the client plugin.
pub fn client_presence_ipc() -> ClientPresenceIpc {
    PRESENCE_PLUGIN.ipc()
}

/// Returns the voxel-streaming bridge endpoint owned by the client plugin.
pub fn client_local_coordinate_ipc() -> LocalCoordinateClientIpc {
    LOCAL_COORDINATE_PLUGIN.ipc()
}

/// Builds the client App with rendering and gameplay plugins installed.
pub fn create_ecs_client_app(voxels: AtomicVoxelRegistry) -> App {
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
            (*PRESENCE_PLUGIN).clone(),
            (*LOCAL_COORDINATE_PLUGIN).clone(),
            (*MARIONETTE_PLUGIN).clone(),
        ));
    app.add_systems(PostStartup, bind_player_controller_to_debug_camera)
        .add_systems(Update, sync_local_player_marker_visibility);
    app
}

/// Runs a client backed by the built-in voxel registry.
pub fn run_ecs_client() {
    create_ecs_client_app(AtomicVoxelRegistry::builtin()).run();
}

// The debug camera is the initial viewpoint until player presentation replaces it.
fn bind_player_controller_to_debug_camera(
    debug_cameras: Query<Entity, With<DebugCamera>>,
    mut player_controller: ResMut<ClientPlayerController>,
) {
    if let Some(debug_camera) = debug_cameras.iter().next() {
        player_controller.bind_camera(debug_camera);
    }
}

// Hide the local marker unless spirit walking separates camera and player.
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
