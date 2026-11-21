//! Composes one fixed-tick authoritative ECS server as an independent World.

use crate::{
    authoritative_creature::AuthoritativeCreaturePlugin, player_control::LocalAiControlPlugin,
};
use avian3d::PhysicsPlugins;
use bevy::{
    MinimalPlugins,
    app::{App, Plugin},
    asset::AssetPlugin,
    mesh::MeshPlugin,
    prelude::{Fixed, Time, World},
};
use roundo_inventory::InventoryPlugin;
use roundo_local_coordinate::{
    AtomicVoxelRegistry, LocalCoordinateServerPlugin, PhysicsDemandBudget, VoxelEditBudget,
    WorldInteractionRuntimeBudgets,
};
use roundo_marionette::MarionetteServerPlugin;
use roundo_portal::RoundoPortalPlugin;

const DEFAULT_TICK_RATE: u32 = 20;

/// Installs the headless authoritative runtime on an App with a published voxel registry.
pub struct ServerEcsPlugin {
    /// FixedUpdate frequency in hertz; zero is normalized to one.
    pub tick_rate: u32,
}

impl Default for ServerEcsPlugin {
    fn default() -> Self {
        Self {
            tick_rate: DEFAULT_TICK_RATE,
        }
    }
}

impl Plugin for ServerEcsPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.world().contains_resource::<AtomicVoxelRegistry>(),
            "ServerEcsPlugin requires AtomicVoxelRegistry"
        );
        app.insert_resource(Time::<Fixed>::from_hz(f64::from(self.tick_rate.max(1))));
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
        crate::block_item::install(app);
        // Capability only: no sources, banks, accounts, or issuance.
        let currencies = roundo_economy::CurrencyBuilder::try_new()
            .expect("empty economy currency catalogue allocation failed")
            .freeze();
        let rules = roundo_economy::RuleBuilder::try_new(currencies)
            .expect("empty economy rule catalogue allocation failed")
            .freeze();
        roundo_economy::install_economy(app, rules)
            .expect("server economy capability installation failed");
        // Definitions only: native values remain optional ECS components. Seal
        // the complete catalogue before any ID-based attribute system initializes.
        let mut attributes = roundo_life::attributes::AttributeRegistryBuilder::new();
        roundo_life::attributes::register_attributes(&mut attributes)
            .expect("Life attribute definitions are unique");
        roundo_inventory::attributes::register_attributes(&mut attributes)
            .expect("storage attribute definitions are unique");
        attributes
            .install(app)
            .expect("server attribute registry installs once");
    }
}

/// Explicitly composes a development Creature for a World-local Player in a server World.
pub fn ensure_local_test_player(
    world: &mut World,
    key: roundo_player::PlayerKey,
) -> roundo_contracts::PlayerId {
    let player = roundo_player::find_or_create_player(world, key);
    crate::authoritative_creature::ensure_test_life_for_player(world, player);
    player
}
