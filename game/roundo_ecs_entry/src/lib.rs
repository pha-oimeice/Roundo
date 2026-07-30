#[cfg(feature = "client")]
mod client;
#[cfg(feature = "server")]
mod server;

#[cfg(feature = "client")]
pub use client::{
    client_character_ipc, client_marionette_ipc, client_static_voxel_ipc, create_ecs_client_app,
    run_ecs_client,
};
#[cfg(feature = "server")]
pub use server::{
    configure_server_tick_rate, run_ecs_server, server_character_ipc, server_marionette_ipc,
    server_static_voxel_ipc,
};
