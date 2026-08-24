use crate::assets::{client_asset_plugin, register_common_asset_source};
use bevy::{
    DefaultPlugins,
    app::{App, PostStartup},
    prelude::{
        Entity, IVec2, PluginGroup, Query, Res, ResMut, Update, Window, WindowPlugin,
        WindowPosition, With, default,
    },
    window::WindowMode,
};
use roundo_local_coordinate::{LocalCoordinateClientIpc, LocalCoordinateClientPlugin};
use roundo_marionette::{ClientMarionetteIpc, ClientPlayerController, MarionetteClientPlugin};
use roundo_presence::{
    ClientPlayerMarker, ClientPresenceIpc, LocalPlayerIdentity, Player, RoundoPresenceClientPlugin,
};
use roundo_rendering::{DebugCamera, RoundoRenderingPlugin};
use std::sync::LazyLock;

static MARIONETTE_PLUGIN: LazyLock<MarionetteClientPlugin> =
    LazyLock::new(MarionetteClientPlugin::new);
static PRESENCE_PLUGIN: LazyLock<RoundoPresenceClientPlugin> =
    LazyLock::new(RoundoPresenceClientPlugin::new);
static LOCAL_COORDINATE_PLUGIN: LazyLock<LocalCoordinateClientPlugin> =
    LazyLock::new(LocalCoordinateClientPlugin::new);

pub fn client_marionette_ipc() -> ClientMarionetteIpc {
    MARIONETTE_PLUGIN.ipc()
}

pub fn client_presence_ipc() -> ClientPresenceIpc {
    PRESENCE_PLUGIN.ipc()
}

pub fn client_local_coordinate_ipc() -> LocalCoordinateClientIpc {
    LOCAL_COORDINATE_PLUGIN.ipc()
}

pub fn create_ecs_client_app() -> App {
    let mut app = App::new();
    register_common_asset_source(&mut app);
    app.add_plugins((
        DefaultPlugins.set(client_asset_plugin()).set(WindowPlugin {
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

pub fn run_ecs_client() {
    create_ecs_client_app().run();
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
    players: Query<(&Player, &ClientPlayerMarker)>,
    mut render_objects: ResMut<roundo_rendering::RenderObjects>,
) {
    for (player, marker) in &players {
        let visible =
            Some(player.id) != local_identity.player_id() || controller.is_spirit_walking();
        roundo_rendering::update_render_object_visibility(
            &mut render_objects,
            marker.render_object_id,
            visible,
        );
    }
}
