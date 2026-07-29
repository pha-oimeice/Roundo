use crate::EndpointConfig;
use roundo_toolbox::string_to_socket_addr;
use std::net::SocketAddr;

impl EndpointConfig {
    #[inline]
    pub fn get_game_addr(&self) -> SocketAddr {
        string_to_socket_addr(&format!("{}:{}", self.host, self.game_port))
    }
    #[inline]
    pub fn get_https_addr(&self) -> SocketAddr {
        string_to_socket_addr(&format!("{}:{}", self.host, self.https_port))
    }
}
