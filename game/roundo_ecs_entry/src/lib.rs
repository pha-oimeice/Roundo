mod assets;
#[cfg(feature = "client")]
mod client;
#[cfg(feature = "server")]
mod server;

#[cfg(feature = "client")]
pub use client::{
    client_local_coordinate_ipc, client_marionette_ipc, client_presence_ipc, create_ecs_client_app,
    run_ecs_client,
};
#[cfg(feature = "server")]
pub use server::{
    configure_server_presence_radius, configure_server_tick_rate, create_ecs_server_app,
    run_ecs_server, server_local_coordinate_ipc, server_marionette_ipc, server_presence_ipc,
};
