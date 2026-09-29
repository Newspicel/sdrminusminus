mod client;
pub mod frame;
pub mod identity;
pub mod pairing;
mod stream;
mod window;

pub use client::{Config, PING, PONG, Status, Tunnel, TunnelError};
pub use identity::DeviceKey;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relayed {
    pub user: String,
}
