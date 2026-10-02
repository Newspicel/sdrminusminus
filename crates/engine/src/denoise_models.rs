use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, PoisonError, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

use sdrmm_channels::{
    neural::Net,
    neural_denoise::{DenoiseNets, NeuralDenoiseError},
};
use sdrmm_wire::DenoiseModel;

use crate::EngineError;

const EXTENSION: &str = "sdrmmnn";

#[derive(Default)]
pub struct DenoiseModels {
    dir: RwLock<Option<PathBuf>>,
    loaded: Mutex<HashMap<DenoiseModel, Arc<Net>>>,
    generation: AtomicU64,
}

impl DenoiseModels {
    pub fn set_dir(&self, dir: PathBuf) {
        *self.dir.write().unwrap_or_else(PoisonError::into_inner) = Some(dir);
        self.forget_all();
    }

    #[must_use]
    pub fn path(&self, model: DenoiseModel) -> Option<PathBuf> {
        self.dir
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|dir| dir.join(format!("{}.{EXTENSION}", model.name())))
    }

    #[must_use]
    pub fn installed(&self, model: DenoiseModel) -> bool {
        self.path(model).is_some_and(|path| path.is_file())
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn install(&self, model: DenoiseModel, bytes: &[u8]) -> Result<(), EngineError> {
        let net = Net::load(bytes).map_err(|error| fail(model, &error))?;
        let path = self
            .path(model)
            .ok_or_else(|| EngineError::Audio("no data directory for models".into()))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|error| fail(model, &error))?;
        }
        let partial = path.with_extension("part");
        std::fs::write(&partial, bytes).map_err(|error| fail(model, &error))?;
        std::fs::rename(&partial, &path).map_err(|error| fail(model, &error))?;
        self.cache().insert(model, Arc::new(net));
        self.generation.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    pub fn remove(&self, model: DenoiseModel) -> Result<(), EngineError> {
        if let Some(path) = self.path(model).filter(|path| path.is_file()) {
            std::fs::remove_file(path).map_err(|error| fail(model, &error))?;
        }
        self.cache().remove(&model);
        self.generation.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn forget_all(&self) {
        self.cache().clear();
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<DenoiseModel, Arc<Net>>> {
        self.loaded.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn load(&self, model: DenoiseModel) -> Result<Arc<Net>, NeuralDenoiseError> {
        let missing = || NeuralDenoiseError(format!("{} is not downloaded", model.name()));
        let path = self.path(model).filter(|path| path.is_file()).ok_or_else(missing)?;
        let bytes = std::fs::read(&path).map_err(|error| NeuralDenoiseError(error.to_string()))?;
        Ok(Arc::new(Net::load(&bytes)?))
    }
}

impl DenoiseNets for DenoiseModels {
    fn net(&self, model: DenoiseModel) -> Result<Arc<Net>, NeuralDenoiseError> {
        if let Some(net) = self.cache().get(&model) {
            return Ok(Arc::clone(net));
        }
        let net = self.load(model)?;
        self.cache().insert(model, Arc::clone(&net));
        Ok(net)
    }
}

fn fail(model: DenoiseModel, error: &dyn std::fmt::Display) -> EngineError {
    EngineError::Audio(format!("{} model: {error}", model.name()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../channels/models/dpdfnet2.sdrmmnn"
        ))
        .expect("fixture is checked in")
    }

    #[test]
    fn a_model_is_missing_until_it_is_installed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let models = DenoiseModels::default();
        models.set_dir(dir.path().to_path_buf());
        assert!(!models.installed(DenoiseModel::Dpdfnet2));
        assert!(models.net(DenoiseModel::Dpdfnet2).is_err());
        let before = models.generation();
        models
            .install(DenoiseModel::Dpdfnet2, &fixture())
            .expect("installs");
        assert!(models.generation() > before);
        assert!(models.installed(DenoiseModel::Dpdfnet2));
        assert!(models.net(DenoiseModel::Dpdfnet2).is_ok());
    }

    #[test]
    fn a_broken_file_is_never_installed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let models = DenoiseModels::default();
        models.set_dir(dir.path().to_path_buf());
        assert!(models.install(DenoiseModel::Baseline, b"nonsense").is_err());
        assert!(!models.installed(DenoiseModel::Baseline));
    }

    #[test]
    fn a_removed_model_stops_loading() {
        let dir = tempfile::tempdir().expect("temp dir");
        let models = DenoiseModels::default();
        models.set_dir(dir.path().to_path_buf());
        models
            .install(DenoiseModel::Dpdfnet2, &fixture())
            .expect("installs");
        models.remove(DenoiseModel::Dpdfnet2).expect("removes");
        assert!(models.net(DenoiseModel::Dpdfnet2).is_err());
    }
}
