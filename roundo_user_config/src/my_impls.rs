use crate::EndpointConfig;
use std::net::{SocketAddr, ToSocketAddrs};

impl EndpointConfig {
    /// Resolves the configured host and QUIC port to the resolver's first address.
    ///
    /// Hostname resolution is synchronous and may consult the operating system's
    /// network resolver. Multiple addresses are not retained.
    ///
    /// # Panics
    ///
    /// Panics when the endpoint cannot be resolved or produces no address.
    #[inline]
    pub fn get_quic_addr(&self) -> SocketAddr {
        format!("{}:{}", self.host, self.quic_port)
            .to_socket_addrs()
            .unwrap_or_else(|error| panic!("failed to resolve configured endpoint: {error}"))
            .next()
            .unwrap_or_else(|| panic!("configured endpoint did not resolve to an address"))
    }
}
