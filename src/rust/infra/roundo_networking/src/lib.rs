//! Typed Serde messages over `roundo_quinnet`. No game protocols, ECS mappings,
//! business identities, persistence, automatic retries, or application callbacks.
//!
//! Both endpoints must agree on their send/receive Rust schemas. The host owns
//! the Tokio runtime, TLS policy, message limit, and all application semantics.

extern crate self as roundo_networking;

use serde::{Serialize, de::DeserializeOwned};
use std::{marker::PhantomData, net::SocketAddr};

pub use roundo_networking_derive::NetworkData;

// Generated implementations resolve Serde through this crate, so consumers do
// not need a direct Serde dependency just to derive NetworkData.
#[doc(hidden)]
pub use serde as __serde;

pub use roundo_quinnet::{
    ClientConfig, ConnectionError, MessageLimit, ServerConfig, TransportConfig,
};

#[derive(Debug, thiserror::Error)]
pub enum TransportationLayerError {
    #[error(transparent)]
    Transport(#[from] roundo_quinnet::Error),
    #[error("message codec failed: {0}")]
    Codec(#[from] postcard::Error),
    #[error("received message contains trailing bytes")]
    TrailingBytes,
}

/// Client endpoint. TLS configuration is supplied explicitly, never inferred
/// from application settings or changed to an insecure fallback.
#[derive(Clone)]
pub struct Client(roundo_quinnet::Client);

impl Client {
    pub fn bind(address: SocketAddr, config: ClientConfig) -> Result<Self, TransportationLayerError> {
        Ok(Self(roundo_quinnet::Client::bind(address, config)?))
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TransportationLayerError> {
        Ok(self.0.local_addr()?)
    }

    pub async fn connect<Send, Receive>(
        &self,
        address: SocketAddr,
        server_name: &str,
        limit: MessageLimit,
    ) -> Result<Connection<Send, Receive>, TransportationLayerError> {
        Ok(Connection::new(
            self.0.connect(address, server_name, limit).await?,
        ))
    }

    pub async fn wait_idle(&self) {
        self.0.wait_idle().await;
    }
}

/// Server endpoint. Accepted connections expose only current peer addresses;
/// the infrastructure does not issue a second connection or user identity.
#[derive(Clone)]
pub struct Server(roundo_quinnet::Server);

impl Server {
    pub fn bind(address: SocketAddr, config: ServerConfig) -> Result<Self, TransportationLayerError> {
        Ok(Self(roundo_quinnet::Server::bind(address, config)?))
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TransportationLayerError> {
        Ok(self.0.local_addr()?)
    }

    pub async fn accept<Send, Receive>(
        &self,
        limit: MessageLimit,
    ) -> Result<Option<Connection<Send, Receive>>, TransportationLayerError> {
        Ok(self.0.accept(limit).await?.map(Connection::new))
    }

    pub fn close(&self, code: u32, reason: &[u8]) {
        self.0.close(code, reason);
    }

    pub async fn wait_idle(&self) {
        self.0.wait_idle().await;
    }
}

// Bound serialization itself, not just the completed allocation passed to QUIC.
struct MessageBuffer {
    bytes: Vec<u8>,
    limit: usize,
}

impl postcard::ser_flavors::Flavor for MessageBuffer {
    type Output = Vec<u8>;

    fn try_push(&mut self, byte: u8) -> postcard::Result<()> {
        self.try_extend(&[byte])
    }

    fn try_extend(&mut self, bytes: &[u8]) -> postcard::Result<()> {
        if bytes.len() > self.limit - self.bytes.len() {
            return Err(postcard::Error::SerializeBufferFull);
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn finalize(self) -> postcard::Result<Self::Output> {
        Ok(self.bytes)
    }
}

/// A typed physical connection. The send and receive schemas may differ.
/// Clones share streams and consume the same receive FIFO.
pub struct Connection<Send, Receive> {
    transport: roundo_quinnet::Connection,
    schemas: PhantomData<fn(Send) -> Receive>,
}

impl<Send, Receive> Clone for Connection<Send, Receive> {
    fn clone(&self) -> Self {
        Self::new(self.transport.clone())
    }
}

impl<Send, Receive> Connection<Send, Receive> {
    fn new(transport: roundo_quinnet::Connection) -> Self {
        Self {
            transport,
            schemas: PhantomData,
        }
    }

    pub fn peer_addr(&self) -> SocketAddr {
        self.transport.peer_addr()
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TransportationLayerError> {
        Ok(self.transport.local_addr()?)
    }

    /// Serializes by reference, without an intermediate business DTO. Success
    /// means QUIC submission, not remote deserialization or application commit.
    pub async fn send(&self, message: &Send) -> Result<(), TransportationLayerError>
    where
        Send: Serialize,
    {
        let bytes = postcard::serialize_with_flavor(
            message,
            MessageBuffer {
                bytes: Vec::new(),
                limit: self.transport.message_limit().bytes(),
            },
        )?;
        Ok(self.transport.send(&bytes).await?)
    }

    /// Decodes one complete message. Codec failures and trailing bytes close
    /// the connection rather than being treated as missing application data.
    /// Cancellation while awaiting bytes is safe; the transport retains progress.
    pub async fn receive(&self) -> Result<Receive, TransportationLayerError>
    where
        Receive: DeserializeOwned,
    {
        let bytes = self.transport.receive().await?;
        let result = postcard::take_from_bytes(&bytes)
            .map_err(TransportationLayerError::Codec)
            .and_then(|(message, rest)| {
                if rest.is_empty() {
                    Ok(message)
                } else {
                    Err(TransportationLayerError::TrailingBytes)
                }
            });
        if result.is_err() {
            self.transport.close(1, b"invalid serialized message");
        }
        result
    }

    pub fn close(&self, code: u32, reason: &[u8]) {
        self.transport.close(code, reason);
    }

    pub async fn closed(&self) -> ConnectionError {
        self.transport.closed().await
    }
}
