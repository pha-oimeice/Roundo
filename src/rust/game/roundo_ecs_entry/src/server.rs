//! Composes one fixed-tick authoritative ECS server and its bound IPC endpoints.

use crate::{
    authoritative_creature::AuthoritativeCreaturePlugin, player_control::PlayerControlServerPlugin,
};
use avian3d::PhysicsPlugins;
use bevy::{
    MinimalPlugins,
    app::App,
    asset::AssetPlugin,
    mesh::MeshPlugin,
    prelude::{Fixed, Time},
};
use roundo_inventory::InventoryPlugin;
use roundo_local_coordinate::{
    AtomicVoxelRegistry, LocalCoordinateServerIpc, LocalCoordinateServerPlugin,
    PhysicsDemandBudget, VoxelEditBudget, WorldInteractionRuntimeBudgets,
};
use roundo_marionette::{MarionetteServerPlugin, ServerMarionetteIpc};
use roundo_portal::RoundoPortalPlugin;
use roundo_presence::{PresenceServerIpc, PresenceServerSettings, RoundoPresenceServerPlugin};

const DEFAULT_TICK_RATE: u32 = 20;
const DEFAULT_PRESENCE_RADIUS: f32 = 128.0;

/// Process-supplied settings captured when one server ECS App is assembled.
#[derive(Clone, Copy, Debug)]
pub struct ServerEcsConfig {
    /// FixedUpdate frequency in hertz; zero is normalized to one.
    pub tick_rate: u32,
    /// Presence distance in world units; Presence normalizes non-finite/non-positive values.
    pub presence_radius: f32,
}

impl Default for ServerEcsConfig {
    fn default() -> Self {
        Self {
            tick_rate: DEFAULT_TICK_RATE,
            presence_radius: DEFAULT_PRESENCE_RADIUS,
        }
    }
}

/// Network-facing endpoints bound to one concrete server ECS App.
#[derive(Clone)]
pub struct ServerEcsEndpoints {
    /// Controller command endpoint connected to this runtime's Marionette plugin.
    pub marionette: ServerMarionetteIpc,
    /// Player lifecycle/snapshot endpoint connected to this runtime's Presence plugin.
    pub presence: PresenceServerIpc,
    /// Resource-stream endpoint connected to this runtime's Local Coordinate plugin.
    pub local_coordinate: LocalCoordinateServerIpc,
    /// Player identity, Access, and Controller-Control endpoint.
    pub player_control: crate::PlayerControlServerIpc,
}

/// One fully assembled authoritative App and exactly its plugin endpoints.
pub struct ServerEcsRuntime {
    app: App,
    endpoints: ServerEcsEndpoints,
}

