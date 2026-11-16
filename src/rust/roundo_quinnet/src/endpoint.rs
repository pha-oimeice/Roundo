use crate::{Connection, Error, MessageLimit};
use std::net::SocketAddr;

/// Outbound QUIC endpoint. Construct and use it within a live Tokio runtime.
#[derive(Clone)]
pub struct Client(quinn::Endpoint);

impl Client {
    /// Binds a UDP socket using the supplied TLS client configuration.
    pub fn bind(address: SocketAddr, config: quinn::ClientConfig) -> Result<Self, Error> {
        let mut endpoint = quinn::Endpoint::client(address)?;
        endpoint.set_default_client_config(config);
        Ok(Self(endpoint))
    }

    pub fn local_addr(&self) -> Result<SocketAddr, Error> {
        Ok(self.0.local_addr()?)
    }

    /// Returns only after the TLS handshake succeeds. No application handshake,
    /// assigned identity, authentication, or automatic reconnection is added.
    pub async fn connect(
        &self,
        address: SocketAddr,
        server_name: &str,
        limit: MessageLimit,
    ) -> Result<Connection, Error> {
        let connection = self.0.connect(address, server_name)?.await?;
        Ok(Connection::new(self.0.clone(), connection, limit))
    }

    /// Waits for all connections retained by this endpoint to finish closing.
    pub async fn wait_idle(&self) {
        self.0.wait_idle().await;
    }
}

/// Inbound QUIC endpoint. TLS certificates and transport settings are host-owned.
#[derive(Clone)]
pub struct Server(quinn::Endpoint);

impl Server {
    pub fn bind(address: SocketAddr, config: quinn::ServerConfig) -> Result<Self, Error> {
        Ok(Self(quinn::Endpoint::server(config, address)?))
    }

    pub fn local_addr(&self) -> Result<SocketAddr, Error> {
        Ok(self.0.local_addr()?)
    }

    /// Accepts a successfully handshaken peer. `None` means the endpoint closed;
    /// a failed handshake is an error, never a published connection. Multiple
    /// accept futures may run concurrently to avoid serializing handshakes.
    pub async fn accept(&self, limit: MessageLimit) -> Result<Option<Connection>, Error> {
        let Some(incoming) = self.0.accept().await else {
            return Ok(None);
        };
        let connection = incoming.await?;
        Ok(Some(Connection::new(self.0.clone(), connection, limit)))
    }

    /// Immediately closes the endpoint and its connections; this is not a
    /// delivery receipt for previously submitted messages.
    pub fn close(&self, code: u32, reason: &[u8]) {
        self.0.close(code.into(), reason);
    }

    pub async fn wait_idle(&self) {
        self.0.wait_idle().await;
    }
}
