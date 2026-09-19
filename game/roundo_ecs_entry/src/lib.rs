//! Feature-gated constructors and IPC accessors for the game ECS runtime.

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "server")]
mod server;
#[cfg(feature = "server")]
mod server_integration;

#[cfg(feature = "client")]
pub use client::{ClientEcsEndpoints, ClientEcsRuntime, create_ecs_client_app, run_ecs_client};
/// Server runtime assembly and its domain IPC endpoints.
#[cfg(feature = "server")]
pub use server::{
    ServerEcsConfig, ServerEcsEndpoints, ServerEcsRuntime, create_ecs_server_app, run_ecs_server,
};
