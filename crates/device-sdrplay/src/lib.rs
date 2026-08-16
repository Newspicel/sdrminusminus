use std::{
    ffi::{c_int, c_void},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use sdrmm_device::{
    DeviceDriver, DeviceError, RxSink, SILENT_STREAM_TIMEOUT, SdrDevice, Worker,
    check_stream_settings,
};
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings};

mod api;
mod caps;
mod ffi;
mod model;
mod settings;
mod stream;
#[cfg(test)]
mod testing;

use api::{DevHandle, InitOutcome};
pub use api::{Sdrplay, shared};
pub use model::DRIVER_ID;
use model::{DuoMode, Model};
use stream::{StreamContext, StreamState};

const STARTUP_SAMPLE_RATE_HZ: f64 = 2_000_000.0;
const MASTER_WAIT: Duration = Duration::from_secs(30);
const MASTER_POLL: Duration = Duration::from_millis(200);
const MONITOR_POLL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug)]
pub struct RuntimeInfo {
    pub version: Option<f32>,
    pub library: Option<String>,
    pub error: Option<String>,
}

#[must_use]
pub fn runtime_info() -> RuntimeInfo {
    match shared() {
        Ok(api) => RuntimeInfo {
            version: Some(api.version()),
            library: Some(api.library_path().display().to_string()),
            error: None,
        },
        Err(error) => RuntimeInfo {
            version: None,
            library: None,
            error: Some(error),
        },
    }
}

#[derive(Default)]
pub struct SdrplayDriver {
    api: Option<Arc<dyn Sdrplay>>,
}

impl SdrplayDriver {
    #[must_use]
    pub fn new() -> Self {
        Self { api: None }
    }

    #[must_use]
    pub fn with_api(api: Arc<dyn Sdrplay>) -> Self {
        Self { api: Some(api) }
    }

    fn api(&self) -> Option<Arc<dyn Sdrplay>> {
        match &self.api {
            Some(api) => Some(api.clone()),
            None => match shared() {
                Ok(api) => Some(api),
                Err(error) => {
                    tracing::debug!("the SDRplay API is unavailable: {error}");
                    None
                }
            },
        }
    }
}

fn offers_mode(device: &ffi::DeviceT, mode: Option<DuoMode>) -> bool {
    match mode {
        Some(mode) => model::duo_modes(device.rsp_duo_mode, device.tuner).contains(&mode),
        None => true,
    }
}

impl DeviceDriver for SdrplayDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        let Some(api) = self.api() else {
            return Vec::new();
        };
        match api.get_devices() {
            Ok(devices) => devices.iter().flat_map(model::describe).collect(),
            Err(error) => {
                tracing::warn!("sdrplay enumerate failed: {error}");
                Vec::new()
            }
        }
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        let api = self.api().ok_or_else(|| DeviceError::NotFound(info.id()))?;
        let (serial, mode) = model::split_key(&info.key);
        let mut device = api
            .get_devices()?
            .into_iter()
            .find(|device| device.serial() == serial && offers_mode(device, mode))
            .ok_or_else(|| DeviceError::NotFound(info.id()))?;
        let model = Model::from_hw_ver(device.hw_ver).ok_or_else(|| {
            DeviceError::Unsupported(format!("hardware version {}", device.hw_ver))
        })?;
        if let Some(mode) = mode {
            device.rsp_duo_mode = mode.api_mode();
            if mode != DuoMode::Slave {
                device.tuner = mode.tuner();
            }
            if mode.is_low_if() && mode != DuoMode::Slave {
                device.rsp_duo_sample_freq = model::DUO_DUAL_TUNER_FS_HZ;
            }
        }
        api.select_device(&mut device)?;
        match SdrplayDevice::new(api.clone(), device, model, mode) {
            Ok(device) => Ok(Box::new(device)),
            Err(error) => {
                let _ = api.release_device(&mut device);
                Err(error)
            }
        }
    }
}

struct SendPtr<T>(*mut T);

unsafe impl<T> Send for SendPtr<T> {}

