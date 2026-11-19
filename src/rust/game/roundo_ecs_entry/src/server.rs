//! Composes one fixed-tick authoritative ECS server as an independent World.

use crate::{
    authoritative_creature::AuthoritativeCreaturePlugin, player_control::LocalAiControlPlugin,
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
    AtomicVoxelRegistry, LocalCoordinateServerPlugin, PhysicsDemandBudget, VoxelEditBudget,
    WorldInteractionRuntimeBudgets,
};
use roundo_marionette::MarionetteServerPlugin;
use roundo_portal::RoundoPortalPlugin;

const DEFAULT_TICK_RATE: u32 = 20;

/// Process-supplied settings captured when one server ECS App is assembled.
#[derive(Clone, Copy, Debug)]
pub struct ServerEcsConfig {
    /// FixedUpdate frequency in hertz; zero is normalized to one.
    pub tick_rate: u32,
}

impl Default for ServerEcsConfig {
    fn default() -> Self {
        Self {
            tick_rate: DEFAULT_TICK_RATE,
        }
    }
}

/// One fully assembled authoritative App.
pub struct ServerEcsRuntime {
    app: App,
}

impl ServerEcsRuntime {
    pub fn new(voxels: AtomicVoxelRegistry, config: ServerEcsConfig) -> Self {
        let mut app = App::new();
        app.insert_resource(voxels)
            .insert_resource(Time::<Fixed>::from_hz(f64::from(config.tick_rate.max(1))));
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            MeshPlugin,
            PhysicsPlugins::default()
                .with_collision_hooks::<crate::physics::StaticOnlyCollisionHooks>(),
            RoundoPortalPlugin,
            MarionetteServerPlugin,
            LocalAiControlPlugin,
            LocalCoordinateServerPlugin::default(),
            AuthoritativeCreaturePlugin,
            // Install finite rules only. No caller-owned instances are composed
            // or activated here.
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
        Self { app }
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
    /// polled after the next fixed tick.
    pub fn request_ai_acquire(
        &mut self,
        ai: roundo_controller::AiControllerId,
        controller: roundo_contracts::ControllerId,
    ) -> Result<
        roundo_controller::authority_gate::RequestId,
        roundo_controller::authority_gate::GateError,
    > {
        crate::player_control::local_ai_request(self.app.world_mut(), ai, controller, false)
    }

    pub fn request_ai_release(
        &mut self,
        ai: roundo_controller::AiControllerId,
        controller: roundo_contracts::ControllerId,
    ) -> Result<
        roundo_controller::authority_gate::RequestId,
        roundo_controller::authority_gate::GateError,
    > {
        crate::player_control::local_ai_request(self.app.world_mut(), ai, controller, true)
    }

    pub fn submit_ai(
        &mut self,
        ai: roundo_controller::AiControllerId,
        controller: roundo_contracts::ControllerId,
        input: crate::LocalAiInput,
    ) -> Result<(), roundo_controller::authority_gate::GateError> {
        crate::player_control::local_ai_submit(self.app.world_mut(), ai, controller, input)
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

    /// Explicitly composes a development Creature for a World-local Player.
    pub fn ensure_local_test_player(
        &mut self,
        key: roundo_player::PlayerKey,
    ) -> roundo_contracts::PlayerId {
        let world = self.app.world_mut();
        let player = roundo_player::find_or_create_player(world, key);
        crate::authoritative_creature::ensure_test_life_for_player(world, player);
        player
    }

    pub fn into_app(self) -> App {
        self.app
    }
}

pub fn create_ecs_server_app(voxels: AtomicVoxelRegistry) -> App {
    ServerEcsRuntime::new(voxels, ServerEcsConfig::default()).into_app()
}

/// Builds and runs a server backed by the built-in voxel registry.
pub fn run_ecs_server() {
    create_ecs_server_app(AtomicVoxelRegistry::builtin()).run();
}
