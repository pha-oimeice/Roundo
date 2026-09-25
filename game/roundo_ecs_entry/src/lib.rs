//! Feature-gated constructors and IPC accessors for the game ECS runtime.

#[cfg(feature = "server")]
mod authoritative_creature;
#[cfg(feature = "client")]
mod client;
#[cfg(feature = "client")]
mod creature_prediction;
#[cfg(any(feature = "client", feature = "server"))]
mod creature_snapshot;
#[cfg(feature = "server")]
mod player_control;
#[cfg(feature = "server")]
mod server;
#[cfg(feature = "server")]
pub use player_control::{PlayerControlCommand, PlayerControlEvent, PlayerControlServerIpc};

#[cfg(feature = "client")]
pub use client::{ClientEcsEndpoints, ClientEcsRuntime, create_ecs_client_app, run_ecs_client};
/// Server runtime assembly and its domain IPC endpoints.
#[cfg(feature = "server")]
pub use server::{
    ServerEcsConfig, ServerEcsEndpoints, ServerEcsRuntime, create_ecs_server_app, run_ecs_server,
};
