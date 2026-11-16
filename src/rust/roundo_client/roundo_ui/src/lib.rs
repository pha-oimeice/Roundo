//! Mod-driven TOML views rendered exclusively by Bevy native UI and Feather widgets.
//! Business commands remain client-owned; no browser, scripting engine or remote ECS endpoint.

use roundo_mod_manager::AssetAccess;
use roundo_mod_manager::ui::{UiCatalogue, expression, layout, schema};
mod registry;
mod runtime;
mod view;

use registry::UiRegistry;
pub use registry::UiResource;
pub use roundo_mod_manager::ui::{ClientInteractionMode, IN_GAME_ROOT_SLOT, MENU_ROOT_SLOT};
pub use runtime::{
    UiCommandIo, UiCommandSource, UiData, UiInstanceId, UiLifecycleError, UiLifecycleManager,
    UiOpenTarget,
};
pub use view::text_edit_has_focus;

use bevy::{
    feathers::{FeathersPlugins, dark_theme::create_dark_theme, theme::UiTheme},
    prelude::*,
};
use runtime::UiRuntime;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum UiSystemSet {
    Ingress,
    Project,
}
#[derive(Resource)]
pub struct UiReady;

pub struct RoundoUiPlugin {
    registry: UiRegistry,
    command_io: UiCommandIo,
}
impl RoundoUiPlugin {
    pub fn new(catalogue: UiCatalogue, assets: AssetAccess, command_io: UiCommandIo) -> Self {
        Self {
            registry: UiRegistry::new(catalogue, assets),
            command_io,
        }
    }
}
impl Plugin for RoundoUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(FeathersPlugins);
        match self.registry.theme.palette {
            registry::Palette::Dark => {
                app.insert_resource(UiTheme(create_dark_theme()));
            }
        }
        let font = app
            .world_mut()
            .resource_mut::<Assets<Font>>()
            .add(Font::from_bytes(
                self.registry.font().expect("load native UI font prototype"),
            ));
        let root = app
            .world_mut()
            .spawn_scene(roundo_scene::scene_root(roundo_scene::SceneKind::Menu))
            .expect("native scene root")
            .id();
        app.insert_resource(UiRuntime {
            registry: self.registry.clone(),
            root,
            focus: Vec::new(),
            next_z: 0,
            io: self.command_io.clone(),
            font,
        })
        .init_resource::<UiData>()
        .configure_sets(Update, (UiSystemSet::Ingress, UiSystemSet::Project).chain())
        .add_systems(Startup, initialize)
        .add_systems(
            Update,
            (view::ingress, runtime::advance)
                .chain()
                .in_set(UiSystemSet::Ingress),
        )
        .add_systems(Update, view::project.in_set(UiSystemSet::Project));
        view::install(app);
    }
}
fn initialize(world: &mut World) {
    UiLifecycleManager::with(world, |ui| ui.replace_root(roundo_scene::SceneKind::Menu))
        .unwrap_or_else(|error| panic!("cannot instantiate native menu: {error}"));
    for mut window in world
        .query_filtered::<&mut Window, With<bevy::window::PrimaryWindow>>()
        .iter_mut(world)
    {
        window.ime_enabled = true;
    }
    world.insert_resource(UiReady);
}
