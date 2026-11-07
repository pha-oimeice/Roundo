//! Authoritative test-Life bootstrap, Controller input, motion, and snapshot orchestration.

mod assembly;
use assembly::assemble_test_life_for_player;
mod bootstrap;
use bootstrap::*;
mod lifecycle;
mod projection;
use projection::*;
pub(crate) mod minimal_application;
pub use projection::{CreatureObservations, project_authorized_creature_observation};

use crate::{
    creature_snapshot::{CreatureSnapshotMetadata, encode_creature_snapshot},
    player_control::PlayerControlServerSet,
};
use bevy::prelude::{
    App, Commands, Component, Entity, FixedUpdate, IntoScheduleConfigs, On, Plugin, Query, Remove,
    Res, Resource, Transform, With, World,
};
use roundo_contracts::{ConnectionId, ControllerAccessLevel, ControllerBindingId, ControllerId};
use roundo_controller::{
    Controller, ControllerCommitScope, ControllerLifecycleError, ControllerLifecyclePlugin,
    ControllerRegistry, IntentDomain,
};
use roundo_creature::{
    BodyInstanceIds, BodyOrientation, CreatureLifeDataPlugin, CreatureMotionError,
    CreatureMotionExperienceState, CreatureMotionPlugin, CreatureMotionRecord, CreatureObservation,
    EnvironmentSample, ExecutionBodyId, FactoryMotionRoute, FactoryMotionRouteValidator,
    GazeCommandEvent, GazeCommandState, GazeOrientation, KinematicVelocity, LocomotionState,
    MovementCommandState, TestLifePrototype, install_factory_motion_route_validator,
};
use roundo_life::{
    AuthoritativeFixedTick, ConceptBodyHandle, ConceptBodyPrototype, EventError, EventPolicy,
    FactoryError, FactoryInvalidation, LifeCommitScope, LifeFixedSet, LifeHandle, LifePrototype,
    LifeRuntimePlugin, ScopedLifeError, StateError, StatePolicy,
};
use roundo_local_coordinate::{LocalCoordinateServerIpc, VirtualChunkEnvironmentMap};
use roundo_marionette::ServerMarionetteSnapshotSender;
use roundo_player::{Player, PlayerRegistry};
use std::collections::HashMap;

const SNAPSHOT_CONNECTION_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SnapshotMetadataCandidate {
    route: FactoryMotionRoute,
    tick: u64,
    player_id: roundo_contracts::PlayerId,
    connection_generation: u64,
    movement_controller: ControllerId,
    gaze_controller: ControllerId,
    acknowledged_movement: u64,
    acknowledged_gaze: u64,
    movement_binding: Option<ControllerBindingId>,
    gaze_binding: Option<ControllerBindingId>,
}

