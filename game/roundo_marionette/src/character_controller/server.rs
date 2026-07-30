use bevy::prelude::{
    App, Commands, Component, Entity, Fixed, FixedUpdate, IntoScheduleConfigs, Plugin, Query, Res,
    ResMut, Resource, Time, Transform, Vec3,
};
use roundo_networking::protocol::{
    CharacterId, ConnectionId, ControllerAccessPolicy, ControllerCameraState, ControllerDescriptor,
    ControllerId, ControllerInput, ControllerKind, ControllerScope, LocomotionInput, UserSession,
    ViewInput,
};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::{HashMap, HashSet};

pub type ServerMarionetteIpc =
    CrossbeamThreadPipeEndpointA<ServerMarionetteCommand, ServerMarionetteEvent>;

#[derive(Clone)]
pub struct MarionetteServerPlugin {
    pipe: CrossbeamThreadPipe<ServerMarionetteCommand, ServerMarionetteEvent>,
}

impl MarionetteServerPlugin {
    pub fn new() -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
        }
    }

    pub fn ipc(&self) -> ServerMarionetteIpc {
        self.pipe.endpoint_a()
    }
}

impl Default for MarionetteServerPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for MarionetteServerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ControllerRegistry>()
            .insert_resource(ServerPipeResource(self.pipe.endpoint_b()))
            .add_systems(
                FixedUpdate,
                (process_server_commands, apply_locomotion_capabilities).chain(),
            );
    }
}

#[derive(Clone, Debug)]
pub enum ServerMarionetteCommand {
    SessionConnected {
        connection_id: ConnectionId,
        user_session: UserSession,
    },
    RegisterCharacter {
        character_id: CharacterId,
        capabilities: CharacterCapabilities,
    },
    RegisterController {
        controller: ControllerDescriptor,
    },
    RequestBinding {
        connection_id: ConnectionId,
        user_session: UserSession,
        controller_id: ControllerId,
    },
    ReleaseBinding {
        user_session: UserSession,
        controller_id: ControllerId,
    },
    SubmitInput {
        connection_id: ConnectionId,
        user_session: UserSession,
        controller_id: ControllerId,
        input: ControllerInput,
    },
}

