mod adopted;
mod endpoint;
mod socket;
#[cfg(any(test, feature = "test-util"))]
pub mod testing;

pub use adopted::Adopted;
pub use endpoint::{CONNECT_TIMEOUT, Endpoint};
pub use socket::{Connection, Read, SocketStop};
