use bevy::prelude::{
    App, ButtonInput, Camera3d, Commands, Component, Entity, FixedUpdate, KeyCode, Plugin, Quat,
    Query, Res, ResMut, Resource, Transform, Update, Vec3, With,
};
use roundo_networking::protocol::{
    ControllerAccessPolicy, ControllerCameraState, ControllerDescriptor, ControllerId,
    ControllerInput, ControllerKind, LocomotionInput, ViewInput,
};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::HashMap;

pub type ClientMarionetteIpc =
    CrossbeamThreadPipeEndpointA<ClientMarionetteCommand, ClientMarionetteEvent>;

pub struct MarionetteClientPlugin {
    pipe: CrossbeamThreadPipe<ClientMarionetteCommand, ClientMarionetteEvent>,
}

impl MarionetteClientPlugin {
    pub fn new() -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
        }
    }

    pub fn ipc(&self) -> ClientMarionetteIpc {
        self.pipe.endpoint_a()
    }
}

impl Default for MarionetteClientPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for MarionetteClientPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClientControllerRegistry>()
            .insert_resource(ClientPipeResource(self.pipe.endpoint_b()))
            .add_systems(Update, (process_client_commands, route_view_input))
            .add_systems(FixedUpdate, route_locomotion_input);
    }
}

#[derive(Clone, Debug)]
pub enum ClientMarionetteCommand {
    RequestController {
        controller_id: ControllerId,
    },
    ReleaseController {
        controller_id: ControllerId,
    },
    ControllerGranted {
        controller: ControllerDescriptor,
    },
    ControllerRevoked {
        controller_id: ControllerId,
    },
    ViewCameraState {
        controller_id: ControllerId,
        state: ControllerCameraState,
    },
}

#[derive(Clone, Debug)]
pub enum ClientMarionetteEvent {
    RequestController {
        controller_id: ControllerId,
    },
    ReleaseController {
        controller_id: ControllerId,
    },
    ControllerInput {
        controller_id: ControllerId,
        input: ControllerInput,
    },
}

#[derive(Component, Clone, Copy, Debug)]
pub struct ControllerCamera {
    pub controller_id: ControllerId,
}

#[derive(Resource, Clone)]
struct ClientPipeResource(
    CrossbeamThreadPipeEndpointB<ClientMarionetteCommand, ClientMarionetteEvent>,
);

#[derive(Resource, Default)]
struct ClientControllerRegistry {
    controllers: HashMap<ControllerId, ControllerDescriptor>,
    cameras: HashMap<ControllerId, Entity>,
    next_input_sequence: HashMap<ControllerId, u64>,
}

impl ClientControllerRegistry {
    fn next_sequence(&mut self, controller_id: ControllerId) -> u64 {
        let sequence = self.next_input_sequence.entry(controller_id).or_default();
        *sequence = sequence.wrapping_add(1);
        *sequence
    }

    fn controller_ids_of_kind(&self, kind: ControllerKind) -> Vec<ControllerId> {
        self.controllers
            .values()
            .filter(|controller| {
                controller.kind == kind
                    && controller.access_policy != ControllerAccessPolicy::ReadOnly
            })
            .map(|controller| controller.controller_id)
            .collect()
    }
}

fn process_client_commands(
    mut commands: Commands,
    pipe: Res<ClientPipeResource>,
    mut registry: ResMut<ClientControllerRegistry>,
    mut cameras: Query<&mut Transform, With<ControllerCamera>>,
) {
    while let Some(command) = pipe.0.try_receive() {
        match command {
            ClientMarionetteCommand::RequestController { controller_id } => {
                let _ = pipe
                    .0
                    .try_send(ClientMarionetteEvent::RequestController { controller_id });
            }
            ClientMarionetteCommand::ReleaseController { controller_id } => {
                let _ = pipe
                    .0
                    .try_send(ClientMarionetteEvent::ReleaseController { controller_id });
            }
            ClientMarionetteCommand::ControllerGranted { controller } => {
                let controller_id = controller.controller_id;
                let is_view = controller.kind == ControllerKind::View;
                registry.controllers.insert(controller_id, controller);
                if is_view && !registry.cameras.contains_key(&controller_id) {
                    let camera = commands
                        .spawn((
                            Camera3d::default(),
                            ControllerCamera { controller_id },
                            Transform::default(),
                        ))
                        .id();
                    registry.cameras.insert(controller_id, camera);
                }
            }
            ClientMarionetteCommand::ControllerRevoked { controller_id } => {
                registry.controllers.remove(&controller_id);
                registry.next_input_sequence.remove(&controller_id);
                if let Some(camera) = registry.cameras.remove(&controller_id) {
                    commands.entity(camera).despawn();
                }
            }
            ClientMarionetteCommand::ViewCameraState {
                controller_id,
                state,
            } => {
                let Some(camera_entity) = registry.cameras.get(&controller_id) else {
                    continue;
                };
                let Ok(mut transform) = cameras.get_mut(*camera_entity) else {
                    continue;
                };
                apply_camera_state(&mut transform, state);
            }
        }
    }
}