fn target_at<'a>(
    tree: *mut ffi::DeviceParamsT,
    model: Model,
    mode: Option<DuoMode>,
    tuner: c_int,
) -> Result<settings::Target<'a>, DeviceError> {
    if tree.is_null() {
        return Err(DeviceError::Io(
            "the SDRplay device has no parameters".to_string(),
        ));
    }
    let (dev, channel) = unsafe {
        let channel = if tuner == ffi::TUNER_B {
            (*tree).rx_channel_b
        } else {
            (*tree).rx_channel_a
        };
        if (*tree).dev_params.is_null() || channel.is_null() {
            return Err(DeviceError::Io(
                "the SDRplay device is missing a parameter block".to_string(),
            ));
        }
        (&mut *(*tree).dev_params, &mut *channel)
    };
    Ok(settings::Target {
        model,
        mode,
        dev,
        channel,
    })
}

fn read_band(
    tree: *mut ffi::DeviceParamsT,
    model: Model,
    mode: Option<DuoMode>,
    tuner: c_int,
) -> Result<caps::Band, DeviceError> {
    Ok(target_at(tree, model, mode, tuner)?.band())
}

pub struct SdrplayDevice {
    api: Arc<dyn Sdrplay>,
    device: ffi::DeviceT,
    handle: DevHandle,
    params: SendPtr<ffi::DeviceParamsT>,
    model: Model,
    mode: Option<DuoMode>,
    capabilities: Capabilities,
    settings: DeviceSettings,
    worker: Worker,
}

unsafe impl Send for SdrplayDevice {}

impl SdrplayDevice {
    fn new(
        api: Arc<dyn Sdrplay>,
        device: ffi::DeviceT,
        model: Model,
        mode: Option<DuoMode>,
    ) -> Result<Self, DeviceError> {
        let handle = DevHandle(device.dev);
        let params = SendPtr(api.device_params(handle)?);
        let band = read_band(params.0, model, mode, ffi::TUNER_A)?;
        let mut this = Self {
            api,
            device,
            handle,
            params,
            model,
            mode,
            capabilities: caps::capabilities(model, mode, band),
            settings: DeviceSettings::default(),
            worker: Worker::new(),
        };
        let start = DeviceSettings {
            sample_rate: Some(STARTUP_SAMPLE_RATE_HZ),
            ..DeviceSettings::default()
        };
        this.apply(&start)?;
        Ok(this)
    }

    fn tuner_for(&self, stream: u32) -> c_int {
        match self.mode {
            Some(DuoMode::DualTuner) => {
                if stream == 0 {
                    ffi::TUNER_A
                } else {
                    ffi::TUNER_B
                }
            }
            Some(DuoMode::Slave) => {
                if self.device.tuner == ffi::TUNER_B {
                    ffi::TUNER_B
                } else {
                    ffi::TUNER_A
                }
            }
            Some(mode) => mode.tuner(),
            None => ffi::TUNER_A,
        }
    }

