use bevy::{
    input::mouse::AccumulatedMouseMotion,
    prelude::{
        App, ButtonInput, Camera, Camera3d, Commands, Component, Entity, EulerRot, FixedUpdate,
        KeyCode, Plugin, Quat, Query, Res, ResMut, Resource, Time, Transform, Update, Vec3, With,
    },
};
use roundo_networking::protocol::{
    ControllerAccessPolicy, ControllerCameraState, ControllerDescriptor, ControllerId,
    ControllerInput, ControllerKind, LocomotionInput, ViewInput,
};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::{BTreeMap, HashMap};

pub type ClientMarionetteIpc =
    CrossbeamThreadPipeEndpointA<ClientMarionetteCommand, ClientMarionetteEvent>;

#[derive(Debug, Clone)]
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
        app.init_resource::<ClientPlayerController>()
            .init_resource::<ClientMarionetteInputSettings>()
            .init_resource::<ClientKeyBindings>()
            .init_resource::<ClientControllerRegistry>()
            .insert_resource(ClientPipeResource(self.pipe.endpoint_b()))
            .add_systems(
                Update,
                (
                    process_client_commands,
                    control_local_camera,
                    route_view_input,
                ),
            )
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ClientPlayerControllerTarget {
    #[default]
    NetworkedControllers,
    LocalEntity(Entity),
}

#[derive(Resource, Clone, Copy, Debug)]
pub struct ClientPlayerController {
    target: ClientPlayerControllerTarget,
    input_enabled: bool,
}

impl Default for ClientPlayerController {
    fn default() -> Self {
        Self {
            target: ClientPlayerControllerTarget::default(),
            input_enabled: true,
        }
    }
}

impl ClientPlayerController {
    pub fn bind_networked_controllers(&mut self) {
        self.target = ClientPlayerControllerTarget::NetworkedControllers;
    }

    pub fn bind_local_entity(&mut self, entity: Entity) {
        self.target = ClientPlayerControllerTarget::LocalEntity(entity);
    }

    pub fn target(&self) -> ClientPlayerControllerTarget {
        self.target
    }

    pub fn is_bound_to(&self, entity: Entity) -> bool {
        self.target == ClientPlayerControllerTarget::LocalEntity(entity)
    }

    pub fn sends_network_intent(&self) -> bool {
        self.target == ClientPlayerControllerTarget::NetworkedControllers
    }

    pub fn set_input_enabled(&mut self, enabled: bool) {
        self.input_enabled = enabled;
    }

    pub fn input_enabled(&self) -> bool {
        self.input_enabled
    }
}

#[derive(Resource, Clone, Copy, Debug)]
pub struct ClientMarionetteInputSettings {
    pub mouse_sensitivity: f32,
    pub camera_move_speed: f32,
}