fn route_locomotion_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    pipe: Res<ClientPipeResource>,
    mut registry: ResMut<ClientControllerRegistry>,
) {
    let direction = keyboard_direction(&keyboard);
    let jump = keyboard.just_pressed(KeyCode::Space);
    let sprint = keyboard.pressed(KeyCode::ShiftLeft);
    for controller_id in registry.controller_ids_of_kind(ControllerKind::Locomotion) {
        let input = LocomotionInput {
            sequence: registry.next_sequence(controller_id),
            world_direction: direction,
            jump,
            sprint,
        };
        let _ = pipe.0.try_send(ClientMarionetteEvent::ControllerInput {
            controller_id,
            input: ControllerInput::Locomotion(input),
        });
    }
}

fn route_view_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    pipe: Res<ClientPipeResource>,
    mut registry: ResMut<ClientControllerRegistry>,
    mut cameras: Query<(&ControllerCamera, &mut Transform)>,
) {
    let translation = view_translation(&keyboard);
    let yaw_delta = axis(&keyboard, KeyCode::ArrowLeft, KeyCode::ArrowRight) * 0.02;
    let pitch_delta = axis(&keyboard, KeyCode::ArrowDown, KeyCode::ArrowUp) * 0.02;
    if translation == [0.0, 0.0, 0.0] && yaw_delta == 0.0 && pitch_delta == 0.0 {
        return;
    }

    for controller_id in registry.controller_ids_of_kind(ControllerKind::View) {
        let input = ViewInput {
            sequence: registry.next_sequence(controller_id),
            translation,
            yaw_delta,
            pitch_delta,
        };
        for (camera, mut transform) in &mut cameras {
            if camera.controller_id == controller_id {
                apply_camera_input(&mut transform, input);
            }
        }
        let _ = pipe.0.try_send(ClientMarionetteEvent::ControllerInput {
            controller_id,
            input: ControllerInput::View(input),
        });
    }
}

fn keyboard_direction(keyboard: &ButtonInput<KeyCode>) -> [f32; 3] {
    [
        axis(keyboard, KeyCode::KeyA, KeyCode::KeyD),
        0.0,
        axis(keyboard, KeyCode::KeyW, KeyCode::KeyS),
    ]
}

fn view_translation(keyboard: &ButtonInput<KeyCode>) -> [f32; 3] {
    [
        axis(keyboard, KeyCode::KeyJ, KeyCode::KeyL) * 0.1,
        axis(keyboard, KeyCode::KeyU, KeyCode::KeyO) * 0.1,
        axis(keyboard, KeyCode::KeyI, KeyCode::KeyK) * 0.1,
    ]
}

fn axis(keyboard: &ButtonInput<KeyCode>, negative: KeyCode, positive: KeyCode) -> f32 {
    let positive_value = if keyboard.pressed(positive) { 1.0 } else { 0.0 };
    let negative_value = if keyboard.pressed(negative) { 1.0 } else { 0.0 };
    positive_value - negative_value
}

fn apply_camera_state(transform: &mut Transform, state: ControllerCameraState) {
    transform.translation = Vec3::from_array(state.translation);
    transform.rotation = Quat::from_rotation_y(state.yaw) * Quat::from_rotation_x(state.pitch);
}

fn apply_camera_input(transform: &mut Transform, input: ViewInput) {
    transform.translation += Vec3::from_array(input.translation);
    transform.rotation = Quat::from_rotation_y(input.yaw_delta)
        * transform.rotation
        * Quat::from_rotation_x(input.pitch_delta);
}
