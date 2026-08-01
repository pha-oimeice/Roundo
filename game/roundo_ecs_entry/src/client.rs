use bevy::{
    DefaultPlugins,
    app::{App, PostStartup},
    prelude::{
        Camera, Entity, IVec2, PluginGroup, PreUpdate, Query, Res, ResMut, Window, WindowPlugin,
        WindowPosition, With, Without, default,
    },
    window::WindowMode,
};
use roundo_local_coordinate::{LocalCoordinateClientIpc, LocalCoordinateClientPlugin};
use roundo_marionette::{
    ClientMarionetteIpc, ClientPlayerController, ControllerCamera, MarionetteClientPlugin,
};
use roundo_presence::{ClientPresenceIpc, RoundoPresenceClientPlugin};
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
    app.add_plugins((
        DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Roundo".to_string(),
                position: WindowPosition::At(IVec2::new(0, 0)),
                mode: WindowMode::Windowed,
                ..default()
            }),
            ..default()
        }),
        (*PRESENCE_PLUGIN).clone(),
        (*LOCAL_COORDINATE_PLUGIN).clone(),
        (*MARIONETTE_PLUGIN).clone(),
        RoundoRenderingPlugin,
    ));
    app.add_systems(PostStartup, bind_player_controller_to_debug_camera)
        .add_systems(PreUpdate, sync_controlled_cameras);
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
        player_controller.bind_local_entity(debug_camera);
    }
}

fn sync_controlled_cameras(
    player_controller: Res<ClientPlayerController>,
    mut debug_cameras: Query<(Entity, &mut Camera), (With<DebugCamera>, Without<ControllerCamera>)>,
    mut networked_cameras: Query<&mut Camera, (With<ControllerCamera>, Without<DebugCamera>)>,
) {
    for (entity, mut camera) in &mut debug_cameras {
        let is_controlled = player_controller.is_bound_to(entity);
        camera.is_active = is_controlled;
    }

    for mut camera in &mut networked_cameras {
        camera.is_active = player_controller.sends_network_intent();
    }
}