#[derive(Clone, Debug)]
pub enum ServerMarionetteEvent {
    ControllerGranted {
        connection_id: ConnectionId,
        controller: ControllerDescriptor,
    },
    ControllerRevoked {
        user_session: UserSession,
        controller_id: ControllerId,
    },
    ViewCameraState {
        user_session: UserSession,
        controller_id: ControllerId,
        state: ControllerCameraState,
    },
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CharacterCapabilities {
    pub locomotion: bool,
}

#[derive(Component, Clone, Copy, Debug)]
struct CharacterIdentity(CharacterId);

#[derive(Component, Clone, Copy, Debug, Default)]
pub struct LocomotionCapability {
    pub world_direction: [f32; 3],
    pub jump_requested: bool,
    pub sprint_requested: bool,
}

#[derive(Component, Clone, Copy, Debug)]
pub struct CharacterMotor {
    pub walk_speed: f32,
    pub sprint_multiplier: f32,
}

impl Default for CharacterMotor {
    fn default() -> Self {
        Self {
            walk_speed: 4.0,
            sprint_multiplier: 1.5,
        }
    }
}

#[derive(Component, Clone, Copy, Debug)]
struct ControllerIdentity(ControllerId);

#[derive(Component, Clone, Debug)]
struct ControllerScopeComponent(ControllerScope);

#[derive(Component, Clone, Copy, Debug, Default)]
struct ViewControllerCameraState {
    state: ControllerCameraState,
}

#[derive(Resource, Clone)]
struct ServerPipeResource(
    CrossbeamThreadPipeEndpointB<ServerMarionetteCommand, ServerMarionetteEvent>,
);

#[derive(Resource, Default)]
struct ControllerRegistry {
    characters: HashMap<CharacterId, Entity>,
    controllers: HashMap<ControllerId, ControllerEntry>,
    bindings: HashSet<ControllerBinding>,
    input_sequences: HashMap<ConnectionInput, u64>,
}

#[derive(Clone)]
struct ControllerEntry {
    entity: Entity,
    descriptor: ControllerDescriptor,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ControllerBinding {
    user_session: UserSession,
    controller_id: ControllerId,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ConnectionInput {
    connection_id: ConnectionId,
    user_session: UserSession,
    controller_id: ControllerId,
}

fn process_server_commands(
    mut commands: Commands,
    pipe: Res<ServerPipeResource>,
    mut registry: ResMut<ControllerRegistry>,
    mut locomotion_capabilities: Query<&mut LocomotionCapability>,
    mut view_cameras: Query<&mut ViewControllerCameraState>,
) {
    while let Some(command) = pipe.0.try_receive() {
        match command {
            ServerMarionetteCommand::SessionConnected {
                connection_id,
                user_session,
            } => restore_session_bindings(
                &pipe.0,
                &registry,
                &mut view_cameras,
                connection_id,
                user_session,
            ),
            ServerMarionetteCommand::RegisterCharacter {
                character_id,
                capabilities,
            } => register_character(&mut commands, &mut registry, character_id, capabilities),
            ServerMarionetteCommand::RegisterController { controller } => {
                register_controller(&mut commands, &mut registry, controller)
            }
            ServerMarionetteCommand::RequestBinding {
                connection_id,
                user_session,
                controller_id,
            } => request_binding(
                &pipe.0,
                &mut registry,
                connection_id,
                user_session,
                controller_id,
            ),
            ServerMarionetteCommand::ReleaseBinding {
                user_session,
                controller_id,
            } => release_binding(&pipe.0, &mut registry, user_session, controller_id),
            ServerMarionetteCommand::SubmitInput {
                connection_id,
                user_session,
                controller_id,
                input,
            } => apply_controller_input(
                &pipe.0,
                &mut registry,
                connection_id,
                user_session,
                controller_id,
                input,
                &mut locomotion_capabilities,
                &mut view_cameras,
            ),
        }
    }
}

fn register_character(
    commands: &mut Commands,
    registry: &mut ControllerRegistry,
    character_id: CharacterId,
    capabilities: CharacterCapabilities,
) {
    if registry.characters.contains_key(&character_id) {
        return;
    }

    let mut entity = commands.spawn((
        CharacterIdentity(character_id),
        CharacterMotor::default(),
        Transform::default(),
    ));
    if capabilities.locomotion {
        entity.insert(LocomotionCapability::default());
    }
    registry.characters.insert(character_id, entity.id());
}

fn register_controller(
    commands: &mut Commands,
    registry: &mut ControllerRegistry,
    descriptor: ControllerDescriptor,
) {
    if registry.controllers.contains_key(&descriptor.controller_id) {
        return;
    }
    if !matches!(
        (&descriptor.kind, &descriptor.scope),
        (
            ControllerKind::Locomotion,
            ControllerScope::CharacterLocomotion { .. }
        ) | (ControllerKind::View, ControllerScope::View)
    ) {
        return;
    }

    let entity = match descriptor.kind {
        ControllerKind::Locomotion => commands
            .spawn((
                ControllerIdentity(descriptor.controller_id),
                ControllerScopeComponent(descriptor.scope.clone()),
            ))
            .id(),
        ControllerKind::View => commands
            .spawn((
                ControllerIdentity(descriptor.controller_id),
                ControllerScopeComponent(descriptor.scope.clone()),
                ViewControllerCameraState::default(),
            ))
            .id(),
    };

    registry.controllers.insert(
        descriptor.controller_id,
        ControllerEntry { entity, descriptor },
    );
}

fn request_binding(
    pipe: &CrossbeamThreadPipeEndpointB<ServerMarionetteCommand, ServerMarionetteEvent>,
    registry: &mut ControllerRegistry,
    connection_id: ConnectionId,
    user_session: UserSession,
    controller_id: ControllerId,
) {
    let Some(entry) = registry.controllers.get(&controller_id).cloned() else {
        return;
    };
    let binding = ControllerBinding {
        user_session,
        controller_id,
    };

    if entry.descriptor.access_policy == ControllerAccessPolicy::Exclusive
        && registry
            .bindings
            .iter()
            .any(|existing| existing.controller_id == controller_id && *existing != binding)
    {
        return;
    }
    registry.bindings.insert(binding);
    let _ = pipe.try_send(ServerMarionetteEvent::ControllerGranted {
        connection_id,
        controller: entry.descriptor,
    });
}

fn release_binding(
    pipe: &CrossbeamThreadPipeEndpointB<ServerMarionetteCommand, ServerMarionetteEvent>,
    registry: &mut ControllerRegistry,
    user_session: UserSession,
    controller_id: ControllerId,
) {
    if registry.bindings.remove(&ControllerBinding {
        user_session,
        controller_id,
    }) {
        registry.input_sequences.retain(|key, _| {
            key.controller_id != controller_id || key.user_session != user_session
        });
        let _ = pipe.try_send(ServerMarionetteEvent::ControllerRevoked {
            user_session,
            controller_id,
        });
    }
}

fn apply_controller_input(
    pipe: &CrossbeamThreadPipeEndpointB<ServerMarionetteCommand, ServerMarionetteEvent>,
    registry: &mut ControllerRegistry,
    connection_id: ConnectionId,
    user_session: UserSession,
    controller_id: ControllerId,
    input: ControllerInput,
    locomotion_capabilities: &mut Query<&mut LocomotionCapability>,
    view_cameras: &mut Query<&mut ViewControllerCameraState>,
) {
    let binding = ControllerBinding {
        user_session,
        controller_id,
    };
    let connection_input = ConnectionInput {
        connection_id,
        user_session,
        controller_id,
    };
    if !registry.bindings.contains(&binding) {
        return;
    }
    let Some(entry) = registry.controllers.get(&controller_id).cloned() else {
        return;
    };
    if entry.descriptor.access_policy == ControllerAccessPolicy::ReadOnly {
        return;
    }

    match (&entry.descriptor.kind, &entry.descriptor.scope, input) {
        (
            ControllerKind::Locomotion,
            ControllerScope::CharacterLocomotion { character_id },
            ControllerInput::Locomotion(input),
        ) => {
            let Some(character) = registry.characters.get(character_id) else {
                return;
            };
            let Ok(mut capability) = locomotion_capabilities.get_mut(*character) else {
                return;
            };
            if input.sequence
                <= registry
                    .input_sequences
                    .get(&connection_input)
                    .copied()
                    .unwrap_or_default()
                || !is_valid_locomotion_input(&input)
            {
                return;
            }
            capability.world_direction = normalize_direction(input.world_direction);
            capability.jump_requested = input.jump;
            capability.sprint_requested = input.sprint;
            registry
                .input_sequences
                .insert(connection_input, input.sequence);
        }
        (ControllerKind::View, ControllerScope::View, ControllerInput::View(input)) => {
            let Ok(mut camera) = view_cameras.get_mut(entry.entity) else {
                return;
            };
            if input.sequence
                <= registry
                    .input_sequences
                    .get(&connection_input)
                    .copied()
                    .unwrap_or_default()
                || !is_valid_view_input(&input)
            {
                return;
            }
            camera.state.translation = add_translation(camera.state.translation, input.translation);
            camera.state.yaw += input.yaw_delta;
            camera.state.pitch = (camera.state.pitch + input.pitch_delta).clamp(-1.55, 1.55);
            registry
                .input_sequences
                .insert(connection_input, input.sequence);
            let _ = pipe.try_send(ServerMarionetteEvent::ViewCameraState {
                user_session,
                controller_id,
                state: camera.state,
            });
        }
        _ => {}
    }
}

fn restore_session_bindings(
    pipe: &CrossbeamThreadPipeEndpointB<ServerMarionetteCommand, ServerMarionetteEvent>,
    registry: &ControllerRegistry,
    view_cameras: &mut Query<&mut ViewControllerCameraState>,
    connection_id: ConnectionId,
    user_session: UserSession,
) {
    for binding in registry
        .bindings
        .iter()
        .filter(|binding| binding.user_session == user_session)
    {
        let Some(controller) = registry.controllers.get(&binding.controller_id) else {
            continue;
        };
        let _ = pipe.try_send(ServerMarionetteEvent::ControllerGranted {
            connection_id,
            controller: controller.descriptor.clone(),
        });
        if controller.descriptor.kind == ControllerKind::View
            && let Ok(camera) = view_cameras.get_mut(controller.entity)
        {
            let _ = pipe.try_send(ServerMarionetteEvent::ViewCameraState {
                user_session,
                controller_id: binding.controller_id,
                state: camera.state,
            });
        }
    }
}

fn is_valid_locomotion_input(input: &LocomotionInput) -> bool {
    input.world_direction.iter().all(|value| value.is_finite())
}

fn is_valid_view_input(input: &ViewInput) -> bool {
    input.translation.iter().all(|value| value.is_finite())
        && input.yaw_delta.is_finite()
        && input.pitch_delta.is_finite()
}

fn normalize_direction(direction: [f32; 3]) -> [f32; 3] {
    let length_squared = direction.iter().map(|value| value * value).sum::<f32>();
    if length_squared <= 1.0 || length_squared == 0.0 {
        return direction;
    }
    let inverse_length = length_squared.sqrt().recip();
    direction.map(|value| value * inverse_length)
}

fn add_translation(current: [f32; 3], delta: [f32; 3]) -> [f32; 3] {
    [
        current[0] + delta[0],
        current[1] + delta[1],
        current[2] + delta[2],
    ]
}

fn apply_locomotion_capabilities(
    time: Res<Time<Fixed>>,
    mut characters: Query<(&LocomotionCapability, &CharacterMotor, &mut Transform)>,
) {
    for (capability, motor, mut transform) in &mut characters {
        let speed = motor.walk_speed
            * if capability.sprint_requested {
                motor.sprint_multiplier
            } else {
                1.0
            };
        transform.translation +=
            Vec3::from_array(capability.world_direction) * speed * time.delta_secs();
    }
}