impl ServerEcsRuntime {
    pub fn new(voxels: AtomicVoxelRegistry, config: ServerEcsConfig) -> Self {
        let presence = RoundoPresenceServerPlugin::new();
        let marionette = MarionetteServerPlugin::new();
        let local_coordinate = LocalCoordinateServerPlugin::new();
        let player_control = PlayerControlServerPlugin::new();
        let endpoints = ServerEcsEndpoints {
            marionette: marionette.ipc(),
            presence: presence.ipc(),
            local_coordinate: local_coordinate.ipc(),
            player_control: player_control.ipc(),
        };
        let mut app = App::new();
        app.insert_resource(voxels)
            .insert_resource(Time::<Fixed>::from_hz(f64::from(config.tick_rate.max(1))))
            .insert_resource(PresenceServerSettings::new(config.presence_radius));
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            MeshPlugin,
            PhysicsPlugins::default()
                .with_collision_hooks::<crate::physics::StaticOnlyCollisionHooks>(),
            RoundoPortalPlugin,
            marionette,
            player_control,
            presence,
            local_coordinate,
            AuthoritativeCreaturePlugin::new(endpoints.local_coordinate.clone()),
            // Install finite rules only. No caller-owned instances are composed
            // or activated here, nor by Connected/Access/old bootstrap.
            crate::MinimalLifeApplicationPlugin::new(
                64,
                WorldInteractionRuntimeBudgets::new(
                    VoxelEditBudget::new(64, 16).expect("explicit finite edit budget"),
                    PhysicsDemandBudget::new(64, 125, 16, 32)
                        .expect("explicit finite physics budget"),
                ),
            )
            .expect("application capacity fits demand budget"),
            // Public infrastructure only; ordinary Bodies still get no inventory.
            InventoryPlugin,
            // No world item or collector behavior is spawned by installation.
            crate::world_items::WorldItemsPlugin::new(4096),
            crate::debug::DebugPlugin,
        ));
        crate::block_item::install(&mut app);
        // Capability only: no sources, banks, accounts, or issuance.
        let currencies = roundo_economy::CurrencyBuilder::try_new()
            .expect("empty economy currency catalogue allocation failed")
            .freeze();
        let rules = roundo_economy::RuleBuilder::try_new(currencies)
            .expect("empty economy rule catalogue allocation failed")
            .freeze();
        roundo_economy::install_economy(&mut app, rules)
            .expect("server economy capability installation failed");
        // Definitions only: native values remain optional ECS components. Seal
        // the complete catalogue before any ID-based attribute system initializes.
        let mut attributes = roundo_life::attributes::AttributeRegistryBuilder::new();
        roundo_life::attributes::register_attributes(&mut attributes)
            .expect("Life attribute definitions are unique");
        roundo_inventory::attributes::register_attributes(&mut attributes)
            .expect("storage attribute definitions are unique");
        attributes
            .install(&mut app)
            .expect("server attribute registry installs once");
        Self { app, endpoints }
    }

    /// Register or retire a trusted server-local producer on this runtime.
    pub fn register_ai(
        &mut self,
    ) -> Result<roundo_controller::AiControllerId, roundo_controller::AiRegistrationError> {
        roundo_controller::register_ai(self.app.world_mut())
    }

    pub fn remove_ai(
        &mut self,
        ai: roundo_controller::AiControllerId,
    ) -> Result<usize, roundo_controller::ControllerControlError> {
        roundo_controller::remove_ai(self.app.world_mut(), ai)
    }

    /// Trusted local AI transition; request admission is synchronous, result is
    /// polled after the next fixed tick. No AI identity crosses the network wire.
    pub fn request_ai_acquire(
        &mut self,
        ai: roundo_controller::AiControllerId,
        controller: roundo_contracts::ControllerId,
    ) -> Result<
        roundo_controller::authority_gate::RequestId,
        roundo_controller::authority_gate::GateError,
    > {
        crate::player_control::local_ai_request(
            self.app.world_mut(),
            &self.endpoints.player_control,
            ai,
            controller,
            false,
        )
    }

    pub fn request_ai_release(
        &mut self,
        ai: roundo_controller::AiControllerId,
        controller: roundo_contracts::ControllerId,
    ) -> Result<
        roundo_controller::authority_gate::RequestId,
        roundo_controller::authority_gate::GateError,
    > {
        crate::player_control::local_ai_request(
            self.app.world_mut(),
            &self.endpoints.player_control,
            ai,
            controller,
            true,
        )
    }

    pub fn submit_ai(
        &mut self,
        ai: roundo_controller::AiControllerId,
        controller: roundo_contracts::ControllerId,
        input: roundo_contracts::DirectedControllerInput,
    ) -> Result<(), roundo_controller::authority_gate::GateError> {
        crate::player_control::local_ai_submit(
            self.app.world_mut(),
            &self.endpoints.player_control,
            ai,
            controller,
            input,
        )
    }

    pub fn take_ai_result(
        &mut self,
        request: roundo_controller::authority_gate::RequestId,
    ) -> Option<crate::player_control::LocalAiOutcome> {
        self.app
            .world_mut()
            .resource_mut::<crate::player_control::LocalAiTickets>()
            .take(request)
    }

    /// Separates the App from endpoints that address exactly its plugin instances.
    ///
    /// Dropping the App disconnects these channels; later runtimes create new queues.
    pub fn into_parts(self) -> (App, ServerEcsEndpoints) {
        (self.app, self.endpoints)
    }
}

/// Builds an App when external IPC access is not required.
pub fn create_ecs_server_app(voxels: AtomicVoxelRegistry) -> App {
    ServerEcsRuntime::new(voxels, ServerEcsConfig::default())
        .into_parts()
        .0
}

/// Builds and runs a server backed by the built-in voxel registry.
pub fn run_ecs_server() {
    create_ecs_server_app(AtomicVoxelRegistry::builtin()).run();
}
