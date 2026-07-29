mod config_serialization;
pub mod fs;
mod size_printer;
mod templates;
mod traits;

#[allow(unused_imports)]
pub use self::{config_serialization::load_or_create_config, templates::*, traits::*};
use std::net::{SocketAddr, ToSocketAddrs};

pub struct ErrorWithData<T> {
    pub data: Option<T>,
    pub err_msg: &'static str,
}

#[inline(always)]
pub fn string_to_socket_addr(addr: &String) -> SocketAddr {
    addr.to_socket_addrs()
        .unwrap_or_else(|e| panic!("Failed to resolve address: {}\n{}", addr, e))
        .next()
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::AddrNotAvailable))
        .unwrap_or_else(|e| panic!("Failed to get socket address: {}", e))
}

pub fn init_logger() {
    let filter_level = if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    env_logger::Builder::new()
        .default_format()
        .format_level(true)
        .filter_module("marionette", filter_level)
        .filter_module("roundo", filter_level)
        .write_style(env_logger::WriteStyle::Always)
        .init();
}
