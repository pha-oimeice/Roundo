//! Typed QUIC transport and Bevy synchronization infrastructure.
//!
//! Transport: [`Client`], [`Server`], [`Connection`].
//! Shared data: [`DataTransferObject`](trait@DataTransferObject), [`SharedEntities`].
//! Synchronization: [`Network`], [`Deadline`], [`BatchPolicy`].
//! Bevy integration: [`NetworkPlugin`], [`NetworkSystems`].

extern crate self as roundo_networking;

mod config;
mod data;
mod entities;
mod error;
mod network;
mod plugin;
mod transport;
mod wire;

pub use config::{BatchPolicy, Deadline};
pub use data::DataTransferObject;
pub use entities::{SharedEntities, SharedEntityId};
pub use error::{NetworkError, TransportationLayerError};
pub use network::{ComponentUpdate, LengthStatistics, Network, NetworkEvent, Peer, ResourceUpdate};
pub use plugin::{NetworkPlugin, NetworkSystems, flush_network, receive_network_updates};
pub use transport::{Client, Connection, Server};
pub use wire::{NetworkTypeId, SyncBatch, SyncConnection, UpdateKind};

pub use roundo_networking_proc_macro::DataTransferObject;
pub use roundo_quinnet::{
    ClientConfig, ConnectionError, MessageLimit, ServerConfig, TransportConfig,
};

// Public path required by DataTransferObject's generated implementations.
#[doc(hidden)]
pub use serde as __serde;
