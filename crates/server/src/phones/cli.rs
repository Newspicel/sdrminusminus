use std::{
    net::TcpStream,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use jiff::{Timestamp, tz::TimeZone};
use sdrmm_wire::{PairingOffer, phone::group_code};

use super::{
    PairError, Phones,
    gate::{candidate, endpoint, stored_listeners},
};
use crate::{Store, StoreError, net};

const LIVENESS_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, thiserror::Error)]
pub enum CliPairError {
    #[error("no SDR-- database at {}: start sdrmm first", .0.display())]
    NoDatabase(PathBuf),
    #[error("phones need HTTPS: turn on Allow phones or start with --tls-self-signed")]
    NoEndpoint,
    #[error("SDR-- is not running (port {0} closed)")]
    NotRunning(u16),
    #[error("{0}")]
    Pair(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<PairError> for CliPairError {
    fn from(error: PairError) -> Self {
        match error {
            PairError::Store(store) => Self::Store(store),
            other => Self::Pair(other.to_string()),
        }
    }
}

pub fn offer_for_cli(db: &Path, name: Option<&str>) -> Result<PairingOffer, CliPairError> {
    if !db.is_file() {
        return Err(CliPairError::NoDatabase(db.to_path_buf()));
    }
    let store = Arc::new(Store::open(Some(db))?);
    let records = stored_listeners(&store)?;
    let listener = candidate(&records).ok_or(CliPairError::NoEndpoint)?;
    let endpoint = endpoint(&records, &net::lan_addresses(), &net::mdns_host())
        .ok_or(CliPairError::NoEndpoint)?;
    if TcpStream::connect_timeout(&listener.reachable_here(), LIVENESS_TIMEOUT).is_err() {
        return Err(CliPairError::NotRunning(listener.port));
    }
    Ok(Phones::new(store).create_offer(name, &endpoint, &net::host_label(), Timestamp::now())?)
}

#[must_use]
pub fn caption(offer: &PairingOffer) -> String {
    format!(
        "Code {}\nKey  {}\nHost {}\nEnds {}",
        group_code(&offer.code),
        offer.key_check,
        offer.endpoint.hosts.join(", "),
        local_time(&offer.expires_at),
    )
}

fn local_time(at: &str) -> String {
    at.parse::<Timestamp>().map_or_else(
        |_| at.to_owned(),
        |at| {
            at.to_zoned(TimeZone::system())
                .strftime("%H:%M")
                .to_string()
        },
    )
}

#[cfg(test)]
mod tests;