#[derive(Resource, Default)]
struct SnapshotMetadataCandidates {
    tick: Option<u64>,
    by_connection: HashMap<ConnectionId, SnapshotMetadataCandidate>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SnapshotPublicationAttempt {
    tick: u64,
    player_id: roundo_contracts::PlayerId,
    connection_generation: u64,
}

#[derive(Resource, Default)]
struct SnapshotPublicationState {
    latest_by_connection: HashMap<ConnectionId, SnapshotPublicationAttempt>,
}

#[derive(Clone, Copy)]
struct ActivePlayerController {
    entity: Entity,
    controller_id: ControllerId,
    player_id: roundo_contracts::PlayerId,
    connection_generation: Option<u64>,
    binding: Option<ControllerBindingId>,
    valid: bool,
}

#[derive(Clone, Copy)]
struct LiveCreatureController {
    life: LifeHandle,
    route: FactoryMotionRoute,
    domain: IntentDomain,
    controller_ids: TestLifeControllers,
}

#[derive(Clone, Copy)]
struct SnapshotSubject {
    route: FactoryMotionRoute,
    tick: u64,
    player_id: roundo_contracts::PlayerId,
    connection_generation: u64,
    movement_controller: ControllerId,
    gaze_controller: ControllerId,
    movement_binding: Option<ControllerBindingId>,
    gaze_binding: Option<ControllerBindingId>,
}

/// The two hard-coded, orthogonal Controller resources created for one test Life.
#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
struct TestLifeControllers {
    movement: roundo_contracts::ControllerId,
    gaze: roundo_contracts::ControllerId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CreatureAssemblyMembers {
    pub(crate) intent: ConceptBodyHandle,
    pub(crate) execution: ConceptBodyHandle,
    pub(crate) record: ConceptBodyHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CreatureRecipeKind {
    LegacyControlled,
    MinimalControllerFree,
}

#[derive(Clone, Copy)]
struct CreatureAssemblyEntry {
    members: CreatureAssemblyMembers,
    kind: CreatureRecipeKind,
}

/// Composition-owned routing metadata; it does not define Life identity or existence.
#[derive(Resource, Default)]
pub(crate) struct CreatureAssemblyRegistry {
    by_life: HashMap<LifeHandle, CreatureAssemblyEntry>,
    life_by_body: HashMap<ConceptBodyHandle, LifeHandle>,
    life_by_controller: HashMap<roundo_contracts::ControllerId, LifeHandle>,
}
impl CreatureAssemblyRegistry {
    fn insert(
        &mut self,
        life: LifeHandle,
        members: CreatureAssemblyMembers,
    ) -> Result<(), CreatureAssemblyError> {
        self.insert_recipe(life, members, CreatureRecipeKind::LegacyControlled)
    }
    fn insert_minimal(
        &mut self,
        life: LifeHandle,
        members: CreatureAssemblyMembers,
    ) -> Result<(), CreatureAssemblyError> {
        self.insert_recipe(life, members, CreatureRecipeKind::MinimalControllerFree)
    }
    fn insert_recipe(
        &mut self,
        life: LifeHandle,
        members: CreatureAssemblyMembers,
        kind: CreatureRecipeKind,
    ) -> Result<(), CreatureAssemblyError> {
        if self.by_life.contains_key(&life)
            || [members.intent, members.execution, members.record]
                .into_iter()
                .any(|b| self.life_by_body.contains_key(&b))
        {
            return Err(CreatureAssemblyError::DuplicateAssembly);
        }
        self.by_life
            .insert(life, CreatureAssemblyEntry { members, kind });
        for body in [members.intent, members.execution, members.record] {
            self.life_by_body.insert(body, life);
        }
        Ok(())
    }
    fn remove_life(&mut self, life: LifeHandle) -> Option<CreatureAssemblyMembers> {
        let members = self.by_life.remove(&life)?.members;
        for body in [members.intent, members.execution, members.record] {
            self.life_by_body.remove(&body);
        }
        self.life_by_controller.retain(|_, mapped| *mapped != life);
        Some(members)
    }
    pub(crate) fn life_for_body(&self, body: ConceptBodyHandle) -> Option<LifeHandle> {
        self.life_by_body.get(&body).copied()
    }
    fn insert_with_controllers(
        &mut self,
        life: LifeHandle,
        members: CreatureAssemblyMembers,
        controllers: TestLifeControllers,
    ) -> Result<(), CreatureAssemblyError> {
        // Validate both derived indexes before publishing either of them.
        if controllers.movement == controllers.gaze
            || self.life_by_controller.contains_key(&controllers.movement)
            || self.life_by_controller.contains_key(&controllers.gaze)
        {
            return Err(CreatureAssemblyError::DuplicateAssembly);
        }
        self.insert(life, members)?;
        self.life_by_controller.insert(controllers.movement, life);
        self.life_by_controller.insert(controllers.gaze, life);
        Ok(())
    }
    fn life_for_controller(
        &self,
        controller: roundo_contracts::ControllerId,
    ) -> Option<LifeHandle> {
        self.life_by_controller.get(&controller).copied()
    }
    pub(crate) fn get(&self, life: LifeHandle) -> Option<CreatureAssemblyMembers> {
        self.by_life.get(&life).map(|entry| entry.members)
    }
    pub(crate) fn accepts_controller_input(&self, life: LifeHandle) -> bool {
        self.by_life
            .get(&life)
            .is_some_and(|entry| entry.kind == CreatureRecipeKind::LegacyControlled)
    }
}

impl FactoryMotionRouteValidator for CreatureAssemblyRegistry {
    fn contains_route(&self, route: FactoryMotionRoute) -> bool {
        self.get(route.life()).is_some_and(|members| {
            members.intent == route.intent()
                && members.execution == route.execution()
                && members.record == route.record()
        })
    }
}

#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
struct BootstrappedTestLife {
    life: LifeHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CreatureAssemblyError {
    Factory(FactoryError),
    Controller(roundo_controller::RegisterControllerError),
    Motion(CreatureMotionError),
    Event(EventError),
    State(StateError),
    DuplicateAssembly,
    InvalidBootstrap,
    ScopedLifecycle(ScopedLifeError),
    ControllerLifecycle(ControllerLifecycleError),
}

impl From<ControllerLifecycleError> for CreatureAssemblyError {
    fn from(value: ControllerLifecycleError) -> Self {
        Self::ControllerLifecycle(value)
    }
}
impl From<ScopedLifeError> for CreatureAssemblyError {
    fn from(value: ScopedLifeError) -> Self {
        Self::ScopedLifecycle(value)
    }
}
impl From<FactoryError> for CreatureAssemblyError {
    fn from(value: FactoryError) -> Self {
        Self::Factory(value)
    }
}
impl From<roundo_controller::RegisterControllerError> for CreatureAssemblyError {
    fn from(value: roundo_controller::RegisterControllerError) -> Self {
        Self::Controller(value)
    }
}
impl From<CreatureMotionError> for CreatureAssemblyError {
    fn from(value: CreatureMotionError) -> Self {
        Self::Motion(value)
    }
}
impl From<EventError> for CreatureAssemblyError {
    fn from(value: EventError) -> Self {
        Self::Event(value)
    }
}
impl From<StateError> for CreatureAssemblyError {
    fn from(value: StateError) -> Self {
        Self::State(value)
    }
}

/// Owns the authoritative Creature lifecycle behind one server-composition interface.
///
/// Player, Controller, Creature Motion, Local Coordinate, and transport remain
/// independent modules. Their ordering and test-bootstrap policy are composed here.
pub(crate) struct AuthoritativeCreaturePlugin;

impl AuthoritativeCreaturePlugin {
    pub(crate) fn new(_local_coordinate: LocalCoordinateServerIpc) -> Self {
        Self
    }
}

impl Plugin for AuthoritativeCreaturePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<CreatureLifeDataPlugin>() {
            app.add_plugins(CreatureLifeDataPlugin);
        }
        if !app.is_plugin_added::<LifeRuntimePlugin>() {
            app.add_plugins(LifeRuntimePlugin::default());
        }
        if !app.is_plugin_added::<ControllerLifecyclePlugin>() {
            app.add_plugins(ControllerLifecyclePlugin);
        }
        app.add_observer(cleanup_factory_invalidation);
        if !app.is_plugin_added::<minimal_application::MinimalLifeApplicationPlugin>() {
            install_factory_motion_route_validator::<CreatureAssemblyRegistry>(app)
                .expect("one Creature assembly adapter per App");
        }
        if !app.is_plugin_added::<CreatureMotionPlugin>() {
            app.add_plugins(CreatureMotionPlugin);
        }
        app.init_resource::<VirtualChunkEnvironmentMap>()
            .init_resource::<CreatureAssemblyRegistry>()
            .init_resource::<SnapshotMetadataCandidates>()
            .init_resource::<SnapshotPublicationState>()
            .add_observer(cleanup_removed_player_assembly)
            .add_observer(cleanup_removed_controller_assembly)
            .configure_sets(
                FixedUpdate,
                PlayerControlServerSet::Commands
                    .after(LifeFixedSet::CommitFactories)
                    .before(roundo_creature::CreatureLifeSet::CommandProjection),
            )
            .add_systems(
                FixedUpdate,
                sample_creature_environment.before(roundo_creature::CreatureLifeSet::MotionCommit),
            )
            .add_systems(
                FixedUpdate,
                capture_creature_snapshot_metadata
                    .after(PlayerControlServerSet::Commands)
                    .after(roundo_creature::CreatureLifeSet::CommandProjection)
                    .before(roundo_creature::CreatureLifeSet::MotionCommit),
            )
            .add_systems(
                FixedUpdate,
                (
                    publish_creature_snapshots.in_set(roundo_creature::CreatureLifeSet::Projection),
                    project_creature_presence
                        .after(roundo_creature::CreatureLifeSet::RecordUpdate)
                        .after(roundo_presence::PresenceServerSet::Commands)
                        .before(roundo_presence::PresenceServerSet::PlayerStates),
                ),
            );
    }
}

/// Ensures one non-spatial Player owns exactly one development test Life.
///
/// The operation is synchronous so a same-tick Access request cannot observe a
/// newly created Player before its grants exist. It grants Access only; neither
/// Controller is acquired here.
pub(crate) fn ensure_test_life_for_player(
    world: &mut World,
    player_id: roundo_contracts::PlayerId,
) {
    let player_entity = world
        .resource::<PlayerRegistry>()
        .player_entity(player_id)
        .expect("resolved Player has an authoritative ECS entity");
    if world.get::<BootstrappedTestLife>(player_entity).is_some() {
        return;
    }

    assemble_test_life_for_player(
        world,
        player_entity,
        bevy::prelude::Vec3::Y,
        EnvironmentSample::default(),
    )
    .expect("test Creature assembly is valid");
}

/// Only composition-owned derived facts are mutable here. Factory identity and
/// Controller lifecycle remain in their owning modules. Structural marker
/// removal uses targeted Commands, never an arbitrary World callback.
#[derive(bevy::ecs::system::SystemParam)]
struct AssemblyCleanup<'w, 's> {
    commands: Commands<'w, 's>,
    assemblies: bevy::prelude::ResMut<'w, CreatureAssemblyRegistry>,
    candidates: Option<bevy::prelude::ResMut<'w, SnapshotMetadataCandidates>>,
    markers: Query<'w, 's, (Entity, &'static BootstrappedTestLife)>,
    execution_controllers: Query<'w, 's, (Entity, &'static TestLifeControllers)>,
}
impl AssemblyCleanup<'_, '_> {
    fn remove_mapping(&mut self, life: LifeHandle) {
        if let Some(candidates) = self.candidates.as_mut() {
            candidates
                .by_connection
                .retain(|_, candidate| candidate.route.life() != life);
        }
        let controller_ids = self
            .assemblies
            .life_by_controller
            .iter()
            .filter_map(|(&controller, &mapped)| (mapped == life).then_some(controller))
            .collect::<Vec<_>>();
        if self.assemblies.remove_life(life).is_none() {
            return;
        }
        for (entity, marker) in &self.markers {
            if marker.life == life {
                self.commands
                    .entity(entity)
                    .try_remove::<BootstrappedTestLife>();
            }
        }
        for (entity, ids) in &self.execution_controllers {
            if controller_ids.contains(&ids.movement) || controller_ids.contains(&ids.gaze) {
                self.commands
                    .entity(entity)
                    .try_remove::<TestLifeControllers>();
            }
        }
    }
}

fn cleanup_factory_invalidation(
    invalidation: On<FactoryInvalidation>,
    mut cleanup: AssemblyCleanup,
) {
    let life = match *invalidation {
        FactoryInvalidation::Life(life) => Some(life),
        FactoryInvalidation::ConceptBody(body) => cleanup.assemblies.life_for_body(body),
    };
    if let Some(life) = life {
        cleanup.remove_mapping(life);
    }
}

fn cleanup_removed_player_assembly(removal: On<Remove, Player>, mut cleanup: AssemblyCleanup) {
    let life = cleanup
        .markers
        .get(removal.entity)
        .ok()
        .map(|(_, marker)| marker.life);
    if let Some(life) = life {
        cleanup.remove_mapping(life);
    }
}

fn cleanup_removed_controller_assembly(
    removal: On<Remove, Controller>,
    controllers: Query<&Controller>,
    mut cleanup: AssemblyCleanup,
) {
    let life = controllers
        .get(removal.entity)
        .ok()
        .and_then(|controller| cleanup.assemblies.life_for_controller(controller.id()));
    if let Some(life) = life {
        cleanup.remove_mapping(life);
    }
}

fn sample_creature_environment(
    environment: Res<VirtualChunkEnvironmentMap>,
    mut creatures: Query<(&Transform, &mut EnvironmentSample), With<ExecutionBodyId>>,
) {
    for (transform, mut sample) in &mut creatures {
        if let Some((values, revision)) = environment.sample(transform.translation) {
            *sample = EnvironmentSample::new(
                values.gravity,
                values.static_friction,
                values.kinetic_friction,
                revision,
            )
            .expect("environment map publishes validated values");
        }
    }
}
