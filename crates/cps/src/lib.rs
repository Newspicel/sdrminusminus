pub mod anytone;
pub mod bits;
pub mod catalog;
pub mod convert;
pub mod discovery;
pub mod image;
pub mod merge;
pub mod model;
pub mod radtel;
pub mod registry;
pub mod serial;
pub mod tones;
pub mod transfer;

pub use image::{Image, Region, RegionKind};
pub use model::{RadioModel, RadioSession};
pub use registry::{ModelRegistry, model, models};
pub use serial::{SerialBackend, SerialLink, SystemSerial};
pub use transfer::{Progress, Transfer, TransferControl};

#[derive(Debug, thiserror::Error)]
pub enum CpsError {
    #[error("no radio model with id {0}")]
    UnknownModel(String),
    #[error("serial transport: {0}")]
    Transport(String),
    #[error("the radio sent {got} of {wanted} expected bytes before the timeout")]
    Timeout { wanted: usize, got: usize },
    #[error("the radio refused the {step} step: {reason}")]
    Protocol { step: &'static str, reason: String },
    #[error("{model} answered as {reported}, which is a different radio")]
    ModelMismatch { model: String, reported: String },
    #[error("the codeplug image has no data at {addr:#010x} ({len} bytes)")]
    MissingRegion { addr: u32, len: usize },
    #[error("codeplug: {0}")]
    Codeplug(String),
    #[error("the transfer was cancelled")]
    Cancelled,
}

impl CpsError {
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::UnknownModel(_))
    }
}