impl Default for ClientMarionetteInputSettings {
    fn default() -> Self {
        Self {
            mouse_sensitivity: 0.002,
            camera_move_speed: 5.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MovementAction {
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    MoveForward,
    MoveBackward,
}

impl MovementAction {
    pub const ALL: [Self; 6] = [
        Self::MoveUp,
        Self::MoveDown,
        Self::MoveLeft,
        Self::MoveRight,
        Self::MoveForward,
        Self::MoveBackward,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::MoveUp => "Move Up",
            Self::MoveDown => "Move Down",
            Self::MoveLeft => "Move Left",
            Self::MoveRight => "Move Right",
            Self::MoveForward => "Move Forward",
            Self::MoveBackward => "Move Backward",
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Self::MoveUp => 0,
            Self::MoveDown => 1,
            Self::MoveLeft => 2,
            Self::MoveRight => 3,
            Self::MoveForward => 4,
            Self::MoveBackward => 5,
        }
    }

    const fn direction(self) -> Vec3 {
        match self {
            Self::MoveUp => Vec3::Y,
            Self::MoveDown => Vec3::NEG_Y,
            Self::MoveLeft => Vec3::NEG_X,
            Self::MoveRight => Vec3::X,
            Self::MoveForward => Vec3::Z,
            Self::MoveBackward => Vec3::NEG_Z,
        }
    }
}

#[derive(Resource, Clone, Debug, Eq, PartialEq)]
pub struct ClientKeyBindings {
    bindings: BTreeMap<KeyCode, Vec<MovementAction>>,
}

impl ClientKeyBindings {
    pub fn empty() -> Self {
        Self {
            bindings: BTreeMap::new(),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (KeyCode, &[MovementAction])> {
        self.bindings
            .iter()
            .map(|(key, actions)| (*key, actions.as_slice()))
    }

    pub fn actions_for(&self, key: KeyCode) -> &[MovementAction] {
        self.bindings.get(&key).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn keys_for(&self, action: MovementAction) -> Vec<KeyCode> {
        self.bindings
            .iter()
            .filter_map(|(key, actions)| actions.contains(&action).then_some(*key))
            .collect()
    }

    pub fn bind(&mut self, key: KeyCode, action: MovementAction) -> bool {
        let actions = self.bindings.entry(key).or_default();
        if actions.contains(&action) {
            return false;
        }
        actions.push(action);
        true
    }

    pub fn unbind(&mut self, key: KeyCode, action: MovementAction) -> bool {
        let Some(actions) = self.bindings.get_mut(&key) else {
            return false;
        };
        let Some(index) = actions.iter().position(|bound| *bound == action) else {
            return false;
        };
        actions.remove(index);
        if actions.is_empty() {
            self.bindings.remove(&key);
        }
        true
    }

    pub fn reorder(
        &mut self,
        key: KeyCode,
        action: MovementAction,
        target: MovementAction,
    ) -> bool {
        let Some(actions) = self.bindings.get_mut(&key) else {
            return false;
        };
        let Some(from) = actions.iter().position(|bound| *bound == action) else {
            return false;
        };
        let Some(to) = actions.iter().position(|bound| *bound == target) else {
            return false;
        };
        if from == to {
            return false;
        }
        let action = actions.remove(from);
        actions.insert(to.min(actions.len()), action);
        true
    }

    fn movement_direction(&self, keyboard: &ButtonInput<KeyCode>) -> Vec3 {
        let mut direction = Vec3::ZERO;
        for (key, actions) in &self.bindings {
            if !keyboard.pressed(*key) {
                continue;
            }
            for action in actions {
                direction += action.direction();
            }
        }
        direction.clamp(Vec3::NEG_ONE, Vec3::ONE)
    }
}

impl Default for ClientKeyBindings {
    fn default() -> Self {
        let mut bindings = Self::empty();
        bindings.bind(KeyCode::Space, MovementAction::MoveUp);
        bindings.bind(KeyCode::ShiftLeft, MovementAction::MoveDown);
        bindings.bind(KeyCode::KeyA, MovementAction::MoveLeft);
        bindings.bind(KeyCode::KeyD, MovementAction::MoveRight);
        bindings.bind(KeyCode::KeyW, MovementAction::MoveForward);
        bindings.bind(KeyCode::KeyS, MovementAction::MoveBackward);
        bindings
    }
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
    player_controller: Res<ClientPlayerController>,
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
                            Camera {
                                is_active: player_controller.sends_network_intent(),
                                ..Default::default()
                            },
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
    bindings: Res<ClientKeyBindings>,
    pipe: Res<ClientPipeResource>,
    player_controller: Res<ClientPlayerController>,
    mut registry: ResMut<ClientControllerRegistry>,
) {
    if !player_controller.sends_network_intent() || !player_controller.input_enabled() {
        return;
    }

    let direction = bindings.movement_direction(&keyboard).to_array();
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
    bindings: Res<ClientKeyBindings>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    time: Res<Time>,
    settings: Res<ClientMarionetteInputSettings>,
    pipe: Res<ClientPipeResource>,
    player_controller: Res<ClientPlayerController>,
    mut registry: ResMut<ClientControllerRegistry>,
    mut cameras: Query<(&ControllerCamera, &mut Transform)>,
) {
    if !player_controller.sends_network_intent() || !player_controller.input_enabled() {
        return;
    }

    for controller_id in registry.controller_ids_of_kind(ControllerKind::View) {
        let Some((_, mut transform)) = cameras
            .iter_mut()
            .find(|(camera, _)| camera.controller_id == controller_id)
        else {
            continue;
        };
        let frame_input = camera_frame_input(
            &keyboard,
            &bindings,
            mouse_motion.delta,
            time.delta_secs(),
            *settings,
            &transform,
        );
        if frame_input.is_idle() {
            continue;
        }
        let input = ViewInput {
            sequence: registry.next_sequence(controller_id),
            translation: frame_input.translation.to_array(),
            yaw_delta: frame_input.yaw_delta,
            pitch_delta: frame_input.pitch_delta,
        };
        apply_camera_input(&mut transform, input);
        let _ = pipe.0.try_send(ClientMarionetteEvent::ControllerInput {
            controller_id,
            input: ControllerInput::View(input),
        });
    }
}

fn control_local_camera(
    keyboard: Res<ButtonInput<KeyCode>>,
    bindings: Res<ClientKeyBindings>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    time: Res<Time>,
    settings: Res<ClientMarionetteInputSettings>,
    player_controller: Res<ClientPlayerController>,
    mut cameras: Query<&mut Transform, With<Camera>>,
) {
    if !player_controller.input_enabled() {
        return;
    }
    let ClientPlayerControllerTarget::LocalEntity(camera_entity) = player_controller.target()
    else {
        return;
    };
    let Ok(mut transform) = cameras.get_mut(camera_entity) else {
        return;
    };
    let frame_input = camera_frame_input(
        &keyboard,
        &bindings,
        mouse_motion.delta,
        time.delta_secs(),
        *settings,
        &transform,
    );
    apply_camera_frame_input(&mut transform, frame_input);
}

fn apply_camera_state(transform: &mut Transform, state: ControllerCameraState) {
    transform.translation = Vec3::from_array(state.translation);
    transform.rotation = Quat::from_rotation_y(state.yaw) * Quat::from_rotation_x(state.pitch);
}

fn apply_camera_input(transform: &mut Transform, input: ViewInput) {
    transform.translation += Vec3::from_array(input.translation);
    apply_camera_rotation(transform, input.yaw_delta, input.pitch_delta);
}

#[derive(Clone, Copy, Debug, Default)]
struct CameraFrameInput {
    translation: Vec3,
    yaw_delta: f32,
    pitch_delta: f32,
}

impl CameraFrameInput {
    fn is_idle(self) -> bool {
        self.translation == Vec3::ZERO && self.yaw_delta == 0.0 && self.pitch_delta == 0.0
    }
}

fn camera_frame_input(
    keyboard: &ButtonInput<KeyCode>,
    bindings: &ClientKeyBindings,
    mouse_delta: bevy::prelude::Vec2,
    delta_seconds: f32,
    settings: ClientMarionetteInputSettings,
    transform: &Transform,
) -> CameraFrameInput {
    let local_direction = bindings.movement_direction(keyboard).normalize_or_zero();
    let (yaw, _, _) = transform.rotation.to_euler(EulerRot::YXZ);
    let horizontal_rotation = Quat::from_rotation_y(yaw);
    let world_direction = horizontal_rotation * Vec3::X * local_direction.x
        + Vec3::Y * local_direction.y
        + horizontal_rotation * Vec3::NEG_Z * local_direction.z;

    CameraFrameInput {
        translation: world_direction * settings.camera_move_speed * delta_seconds,
        yaw_delta: -mouse_delta.x * settings.mouse_sensitivity,
        pitch_delta: -mouse_delta.y * settings.mouse_sensitivity,
    }
}

fn apply_camera_frame_input(transform: &mut Transform, input: CameraFrameInput) {
    transform.translation += input.translation;
    apply_camera_rotation(transform, input.yaw_delta, input.pitch_delta);
}

fn apply_camera_rotation(transform: &mut Transform, yaw_delta: f32, pitch_delta: f32) {
    if yaw_delta == 0.0 && pitch_delta == 0.0 {
        return;
    }
    let (yaw, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
    let pitch_limit = std::f32::consts::FRAC_PI_2 - 0.01;
    transform.rotation = Quat::from_euler(
        EulerRot::YXZ,
        yaw + yaw_delta,
        (pitch + pitch_delta).clamp(-pitch_limit, pitch_limit),
        0.0,
    );
}

#[cfg(test)]
mod tests {
    use super::{
        ClientKeyBindings, ClientMarionetteInputSettings, MovementAction, apply_camera_rotation,
        camera_frame_input,
    };
    use bevy::prelude::{ButtonInput, EulerRot, KeyCode, Quat, Transform, Vec2, Vec3};

    #[test]
    fn mouse_motion_controls_yaw_and_pitch() {
        let keyboard = ButtonInput::default();
        let bindings = ClientKeyBindings::default();
        let transform = Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, 0.4, 0.7, 0.2));

        let input = camera_frame_input(
            &keyboard,
            &bindings,
            Vec2::new(20.0, 50.0),
            1.0,
            ClientMarionetteInputSettings::default(),
            &transform,
        );

        assert!((input.yaw_delta + 0.04).abs() < f32::EPSILON);
        assert!((input.pitch_delta + 0.1).abs() < f32::EPSILON);
    }

    #[test]
    fn camera_pitch_stays_strictly_between_vertical_limits() {
        let mut upward = Transform::default();
        apply_camera_rotation(&mut upward, 0.0, f32::MAX);
        let (_, upward_pitch, _) = upward.rotation.to_euler(EulerRot::YXZ);

        let mut downward = Transform::default();
        apply_camera_rotation(&mut downward, 0.0, -f32::MAX);
        let (_, downward_pitch, _) = downward.rotation.to_euler(EulerRot::YXZ);

        assert!(upward_pitch < std::f32::consts::FRAC_PI_2);
        assert!(downward_pitch > -std::f32::consts::FRAC_PI_2);
    }

    #[test]
    fn space_and_shift_move_along_world_vertical() {
        let settings = ClientMarionetteInputSettings {
            camera_move_speed: 1.0,
            ..Default::default()
        };
        let bindings = ClientKeyBindings::default();
        let transform = Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, 0.4, 0.7, 0.2));
        let mut keyboard = ButtonInput::default();
        keyboard.press(KeyCode::Space);
        let upward =
            camera_frame_input(&keyboard, &bindings, Vec2::ZERO, 1.0, settings, &transform);

        keyboard.release(KeyCode::Space);
        keyboard.press(KeyCode::ShiftLeft);
        let downward =
            camera_frame_input(&keyboard, &bindings, Vec2::ZERO, 1.0, settings, &transform);

        assert_eq!(upward.translation, bevy::prelude::Vec3::Y);
        assert_eq!(downward.translation, bevy::prelude::Vec3::NEG_Y);
    }

    #[test]
    fn forward_movement_uses_only_camera_horizontal_heading() {
        let settings = ClientMarionetteInputSettings {
            camera_move_speed: 1.0,
            ..Default::default()
        };
        let bindings = ClientKeyBindings::default();
        let yaw = 0.4;
        let transform = Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, yaw, 0.7, 0.2));
        let mut keyboard = ButtonInput::default();
        keyboard.press(KeyCode::KeyW);

        let forward =
            camera_frame_input(&keyboard, &bindings, Vec2::ZERO, 1.0, settings, &transform);
        let expected = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;

        assert!(forward.translation.abs_diff_eq(expected, f32::EPSILON));
        assert_eq!(forward.translation.y, 0.0);
    }

