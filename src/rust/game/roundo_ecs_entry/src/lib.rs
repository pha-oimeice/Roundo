//! Feature-gated constructors and IPC accessors for the game ECS runtime.

#[cfg(feature = "server")]
mod authoritative_creature;
#[cfg(feature = "server")]
mod block_item;
#[cfg(feature = "server")]
pub use block_item::{BlockItemEdits, BlockItemPermissions, create_block_item};
#[cfg(feature = "server")]
pub mod authority_admission;
#[cfg(feature = "server")]
pub use authoritative_creature::minimal_application::{
    MinimalCleanupError, MinimalLifeApplicationPlugin, MinimalLifeApplicationSet,
    MinimalLifeCompositionError, MinimalLifeIntents, MinimalLifeMembers,
    MinimalLifeParameterValues, MinimalLifeParameters, MinimalLifeStatus, MinimalLifeSubmitError,
    compose_minimal_life, minimal_life_status, unload_minimal_life,
};
#[cfg(feature = "server")]
pub use authoritative_creature::{CreatureObservations, project_authorized_creature_observation};
#[cfg(feature = "client")]
mod client;
#[cfg(feature = "client")]
mod creature_prediction;
#[cfg(any(feature = "client", feature = "server"))]
mod creature_snapshot;
#[cfg(feature = "server")]
mod debug;
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
    FreezeDispatchResult, FreezeDispatchResults, LocalAiOutcome, LocalAiTickets,
    PlayerControlCommand, PlayerControlEvent, PlayerControlServerIpc, PlayerControlServerPlugin,
    PrefixProofResult, SettlementCertificates, local_ai_request, local_ai_submit,
};

#[cfg(feature = "client")]
pub use client::{ClientEcsEndpoints, ClientEcsRuntime, create_ecs_client_app, run_ecs_client};
#[cfg(feature = "client")]
pub use creature_prediction::{CreaturePredictionPlugin, PredictionCleanupIpc};
/// Server runtime assembly and its domain IPC endpoints.
#[cfg(feature = "server")]
pub use server::{
    ServerEcsConfig, ServerEcsEndpoints, ServerEcsRuntime, create_ecs_server_app, run_ecs_server,
};
