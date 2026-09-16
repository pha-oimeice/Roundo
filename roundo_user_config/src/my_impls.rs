use crate::EndpointConfig;
use roundo_toolbox::string_to_socket_addr;
use std::net::SocketAddr;

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
        string_to_socket_addr(&format!("{}:{}", self.host, self.quic_port))
    }
}