    #[test]
    fn bindings_form_an_ordered_unique_bipartite_graph() {
        let mut bindings = ClientKeyBindings::default();
        assert!(!bindings.bind(KeyCode::KeyW, MovementAction::MoveForward));
        assert!(bindings.bind(KeyCode::KeyW, MovementAction::MoveUp));
        assert_eq!(
            bindings.actions_for(KeyCode::KeyW),
            &[MovementAction::MoveForward, MovementAction::MoveUp]
        );
        let up_keys = bindings.keys_for(MovementAction::MoveUp);
        assert_eq!(up_keys.len(), 2);
        assert!(up_keys.contains(&KeyCode::KeyW));
        assert!(up_keys.contains(&KeyCode::Space));

        assert!(bindings.reorder(
            KeyCode::KeyW,
            MovementAction::MoveUp,
            MovementAction::MoveForward
        ));
        assert_eq!(
            bindings.actions_for(KeyCode::KeyW),
            &[MovementAction::MoveUp, MovementAction::MoveForward]
        );
        assert!(bindings.unbind(KeyCode::KeyW, MovementAction::MoveUp));
        assert!(!bindings.unbind(KeyCode::KeyW, MovementAction::MoveUp));
    }

    #[test]
    fn one_key_executes_its_complete_action_queue() {
        let mut bindings = ClientKeyBindings::default();
        bindings.bind(KeyCode::KeyW, MovementAction::MoveRight);
        let mut keyboard = ButtonInput::default();
        keyboard.press(KeyCode::KeyW);

        assert_eq!(
            bindings.movement_direction(&keyboard),
            Vec3::new(1.0, 0.0, 1.0)
        );
    }
}
