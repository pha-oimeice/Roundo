//! Ordered Bevy adapter: facts/intents, document transactions, then host projection.

use super::{ClientSceneManager, ClientSceneProjection, ClientUiIntent};
use crate::{commands::ClientCommandSystemSet, network::ClientNetworkManager};
use bevy::{app::AppExit, prelude::*};
use roundo_marionette::MarionetteClientSet;
use roundo_webui::{
    RecoveryAction, RecoveryActionRequest, UiLifecycleManager, UiNavigationExecutor,
    UiRootReplacement, WebUiSystemSet,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum ClientSceneSystemSet {
    Prepare,
    Requests,
    Project,
}

/// Owns all client flow orchestration; the hardware host only applies a projection.
pub struct ClientScenePlugin;
impl Plugin for ClientScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClientSceneManager>()
            .init_resource::<ClientSceneProjection>()
            .configure_sets(
                Update,
                (
                    ClientSceneSystemSet::Prepare.before(ClientCommandSystemSet::ExecuteAndPublish),
                    ClientSceneSystemSet::Requests
                        .after(ClientCommandSystemSet::ExecuteAndPublish)
                        .before(WebUiSystemSet::Platform),
                    ClientSceneSystemSet::Project
                        .after(WebUiSystemSet::Platform)
                        .after(ClientSceneSystemSet::Requests)
                        .before(MarionetteClientSet::Input),
                ),
            )
            .add_systems(
                Update,
                (apply_recovery_action, coordinate_scene)
                    .chain()
                    .in_set(ClientSceneSystemSet::Prepare),
            )
            .add_systems(
                Update,
                (coordinate_scene, input_intents)
                    .chain()
                    .in_set(ClientSceneSystemSet::Requests),
            )
            .add_systems(Update, project_scene.in_set(ClientSceneSystemSet::Project));
    }
}

fn apply_recovery_action(
    mut actions: MessageReader<RecoveryActionRequest>,
    mut network: ResMut<ClientNetworkManager>,
    mut ui: ResMut<UiLifecycleManager>,
    mut app_exit: MessageWriter<AppExit>,
) {
    for request in actions.read() {
        match request.0 {
            RecoveryAction::Retry => {
                if let Err(error) = ui.retry_recovery() {
                    log::error!("cannot retry Scene root recovery: {error}");
                }
            }
            RecoveryAction::Disconnect => {
                network.disconnect();
            }
            RecoveryAction::Quit => {
                app_exit.write(AppExit::Success);
            }
        }
    }
}

fn coordinate_scene(
    network: Res<ClientNetworkManager>,
    mut scene: ResMut<ClientSceneManager>,
    mut ui: ResMut<UiLifecycleManager>,
    mut navigation: NonSendMut<UiNavigationExecutor>,
) {
    scene.synchronize(&network.status().status, &ui);
    let target = scene.root_scene();
    // Retain failure presentation while it still belongs to the desired Scene.
    // A connection change can select another root, but cannot dismiss Recovery
    // until that projection is accepted (or an existing complete root is reused).
    if ui
        .recovery_surface()
        .is_some_and(|recovery| recovery.scene == target)
    {
        return;
    }
    match navigation.replace_root(&mut ui, target) {
        Ok(replacement) => {
            ui.dismiss_recovery_surface();
            if let UiRootReplacement::Completed(destroyed) = replacement {
                log::info!(
                    "Scene root projection committed without a page: {target:?}; destroyed={}",
                    destroyed.len()
                );
            }
        }
        Err(error) => log::error!("cannot stage Scene root {target:?}: {error}"),
    }
}

fn input_intents(
    mut escapes: MessageReader<crate::input::ClientEscape>,
    network: Res<ClientNetworkManager>,
    mut scene: ResMut<ClientSceneManager>,
    mut ui: ResMut<UiLifecycleManager>,
    mut navigation: NonSendMut<UiNavigationExecutor>,
) {
    if escapes.read().count() == 0 {
        return;
    }
    let intent = ClientUiIntent::Escape;
    if let Err(error) = scene.execute(intent, &network.status().status, &mut ui, &mut navigation) {
        log::warn!("host UI intent {intent:?} rejected: {error}");
    }
}

fn project_scene(
    network: Res<ClientNetworkManager>,
    mut scene: ResMut<ClientSceneManager>,
    ui: Res<UiLifecycleManager>,
    navigation: NonSend<UiNavigationExecutor>,
    mut projection: ResMut<ClientSceneProjection>,
) {
    // Consume same-frame document commits/failures before input collection.
    scene.synchronize(&network.status().status, &ui);
    *projection = scene.projection(&ui, &navigation);
}
