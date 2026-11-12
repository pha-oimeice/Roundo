//! Ordered native input, commands, scene navigation and host projection.

use super::{ClientSceneProjection, ClientUiIntent};
use crate::{commands::ClientCommandSystemSet, network::ClientNetworkManager};
use bevy::{ecs::message::MessageCursor, prelude::*};
use roundo_marionette::MarionetteClientSet;
use roundo_scene::{Scene, SceneKind};
use roundo_ui::{UiLifecycleManager, UiReady, UiSystemSet};

#[derive(Clone, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum ClientSceneSystemSet {
    Prepare,
    Requests,
    Project,
}
pub struct ClientScenePlugin;
impl Plugin for ClientScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<Scene>()
            .add_computed_state::<SceneKind>()
            .init_resource::<ClientSceneProjection>()
            .configure_sets(
                Update,
                (
                    ClientSceneSystemSet::Prepare
                        .after(UiSystemSet::Ingress)
                        .before(ClientCommandSystemSet::ExecuteAndPublish),
                    ClientSceneSystemSet::Requests
                        .after(ClientCommandSystemSet::ExecuteAndPublish)
                        .before(UiSystemSet::Project),
                    ClientSceneSystemSet::Project
                        .after(UiSystemSet::Project)
                        .after(ClientSceneSystemSet::Requests)
                        .before(MarionetteClientSet::Input),
                ),
            )
            .add_systems(
                Update,
                coordinate_scene
                    .in_set(ClientSceneSystemSet::Prepare)
                    .run_if(resource_exists::<UiReady>),
            )
            .add_systems(
                Update,
                (coordinate_scene, input_intents)
                    .chain()
                    .in_set(ClientSceneSystemSet::Requests)
                    .run_if(resource_exists::<UiReady>),
            )
            .add_systems(
                Update,
                project_scene
                    .in_set(ClientSceneSystemSet::Project)
                    .run_if(resource_exists::<UiReady>),
            );
    }
}
/// Also called before each queued command, so a disconnect in the preceding
/// request revokes its old view sources before the next request is admitted.
pub(crate) fn reconcile_scene(world: &mut World) -> Result<(), roundo_ui::UiLifecycleError> {
    let status = world.resource::<ClientNetworkManager>().status().status;
    super::synchronize(world, &status);
    let target = *world.resource::<State<SceneKind>>().get();
    UiLifecycleManager::with(world, |ui| ui.replace_root(target))
}
fn coordinate_scene(world: &mut World) {
    if let Err(error) = reconcile_scene(world) {
        log::error!("cannot instantiate native Scene root: {error}");
        world.write_message(bevy::app::AppExit::error());
    }
}
fn input_intents(world: &mut World, mut cursor: Local<MessageCursor<crate::input::ClientEscape>>) {
    if cursor
        .read(world.resource::<Messages<crate::input::ClientEscape>>())
        .count()
        == 0
    {
        return;
    }
    let status = world.resource::<ClientNetworkManager>().status().status;
    super::synchronize(world, &status);
    UiLifecycleManager::with(world, |ui| {
        if let Err(error) = super::execute(ClientUiIntent::Escape, ui) {
            log::warn!("host UI Escape rejected: {error}");
        }
    });
    super::apply_pending_transition(world);
}
fn project_scene(world: &mut World) {
    coordinate_scene(world);
    let focused = world
        .query_filtered::<&Window, With<bevy::window::PrimaryWindow>>()
        .single(world)
        .is_ok_and(|w| w.focused);
    let projection = UiLifecycleManager::with(world, |ui| super::projection(ui, focused));
    *world.resource_mut::<ClientSceneProjection>() = projection;
}
