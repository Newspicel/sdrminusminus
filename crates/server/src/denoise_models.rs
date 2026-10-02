use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use reqwest::Client;
use sdrmm_engine::{DenoiseModels, Engine};
use sdrmm_wire::{DenoiseModel, DenoiseModelState, DenoiseModelStatus};
use sha2::{Digest, Sha256};

pub const MODELS_PREFIX: &str = "denoise/v1";
pub const MODELS_URL: &str = "https://downloads.sdrmm.com/denoise/v1";
const TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Artifact {
    pub bytes: u64,
    pub sha256: &'static str,
}

#[must_use]
pub fn artifact(model: DenoiseModel) -> Artifact {
    let (bytes, sha256) = match model {
        DenoiseModel::Baseline => (
            4_355_289,
            "521bd03e3b3e49bb9fd5937ec1222362b31779352d74967c44018fe90f86334e",
        ),
        DenoiseModel::Dpdfnet2 => (
            5_086_797,
            "25567253213f524449f39636cb3c9ea1a8869b6c584c08d80ff2111ae6541bb9",
        ),
        DenoiseModel::Dpdfnet4 => (
            5_817_413,
            "95cea940d1449ca13441fafe2c3dd23832692bff9f0d13755db38a982d6ef470",
        ),
        DenoiseModel::Dpdfnet8 => (
            7_278_645,
            "b19c7edfd3c1117aaf7d28b70ec508dd2bfb573d04406f3674bfc7e018a2fdbc",
        ),
        DenoiseModel::Dpdfnet2Narrow => (
            5_090_797,
            "a7863b2a439386e0aa888745b0da67c4571325346b810eb6251ae22d792b6e00",
        ),
        DenoiseModel::Dpdfnet8Narrow => (
            7_282_645,
            "9a9acae4e956ecf11e08b3aa961b1adede0ab7a3abe7c2343d27e024472d96f2",
        ),
        DenoiseModel::Dpdfnet2Full => (
            5_245_996,
            "1463d6cdc32a85ac085c99c93d088fe6f467c5ac82b467be218034a9e1341768",
        ),
        DenoiseModel::Dpdfnet8Full => (
            7_437_844,
            "4580d5e8b6a4aa09b6417b6815e97b6cf371c410dc02dee5d2934fc78e1a2968",
        ),
    };
    Artifact { bytes, sha256 }
}

#[must_use]
pub fn file_name(model: DenoiseModel) -> String {
    format!("{}.sdrmmnn", model.name())
}

#[derive(Default)]
pub(crate) struct Downloads {
    active: Mutex<HashMap<DenoiseModel, DenoiseModelState>>,
}

impl Downloads {
    pub(crate) fn statuses(&self, models: &DenoiseModels) -> Vec<DenoiseModelStatus> {
        let active = self.active();
        DenoiseModel::ALL
            .into_iter()
            .map(|model| DenoiseModelStatus {
                model,
                bytes: artifact(model).bytes,
                state: match active.get(&model) {
                    Some(state) => state.clone(),
                    None if models.installed(model) => DenoiseModelState::Ready,
                    None => DenoiseModelState::Missing,
                },
            })
            .collect()
    }

    pub(crate) fn start(self: &Arc<Self>, engine: &Arc<Engine>, model: DenoiseModel) -> bool {
        {
            let mut active = self.active();
            if matches!(
                active.get(&model),
                Some(DenoiseModelState::Downloading { .. })
            ) {
                return false;
            }
            active.insert(model, DenoiseModelState::Downloading { received: 0 });
        }
        let downloads = Arc::clone(self);
        let models = Arc::clone(engine.denoise_models());
        tokio::spawn(async move {
            let url = format!("{MODELS_URL}/{}", file_name(model));
            let outcome = match downloads.fetch(model, &url).await {
                Ok(bytes) => install(models, model, bytes).await,
                Err(error) => Err(error),
            };
            let mut active = downloads.active();
            match outcome {
                Ok(()) => {
                    active.remove(&model);
                }
                Err(error) => {
                    tracing::warn!(model = model.name(), %error, "denoise model download failed");
                    active.insert(model, DenoiseModelState::Failed { error });
                }
            }
        });
        true
    }

    pub(crate) fn forget(&self, model: DenoiseModel) {
        let mut active = self.active();
        if matches!(active.get(&model), Some(DenoiseModelState::Failed { .. })) {
            active.remove(&model);
        }
    }

    fn active(&self) -> std::sync::MutexGuard<'_, HashMap<DenoiseModel, DenoiseModelState>> {
        self.active.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn fetch(&self, model: DenoiseModel, url: &str) -> Result<Vec<u8>, String> {
        let expected = artifact(model);
        let client = Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|error| format!("no HTTP client: {error}"))?;
        let mut response = client
            .get(url)
            .send()
            .await
            .map_err(|error| format!("could not reach {url}: {error}"))?;
        if !response.status().is_success() {
            return Err(format!("{url} answered {}", response.status()));
        }
        let mut body = Vec::with_capacity(usize::try_from(expected.bytes).unwrap_or(0));
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| format!("download broke off: {error}"))?
        {
            body.extend_from_slice(&chunk);
            if body.len() as u64 > expected.bytes {
                return Err("download is larger than expected".into());
            }
            self.active().insert(
                model,
                DenoiseModelState::Downloading {
                    received: body.len() as u64,
                },
            );
        }
        let digest: String = Sha256::digest(&body)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if digest != expected.sha256 {
            return Err("download does not match its checksum".into());
        }
        Ok(body)
    }
}

async fn install(
    models: Arc<DenoiseModels>,
    model: DenoiseModel,
    bytes: Vec<u8>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || models.install(model, &bytes))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_fixture_matches_its_catalog_entry() {
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../channels/models/dpdfnet2.sdrmmnn"
        ))
        .expect("fixture is checked in");
        let expected = artifact(DenoiseModel::Dpdfnet2);
        assert_eq!(fixture.len() as u64, expected.bytes);
        let digest: String = Sha256::digest(&fixture)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(digest, expected.sha256);
    }

    #[test]
    fn the_url_names_the_upload_prefix() {
        assert!(MODELS_URL.ends_with(MODELS_PREFIX));
    }

    #[test]
    fn every_model_starts_missing_without_a_data_directory() {
        let statuses = Downloads::default().statuses(&DenoiseModels::default());
        assert_eq!(statuses.len(), DenoiseModel::ALL.len());
        assert!(
            statuses
                .iter()
                .all(|status| status.state == DenoiseModelState::Missing && status.bytes > 0)
        );
    }
}
