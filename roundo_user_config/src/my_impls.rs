use crate::EndpointConfig;
use roundo_toolbox::string_to_socket_addr;
use std::net::SocketAddr;

impl EndpointConfig {
    #[inline]
    pub fn get_quic_addr(&self) -> SocketAddr {
        string_to_socket_addr(&format!("{}:{}", self.host, self.quic_port))
    }
}
