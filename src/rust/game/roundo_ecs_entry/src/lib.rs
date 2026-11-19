//! Feature-gated World constructors for the game ECS runtime.

#[cfg(feature = "server")]
mod authoritative_creature;
#[cfg(feature = "server")]
pub mod authority_admission;
#[cfg(feature = "server")]
mod block_item;
#[cfg(feature = "server")]
pub use authoritative_creature::minimal_application::{
    MinimalCleanupError, MinimalLifeApplicationPlugin, MinimalLifeApplicationSet,
    MinimalLifeCompositionError, MinimalLifeIntents, MinimalLifeMembers,
    MinimalLifeParameterValues, MinimalLifeParameters, MinimalLifeStatus,
    MinimalLifeSubmitError, compose_minimal_life, minimal_life_status, unload_minimal_life,
};
#[cfg(feature = "server")]
pub use authoritative_creature::{
    CreatureObservations, project_authorized_creature_observation,
};
#[cfg(feature = "client")]
mod client;
#[cfg(feature = "server")]
pub mod debug;
#[cfg(feature = "server")]
mod item_lifecycle;
#[cfg(feature = "server")]
pub use item_lifecycle::{ItemDeletionError, destroy_item};
#[cfg(feature = "server")]
pub mod physics;
#[cfg(feature = "server")]
mod player_control;
#[cfg(feature = "server")]
mod server;
#[cfg(feature = "server")]
pub mod world_items;
#[cfg(feature = "server")]
pub use player_control::{
    LocalAiControlPlugin, LocalAiInput, LocalAiOutcome, LocalAiTickets, local_ai_request,
    local_ai_submit,
};

#[cfg(feature = "client")]
pub use client::{create_ecs_client_app, run_ecs_client};
#[cfg(feature = "server")]
pub use server::{ServerEcsConfig, ServerEcsRuntime, create_ecs_server_app, run_ecs_server};
