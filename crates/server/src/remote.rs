use std::sync::{Arc, Mutex, MutexGuard};

use axum::Router;
use sdrmm_tunnel::{
    Config as TunnelConfig, DeviceKey, Status as TunnelStatus, Tunnel,
    pairing::{Paired, Pairing, PairingError, Started},
};
use sdrmm_wire::{DEFAULT_REMOTE_APP, RemoteState, RemoteStatus};
use tokio::task::JoinHandle;

use crate::store::{RemotePairing, Store, StoreError};

#[derive(Debug, thiserror::Error)]
pub(crate) enum RemoteError {
    #[error("already connected; disconnect first")]
    AlreadyPaired,
    #[error("{0}")]
    Pairing(#[from] PairingError),
    #[error("device key: {0}")]
    Key(#[from] sdrmm_tunnel::identity::IdentityError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

enum Phase {
    Unpaired,
    Pairing {
        started: Started,
        task: JoinHandle<()>,
    },
    Paired {
        device_id: String,
        tunnel: Tunnel,
    },
}

impl Drop for Phase {
    fn drop(&mut self) {
        if let Self::Pairing { task, .. } = self {
            task.abort();
        }
    }
}

struct Inner {
    router: Option<Router>,
    phase: Phase,
    error: Option<String>,
}

pub(crate) struct RemoteHub {
    app: String,
    store: Arc<Store>,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for RemoteHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteHub")
            .field("app", &self.app)
            .finish_non_exhaustive()
    }
}

pub fn device_name() -> String {
    whoami::devicename()
        .or_else(|_| whoami::hostname())
        .unwrap_or_default()
}

pub(crate) fn remote_state(status: &TunnelStatus) -> (RemoteState, Option<String>) {
    match status {
        TunnelStatus::Connecting => (RemoteState::Connecting, None),
        TunnelStatus::Online => (RemoteState::Online, None),
        TunnelStatus::Retrying { error, .. } => (RemoteState::Retrying, Some(error.clone())),
        TunnelStatus::Rejected { reason } => (RemoteState::Rejected, Some(reason.clone())),
    }
}

impl RemoteHub {
    pub(crate) fn new(app: Option<&url::Url>, store: Arc<Store>) -> Self {
        Self {
            app: app.map_or_else(|| DEFAULT_REMOTE_APP.to_string(), url::Url::to_string),
            store,
            inner: Mutex::new(Inner {
                router: None,
                phase: Phase::Unpaired,
                error: None,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn attach(&self, router: Router) {
        self.lock().router = Some(router);
        match self.store.remote_pairing() {
            Ok(Some(pairing)) => self.resume(&pairing),
            Ok(None) => {}
            Err(error) => tracing::error!(%error, "could not read the remote pairing"),
        }
    }

    pub(crate) fn shutdown(&self) {
        let mut inner = self.lock();
        inner.router = None;
        inner.phase = Phase::Unpaired;
    }

    pub(crate) fn status(&self, via_relay: bool) -> RemoteStatus {
        let inner = self.lock();
        let mut status = RemoteStatus {
            state: RemoteState::Unpaired,
            device_id: None,
            user_code: None,
            verification_uri: None,
            verification_uri_complete: None,
            error: inner.error.clone(),
            app_origin: self.origin(),
            via_relay,
        };
        match &inner.phase {
            Phase::Unpaired => {}
            Phase::Pairing { started, .. } => {
                status.state = RemoteState::Pairing;
                status.user_code = Some(started.user_code.clone());
                status.verification_uri = Some(started.verification_uri.clone());
                status.verification_uri_complete = Some(started.verification_uri_complete.clone());
            }
            Phase::Paired { device_id, tunnel } => {
                let (state, error) = remote_state(&tunnel.status().borrow());
                status.state = state;
                status.error = error;
                status.device_id = Some(device_id.clone());
            }
        }
        status
    }

    fn origin(&self) -> String {
        self.app.parse::<url::Url>().map_or_else(
            |_| self.app.clone(),
            |url| url.origin().ascii_serialization(),
        )
    }

    pub(crate) async fn pair(self: &Arc<Self>) -> Result<RemoteStatus, RemoteError> {
        if self.pairing_blocked() {
            return Err(RemoteError::AlreadyPaired);
        }
        let app: url::Url = self
            .app
            .parse()
            .map_err(|error| PairingError::Address(format!("{}: {error}", self.app)))?;
        let (key, document) = DeviceKey::generate()?;
        let pairing = match Pairing::start(&app, &key, &device_name()).await {
            Ok(pairing) => pairing,
            Err(error) => {
                self.lock().error = Some(error.to_string());
                return Err(error.into());
            }
        };
        let started = pairing.started().clone();
        let hub = Arc::clone(self);
        let task = tokio::spawn(async move {
            let outcome = pairing.wait().await;
            hub.finish(outcome, key, document);
        });
        let mut inner = self.lock();
        inner.error = None;
        inner.phase = Phase::Pairing { started, task };
        drop(inner);
        Ok(self.status(false))
    }

    fn pairing_blocked(&self) -> bool {
        match &self.lock().phase {
            Phase::Paired { tunnel, .. } => {
                !matches!(*tunnel.status().borrow(), TunnelStatus::Rejected { .. })
            }
            Phase::Unpaired | Phase::Pairing { .. } => false,
        }
    }

    fn finish(&self, outcome: Result<Paired, PairingError>, key: DeviceKey, document: Vec<u8>) {
        let paired = match outcome {
            Ok(paired) => paired,
            Err(error) => {
                tracing::info!(%error, "pairing with the remote app ended");
                let mut inner = self.lock();
                inner.error = Some(error.to_string());
                inner.phase = Phase::Unpaired;
                return;
            }
        };
        let pairing = RemotePairing::new(
            paired.device_id.clone(),
            paired.relay_url.to_string(),
            document,
        );
        if let Err(error) = self.store.save_remote_pairing(&pairing) {
            tracing::error!(%error, "could not keep the remote pairing");
            let mut inner = self.lock();
            inner.error = Some(error.to_string());
            inner.phase = Phase::Unpaired;
            return;
        }
        tracing::info!(device = %paired.device_id, "paired with the remote app");
        self.connect(paired.device_id, paired.relay_url, key);
    }

    fn resume(&self, pairing: &RemotePairing) {
        let key = match DeviceKey::from_pkcs8(&pairing.key) {
            Ok(key) => key,
            Err(error) => {
                tracing::error!(%error, "stored remote key is unusable; pair again");
                self.lock().error = Some(error.to_string());
                return;
            }
        };
        match pairing.relay_url.parse() {
            Ok(url) => self.connect(pairing.device_id.clone(), url, key),
            Err(error) => {
                tracing::error!(%error, "stored relay address is unusable; pair again");
                self.lock().error = Some(format!("relay address: {error}"));
            }
        }
    }

    fn connect(&self, device_id: String, url: url::Url, key: DeviceKey) {
        let mut inner = self.lock();
        let Some(router) = inner.router.clone() else {
            inner.error = Some("the server is shutting down".to_string());
            return;
        };
        if tokio::runtime::Handle::try_current().is_err() {
            tracing::error!("no async runtime to run remote access in");
            inner.error = Some("remote access could not start".to_string());
            return;
        }
        let config = TunnelConfig {
            url,
            key: Arc::new(key),
        };
        inner.error = None;
        inner.phase = Phase::Paired {
            device_id,
            tunnel: Tunnel::spawn(config, router),
        };
    }

    pub(crate) fn unpair(&self) -> Result<(), RemoteError> {
        let mut inner = self.lock();
        inner.error = None;
        inner.phase = Phase::Unpaired;
        drop(inner);
        self.store.forget_remote_pairing()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn tunnel_status_maps_onto_remote_state() {
        assert_eq!(
            remote_state(&TunnelStatus::Connecting),
            (RemoteState::Connecting, None)
        );
        assert_eq!(
            remote_state(&TunnelStatus::Online),
            (RemoteState::Online, None)
        );
        assert_eq!(
            remote_state(&TunnelStatus::Retrying {
                error: "down".to_string(),
                delay: Duration::from_secs(1),
            }),
            (RemoteState::Retrying, Some("down".to_string()))
        );
        assert_eq!(
            remote_state(&TunnelStatus::Rejected {
                reason: "device removed".to_string(),
            }),
            (RemoteState::Rejected, Some("device removed".to_string()))
        );
    }

    #[test]
    fn a_fresh_hub_is_unpaired_and_names_its_app() {
        let hub = RemoteHub::new(None, Arc::new(Store::open(None).expect("store")));
        let status = hub.status(true);
        assert_eq!(status.state, RemoteState::Unpaired);
        assert_eq!(status.app_origin, "https://app.sdrmm.com");
        assert!(status.via_relay);
        assert_eq!(status.device_id, None);
    }

    #[test]
    fn the_device_name_is_never_used_raw() {
        let name = sdrmm_tunnel::pairing::device_name(&device_name());
        assert!(!name.is_empty());
        assert!(name.chars().count() <= sdrmm_tunnel::pairing::MAX_NAME_CHARS);
    }
}
