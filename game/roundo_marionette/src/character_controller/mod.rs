mod client;
mod server;

pub use self::{
    client::{
        ClientMarionetteCommand, ClientMarionetteEvent, ClientMarionetteIpc, MarionetteClientPlugin,
    },
    server::{
        CharacterCapabilities, CharacterMotor, LocomotionCapability, MarionetteServerPlugin,
        ServerMarionetteCommand, ServerMarionetteEvent, ServerMarionetteIpc,
    },
};
