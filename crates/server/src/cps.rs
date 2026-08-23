use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use sdrmm_cps::{
    CpsError, Image, Progress, RadioModel, SerialBackend, SystemSerial, Transfer, TransferControl,
    image::changed_blocks, model,
};
use sdrmm_wire::cps::{
    Codeplug, CpsCodeplugRequest, CpsJob, CpsJobKind, CpsJobState, CpsPort, CpsUser, RadioIdent,
};

use crate::store::{Store, StoreError};

const JOB_HISTORY: usize = 32;

#[derive(Debug, thiserror::Error)]
pub(crate) enum CpsJobError {
    #[error("{0}")]
    Radio(#[from] CpsError),
    #[error("{0}")]
    Store(#[from] StoreError),
    #[error("another radio transfer is already running")]
    Busy,
    #[error("job {0} not found")]
    NotFound(u64),
    #[error("job {0} has already finished")]
    Finished(u64),
    #[error("a write needs `confirm` set, because it replaces what the radio holds")]
    Unconfirmed,
    #[error("codeplug {0} was not read off a radio, so there is no image to restore")]
    NoStoredImage(i64),
}

struct Entry {
    job: Mutex<CpsJob>,
    cancel: AtomicBool,
}

struct Control {
    entry: Arc<Entry>,
}

impl TransferControl for Control {
    fn cancelled(&self) -> bool {
        self.entry.cancel.load(Ordering::Relaxed)
    }

    fn report(&self, progress: Progress) {
        if let Ok(mut job) = self.entry.job.lock() {
            job.step = progress.step;
            job.done_bytes = progress.done;
            job.total_bytes = progress.total;
        }
    }
}

pub(crate) struct CpsHub {
    jobs: Mutex<BTreeMap<u64, Arc<Entry>>>,
    next_id: AtomicU64,
    backend: Arc<dyn SerialBackend>,
    port_gate: Mutex<()>,
}

impl Default for CpsHub {
    fn default() -> Self {
        Self {
            jobs: Mutex::new(BTreeMap::new()),
            next_id: AtomicU64::new(1),
            backend: Arc::new(SystemSerial),
            port_gate: Mutex::new(()),
        }
    }
}

impl CpsHub {
    #[cfg(test)]
    pub(crate) fn with_backend(backend: Arc<dyn SerialBackend>) -> Self {
        Self {
            backend,
            ..Self::default()
        }
    }

    pub(crate) fn ports(&self) -> Result<(Vec<CpsPort>, Vec<String>), CpsJobError> {
        let ports = self.backend.ports().map_err(CpsError::Transport)?;
        Ok(sdrmm_cps::discovery::partition(ports, sdrmm_cps::models()))
    }

    pub(crate) fn identify(&self, model_id: &str, port: &str) -> Result<RadioIdent, CpsJobError> {
        let radio = self.model(model_id)?;
        let _busy = self.port_gate.try_lock().map_err(|_| CpsJobError::Busy)?;
        let link = self
            .backend
            .open(port, radio.baud())
            .map_err(CpsError::Transport)?;
        let mut session = radio.open(link)?;
        let ident = session.identify()?;
        session.finish()?;
        Ok(ident)
    }

    pub(crate) fn jobs(&self) -> Vec<CpsJob> {
        let jobs = self.lock();
        jobs.values()
            .filter_map(|entry| entry.job.lock().ok().map(|job| job.clone()))
            .collect()
    }

    pub(crate) fn job(&self, id: u64) -> Result<CpsJob, CpsJobError> {
        let jobs = self.lock();
        jobs.get(&id)
            .and_then(|entry| entry.job.lock().ok().map(|job| job.clone()))
            .ok_or(CpsJobError::NotFound(id))
    }

    pub(crate) fn cancel(&self, id: u64) -> Result<CpsJob, CpsJobError> {
        let entry = {
            let jobs = self.lock();
            jobs.get(&id).cloned().ok_or(CpsJobError::NotFound(id))?
        };
        let finished = entry
            .job
            .lock()
            .ok()
            .is_some_and(|job| job.state.is_final());
        if finished {
            return Err(CpsJobError::Finished(id));
        }
        entry.cancel.store(true, Ordering::Relaxed);
        self.job(id)
    }

    fn model(&self, id: &str) -> Result<&'static dyn RadioModel, CpsJobError> {
        model(id).ok_or_else(|| CpsJobError::Radio(CpsError::UnknownModel(id.to_owned())))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, Arc<Entry>>> {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn register(&self, job: CpsJob) -> Arc<Entry> {
        let entry = Arc::new(Entry {
            job: Mutex::new(job.clone()),
            cancel: AtomicBool::new(false),
        });
        let mut jobs = self.lock();
        jobs.insert(job.id, entry.clone());
        while jobs.len() > JOB_HISTORY {
            let Some(oldest) = jobs
                .iter()
                .filter(|(_, entry)| {
                    entry
                        .job
                        .lock()
                        .ok()
                        .is_some_and(|job| job.state.is_final())
                })
                .map(|(id, _)| *id)
                .next()
            else {
                break;
            };
            jobs.remove(&oldest);
        }
        entry
    }

    fn new_job(&self, kind: CpsJobKind, model_id: &str, port: &str) -> CpsJob {
        CpsJob {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            kind,
            model_id: model_id.to_owned(),
            port: port.to_owned(),
            state: CpsJobState::Running,
            step: "opening".to_owned(),
            done_bytes: 0,
            total_bytes: 0,
            started_at: crate::store::rfc3339_now(),
            finished_at: None,
            device_id: None,
            codeplug_id: None,
            radio: None,
            report: None,
            error: None,
        }
    }

    pub(crate) fn read(
        self: &Arc<Self>,
        store: &Arc<Store>,
        model_id: &str,
        port: &str,
        name: &str,
        device_id: Option<i64>,
        user_id: Option<i64>,
    ) -> Result<CpsJob, CpsJobError> {
        let radio = self.model(model_id)?;
        let mut job = self.new_job(CpsJobKind::Read, model_id, port);
        job.device_id = device_id;
        job.total_bytes = radio.transfer_bytes();
        let entry = self.register(job.clone());
        let hub = self.clone();
        let store = store.clone();
        let name = name.to_owned();
        let port = port.to_owned();
        let model_id = model_id.to_owned();
        std::thread::spawn(move || {
            let outcome = hub.run_read(
                radio, &store, &port, &name, &model_id, device_id, user_id, &entry,
            );
            finish(&entry, outcome);
        });
        Ok(job)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_read(
        &self,
        radio: &'static dyn RadioModel,
        store: &Store,
        port: &str,
        name: &str,
        model_id: &str,
        device_id: Option<i64>,
        user_id: Option<i64>,
        entry: &Arc<Entry>,
    ) -> Result<(), CpsJobError> {
        let _busy = self.port_gate.try_lock().map_err(|_| CpsJobError::Busy)?;
        let link = self
            .backend
            .open(port, radio.baud())
            .map_err(CpsError::Transport)?;
        let mut session = radio.open(link)?;
        let ident = session.identify()?;
        if let Ok(mut job) = entry.job.lock() {
            job.radio = Some(ident.clone());
        }
        let control = Control {
            entry: entry.clone(),
        };
        let image = Transfer::read(radio, session.as_mut(), &control)?;
        session.finish()?;

        let mut codeplug = radio.decode(&image)?;
        codeplug.meta.firmware = ident.firmware.clone();
        codeplug.meta.bands = ident.bands.clone();
        let id = store.store_cps_codeplug(
            &CpsCodeplugRequest {
                name: name.to_owned(),
                model_id: model_id.to_owned(),
                device_id,
                user_id,
                codeplug,
            },
            Some(&image.to_bytes()),
        )?;
        if let Ok(mut job) = entry.job.lock() {
            job.codeplug_id = Some(id);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn write(
        self: &Arc<Self>,
        store: &Arc<Store>,
        model_id: &str,
        port: &str,
        codeplug_id: i64,
        user: Option<CpsUser>,
        device_id: Option<i64>,
        confirm: bool,
        restore_image: bool,
    ) -> Result<CpsJob, CpsJobError> {
        if !confirm {
            return Err(CpsJobError::Unconfirmed);
        }
        let radio = self.model(model_id)?;
        let stored = store.cps_codeplug(codeplug_id)?;
        let backup = restore_image
            .then(|| store.cps_codeplug_image(codeplug_id))
            .transpose()?
            .flatten()
            .as_deref()
            .and_then(Image::from_bytes);
        if restore_image && backup.is_none() {
            return Err(CpsJobError::NoStoredImage(codeplug_id));
        }
        let mut codeplug = stored.codeplug;
        if let Some(user) = user {
            apply_user(&mut codeplug, &user);
        }
        let mut job = self.new_job(CpsJobKind::Write, model_id, port);
        job.device_id = device_id;
        job.codeplug_id = Some(codeplug_id);
        job.total_bytes = radio.transfer_bytes();
        let entry = self.register(job.clone());
        let hub = self.clone();
        let port = port.to_owned();
        std::thread::spawn(move || {
            let outcome = hub.run_write(radio, &port, codeplug, backup, &entry);
            finish(&entry, outcome);
        });
        Ok(job)
    }

    fn run_write(
        &self,
        radio: &'static dyn RadioModel,
        port: &str,
        codeplug: Codeplug,
        backup: Option<Image>,
        entry: &Arc<Entry>,
    ) -> Result<(), CpsJobError> {
        let _busy = self.port_gate.try_lock().map_err(|_| CpsJobError::Busy)?;
        let link = self
            .backend
            .open(port, radio.baud())
            .map_err(CpsError::Transport)?;
        let mut session = radio.open(link)?;
        let ident = session.identify()?;
        if let Ok(mut job) = entry.job.lock() {
            job.radio = Some(ident);
        }
        let control = Control {
            entry: entry.clone(),
        };
        let before = Transfer::read(radio, session.as_mut(), &control)?;
        let mut after = backup.unwrap_or_else(|| before.clone());
        let report = radio.encode(&codeplug, &mut after)?;
        if let Ok(mut job) = entry.job.lock() {
            job.report = Some(report);
            job.step = "writing".to_owned();
            job.done_bytes = 0;
            job.total_bytes = changed_blocks(&before, &after, session.block_size().max(1))
                .iter()
                .map(|(_, data)| data.len() as u64)
                .sum();
        }
        Transfer::write(radio, session.as_mut(), &before, &after, &control)?;
        session.finish()?;
        Ok(())
    }
}

fn finish(entry: &Arc<Entry>, outcome: Result<(), CpsJobError>) {
    let Ok(mut job) = entry.job.lock() else {
        return;
    };
    job.finished_at = Some(crate::store::rfc3339_now());
    match outcome {
        Ok(()) => {
            job.state = CpsJobState::Done;
            job.step = "done".to_owned();
            job.done_bytes = job.total_bytes;
        }
        Err(CpsJobError::Radio(error)) if error.is_cancelled() => {
            job.state = CpsJobState::Cancelled;
            job.step = "cancelled".to_owned();
        }
        Err(error) => {
            job.state = CpsJobState::Failed;
            job.step = "failed".to_owned();
            job.error = Some(error.to_string());
        }
    }
}

pub(crate) fn apply_user(codeplug: &mut Codeplug, user: &CpsUser) {
    let Some(number) = user.dmr_id else {
        return;
    };
    let name = user
        .callsign
        .clone()
        .filter(|callsign| !callsign.trim().is_empty())
        .unwrap_or_else(|| user.name.clone());
    match codeplug.radio_ids.first_mut() {
        Some(first) => {
            let old = first.name.clone();
            first.name.clone_from(&name);
            first.number = number;
            rename_radio_id(codeplug, &old, &name);
        }
        None => codeplug.radio_ids.push(sdrmm_wire::cps::RadioId {
            name: name.clone(),
            number,
        }),
    }
    codeplug.settings.default_radio_id = Some(name);
}

fn rename_radio_id(codeplug: &mut Codeplug, old: &str, new: &str) {
    if codeplug.settings.default_radio_id.as_deref() == Some(old) {
        codeplug.settings.default_radio_id = Some(new.to_owned());
    }
    for channel in &mut codeplug.channels {
        if let sdrmm_wire::cps::ChannelMode::Dmr(dmr) = &mut channel.mode
            && dmr.radio_id.as_deref() == Some(old)
        {
            dmr.radio_id = Some(new.to_owned());
        }
    }
}