    fn target(&mut self, tuner: c_int) -> Result<settings::Target<'_>, DeviceError> {
        target_at(self.params.0, self.model, self.mode, tuner)
    }

    fn refresh_capabilities(&mut self) -> Result<(), DeviceError> {
        let band = self.target(self.tuner_for(0))?.band();
        self.capabilities = caps::capabilities(self.model, self.mode, band);
        Ok(())
    }

    fn read_settings(&mut self) -> Result<DeviceSettings, DeviceError> {
        let mut settings = settings::read(&self.target(self.tuner_for(0))?);
        if self.capabilities.rx_streams > 1 {
            let mut streams = Vec::new();
            for stream in 0..self.capabilities.rx_streams {
                let read = settings::read(&self.target(self.tuner_for(stream))?);
                streams.push(sdrmm_wire::StreamSettings {
                    stream,
                    center_hz: read.center_hz,
                    gains: read.gains,
                    antenna: None,
                });
            }
            settings.streams = streams;
        }
        Ok(settings)
    }

    fn wait_for_master(
        &self,
        state: &StreamState,
        context: *mut c_void,
    ) -> Result<(), DeviceError> {
        let callbacks = StreamContext::callbacks();
        let deadline = Instant::now() + MASTER_WAIT;
        while Instant::now() < deadline {
            std::thread::sleep(MASTER_POLL);
            if !state.master_ready() {
                continue;
            }
            match self.api.init(self.handle, &callbacks, context)? {
                InitOutcome::Started => return Ok(()),
                InitOutcome::MasterPending => {}
            }
        }
        Err(DeviceError::Unsupported(
            "this RSPduo slave is waiting for a master application to start streaming".to_string(),
        ))
    }

    fn monitor(
        api: Arc<dyn Sdrplay>,
        handle: DevHandle,
        state: Arc<StreamState>,
        context: *mut StreamContext,
        running: &AtomicBool,
    ) {
        let mut seen = state.samples();
        let mut last_change = Instant::now();
        let mut reason = None;
        while running.load(Ordering::Acquire) {
            std::thread::sleep(MONITOR_POLL);
            if let Some(fatal) = state.fatal() {
                reason = Some(fatal);
                break;
            }
            let samples = state.samples();
            if samples != seen {
                seen = samples;
                last_change = Instant::now();
            } else if last_change.elapsed() >= SILENT_STREAM_TIMEOUT {
                reason = Some(format!(
                    "the SDRplay stream delivered nothing for {} seconds",
                    SILENT_STREAM_TIMEOUT.as_secs()
                ));
                break;
            }
        }
        let _ = api.uninit(handle);
        // sdrplay_api_Uninit stops the API's own callback threads before it returns, so the
        // context is ours again and the sinks can be touched from here.
        let mut context = unsafe { Box::from_raw(context) };
        if let Some(reason) = reason {
            context.fail_sinks(&reason);
        }
    }
}

impl SdrDevice for SdrplayDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, delta: &DeviceSettings) -> Result<(), DeviceError> {
        check_stream_settings(delta, &self.capabilities)?;
        let streaming = self.worker.is_running();
        let capabilities = self.capabilities.clone();
        for stream in 0..capabilities.rx_streams {
            let resolved = delta.for_stream(stream, &capabilities.per_stream);
            let tuner = self.tuner_for(stream);
            let applied = {
                let mut target = self.target(tuner)?;
                settings::apply(&mut target, &resolved, &capabilities)?
            };
            if streaming && !applied.reasons.is_empty() {
                self.api.update(
                    self.handle,
                    tuner,
                    applied.reasons.reason,
                    applied.reasons.ext1,
                )?;
            }
        }
        self.settings.merge_from(delta);
        self.refresh_capabilities()?;
        let actual = self.read_settings()?;
        self.settings.merge_from(&actual);
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let streams = self.capabilities.rx_streams as usize;
        if sinks.len() != streams {
            return Err(DeviceError::Unsupported(format!(
                "this device has {streams} rx streams, got {} sinks",
                sinks.len()
            )));
        }
        if self.worker.is_running() {
            return Err(DeviceError::AlreadyStreaming);
        }
        let state = StreamState::new(self.api.clone(), self.handle);
        let context = Box::into_raw(StreamContext::new(sinks, state.clone()));
        let started = self
            .api
            .init(self.handle, &StreamContext::callbacks(), context.cast())
            .and_then(|outcome| match outcome {
                InitOutcome::Started => Ok(()),
                InitOutcome::MasterPending => self.wait_for_master(&state, context.cast()),
            });
        if let Err(error) = started {
            let _ = self.api.uninit(self.handle);
            drop(unsafe { Box::from_raw(context) });
            return Err(error);
        }
        let api = self.api.clone();
        let handle = self.handle;
        let pointer = SendPtr(context);
        if let Err(error) = self.worker.start("sdrmm-sdrplay-rx", move |running| {
            let pointer = pointer;
            Self::monitor(api, handle, state, pointer.0, running);
        }) {
            let _ = self.api.uninit(self.handle);
            drop(unsafe { Box::from_raw(context) });
            return Err(error);
        }
        Ok(())
    }

    fn rx_stop(&mut self) {
        self.worker.stop();
    }
}

impl Drop for SdrplayDevice {
    fn drop(&mut self) {
        self.rx_stop();
        let _ = self.api.release_device(&mut self.device);
    }
}

#[cfg(test)]
mod tests;
