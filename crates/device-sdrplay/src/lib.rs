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
mod tests {
    use std::sync::mpsc;

    use sdrmm_wire::{ExtraValue, GainValue, StreamSettings};

    use super::*;
    use crate::testing::FakeApi;

    fn driver(api: Arc<FakeApi>) -> SdrplayDriver {
        SdrplayDriver::with_api(api)
    }

    fn open(api: &Arc<FakeApi>, key: &str) -> Box<dyn SdrDevice> {
        let driver = driver(api.clone());
        let info = driver
            .probe()
            .into_iter()
            .find(|info| info.key == key)
            .unwrap_or_else(|| panic!("{key} is not listed"));
        driver.open(&info).expect("open")
    }

    #[test]
    fn a_receiver_is_listed_once_with_its_model_in_the_label() {
        let api = Arc::new(FakeApi::rsp1a());
        let listed = driver(api).probe();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].driver, DRIVER_ID);
        assert_eq!(listed[0].key, "1234567890");
        assert_eq!(listed[0].label, "RSP1A 1234567890");
        assert_eq!(listed[0].serial.as_deref(), Some("1234567890"));
    }

    #[test]
    fn nothing_is_listed_when_the_vendor_api_is_missing() {
        if shared().is_err() {
            assert!(SdrplayDriver::new().probe().is_empty());
        }
    }

    #[test]
    fn opening_a_key_that_is_gone_reports_not_found() {
        let api = Arc::new(FakeApi::rsp1a());
        let info = DeviceInfo {
            driver: DRIVER_ID.to_string(),
            key: "9999999999".to_string(),
            label: "gone".to_string(),
            serial: Some("9999999999".to_string()),
            profile: None,
        };
        assert!(matches!(
            driver(api).open(&info),
            Err(DeviceError::NotFound(_))
        ));
    }

    #[test]
    fn a_second_open_of_the_same_receiver_reports_it_is_in_use() {
        let api = Arc::new(FakeApi::rsp1a());
        let _first = open(&api, "1234567890");
        let driver = driver(api.clone());
        let info = driver.probe().into_iter().next().expect("listed");
        assert!(matches!(driver.open(&info), Err(DeviceError::InUse(_))));
    }

    #[test]
    fn opening_configures_a_coherent_starting_point() {
        let api = Arc::new(FakeApi::rsp1a());
        let device = open(&api, "1234567890");
        assert_eq!(device.settings().sample_rate, Some(2_000_000.0));
        assert_eq!(api.dev_params().fs_freq.fs_hz, 2_000_000.0);
        assert_eq!(api.channel(ffi::TUNER_A).tuner_params.if_type, ffi::IF_ZERO);
        assert_eq!(device.capabilities().rx_streams, 1);
        assert_eq!(device.capabilities().tx_streams, 0);
    }

    #[test]
    fn a_closed_device_is_released_back_to_the_api() {
        let api = Arc::new(FakeApi::rsp1a());
        let device = open(&api, "1234567890");
        assert!(api.is_selected());
        drop(device);
        assert!(!api.is_selected());
    }

    #[test]
    fn settings_applied_before_streaming_reach_the_parameters_without_an_update_call() {
        let api = Arc::new(FakeApi::rsp1a());
        let mut device = open(&api, "1234567890");
        device
            .apply(&DeviceSettings {
                center_hz: Some(99_500_000.0),
                ..DeviceSettings::default()
            })
            .expect("tune");
        assert_eq!(
            api.channel(ffi::TUNER_A).tuner_params.rf_freq.rf_hz,
            99_500_000.0
        );
        assert!(api.updates().is_empty());
        assert_eq!(device.settings().center_hz, Some(99_500_000.0));
    }

    #[test]
    fn settings_applied_while_streaming_are_pushed_with_the_matching_reason() {
        let api = Arc::new(FakeApi::rsp1a());
        let mut device = open(&api, "1234567890");
        device
            .rx_start(vec![RxSink::new(|_, _| {})])
            .expect("start");
        device
            .apply(&DeviceSettings {
                center_hz: Some(145_500_000.0),
                ..DeviceSettings::default()
            })
            .expect("tune");
        let updates = api.updates();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].0, ffi::TUNER_A);
        assert_eq!(updates[0].1, ffi::UPDATE_TUNER_FRF);
        device.rx_stop();
    }

    #[test]
    fn a_setting_that_changes_nothing_is_not_pushed_to_the_hardware() {
        let api = Arc::new(FakeApi::rsp1a());
        let mut device = open(&api, "1234567890");
        device
            .rx_start(vec![RxSink::new(|_, _| {})])
            .expect("start");
        let center = device.settings().center_hz;
        device
            .apply(&DeviceSettings {
                center_hz: center,
                ..DeviceSettings::default()
            })
            .expect("tune");
        assert!(api.updates().is_empty());
        device.rx_stop();
    }

    #[test]
    fn samples_from_the_api_reach_the_sink() {
        let api = Arc::new(FakeApi::rsp1a());
        let mut device = open(&api, "1234567890");
        let (tx, rx) = mpsc::channel();
        device
            .rx_start(vec![RxSink::new(move |samples, _| {
                tx.send(samples.len()).expect("receiver lives");
            })])
            .expect("start");
        assert!(api.is_streaming());
        api.emit(ffi::TUNER_A, &[(1000, -1000); 64]);
        assert_eq!(rx.try_recv().expect("a block"), 64);
        device.rx_stop();
        assert!(!api.is_streaming());
    }

    #[test]
    fn a_second_start_is_refused_while_one_runs() {
        let api = Arc::new(FakeApi::rsp1a());
        let mut device = open(&api, "1234567890");
        device
            .rx_start(vec![RxSink::new(|_, _| {})])
            .expect("start");
        assert!(matches!(
            device.rx_start(vec![RxSink::new(|_, _| {})]),
            Err(DeviceError::AlreadyStreaming)
        ));
        device.rx_stop();
    }

    #[test]
    fn the_wrong_number_of_sinks_is_refused_before_the_hardware_is_touched() {
        let api = Arc::new(FakeApi::rsp1a());
        let mut device = open(&api, "1234567890");
        assert!(matches!(
            device.rx_start(vec![RxSink::new(|_, _| {}), RxSink::new(|_, _| {})]),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(!api.is_streaming());
    }

    #[test]
    fn an_unplugged_receiver_surfaces_through_the_fatal_handler() {
        let api = Arc::new(FakeApi::rsp1a());
        let mut device = open(&api, "1234567890");
        let (tx, rx) = mpsc::channel();
        device
            .rx_start(vec![RxSink::with_fatal_handler(
                |_, _| {},
                move |error| tx.send(error.to_string()).expect("receiver lives"),
            )])
            .expect("start");
        api.unplug();
        let reported = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the monitor reports the unplug");
        assert!(reported.contains("unplugged"));
        assert!(!api.is_streaming());
    }

    #[test]
    fn a_dual_tuner_duo_streams_both_tuners_independently() {
        let api = Arc::new(FakeApi::dual_tuner_duo());
        let mut device = open(&api, "1809001DDD@DT");
        assert_eq!(device.capabilities().rx_streams, 2);
        assert!(device.capabilities().per_stream.tuning);
        assert_eq!(api.dev_params().fs_freq.fs_hz, model::DUO_DUAL_TUNER_FS_HZ);
        assert_eq!(
            api.channel(ffi::TUNER_B).tuner_params.if_type,
            ffi::IF_1_620
        );

        device
            .apply(&DeviceSettings {
                streams: vec![
                    StreamSettings {
                        stream: 0,
                        center_hz: Some(7_100_000.0),
                        ..StreamSettings::default()
                    },
                    StreamSettings {
                        stream: 1,
                        center_hz: Some(14_200_000.0),
                        ..StreamSettings::default()
                    },
                ],
                ..DeviceSettings::default()
            })
            .expect("tune both tuners");
        assert_eq!(
            api.channel(ffi::TUNER_A).tuner_params.rf_freq.rf_hz,
            7_100_000.0
        );
        assert_eq!(
            api.channel(ffi::TUNER_B).tuner_params.rf_freq.rf_hz,
            14_200_000.0
        );

        let (tx_a, rx_a) = mpsc::channel();
        let (tx_b, rx_b) = mpsc::channel();
        device
            .rx_start(vec![
                RxSink::new(move |samples, _| tx_a.send(samples.len()).expect("receiver lives")),
                RxSink::new(move |samples, _| tx_b.send(samples.len()).expect("receiver lives")),
            ])
            .expect("start");
        api.emit(ffi::TUNER_A, &[(1, 1); 16]);
        api.emit(ffi::TUNER_B, &[(1, 1); 32]);
        assert_eq!(rx_a.try_recv().expect("tuner 1 block"), 16);
        assert_eq!(rx_b.try_recv().expect("tuner 2 block"), 32);
        device.rx_stop();
    }

    #[test]
    fn a_stream_index_the_device_does_not_have_is_refused() {
        let api = Arc::new(FakeApi::rsp1a());
        let mut device = open(&api, "1234567890");
        assert!(matches!(
            device.apply(&DeviceSettings {
                streams: vec![StreamSettings {
                    stream: 1,
                    center_hz: Some(100e6),
                    ..StreamSettings::default()
                }],
                ..DeviceSettings::default()
            }),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn a_duo_slave_waits_for_its_master_before_streaming() {
        let api = Arc::new(FakeApi::with_devices(vec![FakeApi::duo(
            "1809001DDD",
            ffi::DUO_MODE_SLAVE,
            ffi::TUNER_B,
        )]));
        api.require_master_before_start(1);
        let mut device = open(&api, "1809001DDD@SLV");
        let master = api.clone();
        let started = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            master.master_started();
        });
        device
            .rx_start(vec![RxSink::new(|_, _| {})])
            .expect("start");
        started.join().expect("the master thread finishes");
        assert!(api.is_streaming());
        device.rx_stop();
    }

    #[test]
    fn a_duo_slave_uses_the_tuner_the_master_left_free() {
        let api = Arc::new(FakeApi::with_devices(vec![FakeApi::duo(
            "1809001DDD",
            ffi::DUO_MODE_SLAVE,
            ffi::TUNER_B,
        )]));
        let mut device = open(&api, "1809001DDD@SLV");
        device
            .rx_start(vec![RxSink::new(|_, _| {})])
            .expect("start");
        device
            .apply(&DeviceSettings {
                center_hz: Some(50_000_000.0),
                ..DeviceSettings::default()
            })
            .expect("tune");
        assert_eq!(api.updates()[0].0, ffi::TUNER_B);
        assert_eq!(
            api.channel(ffi::TUNER_B).tuner_params.rf_freq.rf_hz,
            50_000_000.0
        );
        device.rx_stop();
    }

    #[test]
    fn the_rf_gain_range_follows_the_band_the_receiver_is_tuned_to() {
        let api = Arc::new(FakeApi::rsp1a());
        let mut device = open(&api, "1234567890");
        let rf_max = |device: &dyn SdrDevice| {
            device
                .capabilities()
                .gains
                .iter()
                .find(|stage| stage.name == caps::RF_GAIN_STAGE)
                .expect("rf stage")
                .range
                .max
        };
        let tune = |device: &mut Box<dyn SdrDevice>, center_hz: f64| {
            device
                .apply(&DeviceSettings {
                    center_hz: Some(center_hz),
                    ..DeviceSettings::default()
                })
                .expect("tune");
        };
        tune(&mut device, 100_000_000.0);
        assert_eq!(rf_max(device.as_ref()), 62.0);
        tune(&mut device, 5_000_000.0);
        assert_eq!(
            rf_max(device.as_ref()),
            61.0,
            "the AM band below 60 MHz has its own gain table"
        );
        tune(&mut device, 500_000_000.0);
        assert_eq!(rf_max(device.as_ref()), 64.0);
        assert!(
            device
                .settings()
                .gains
                .iter()
                .any(|gain| gain.stage == caps::IF_GAIN_STAGE)
        );
    }

    #[test]
    fn an_extra_this_receiver_does_not_have_is_refused_and_changes_nothing() {
        let api = Arc::new(FakeApi::with_devices(vec![FakeApi::device(
            ffi::RSP1_ID,
            "1000000001",
        )]));
        let mut device = open(&api, "1000000001");
        assert!(matches!(
            device.apply(&DeviceSettings {
                extra: vec![ExtraValue {
                    name: caps::EXTRA_BIAS_T.to_string(),
                    value: true.into(),
                }],
                ..DeviceSettings::default()
            }),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(api.updates().is_empty());
    }

    #[test]
    fn a_gain_setting_survives_the_round_trip_through_the_hardware_units() {
        let api = Arc::new(FakeApi::rsp1a());
        let mut device = open(&api, "1234567890");
        device
            .apply(&DeviceSettings {
                gains: vec![GainValue {
                    stage: caps::IF_GAIN_STAGE.to_string(),
                    value_db: 25.0,
                }],
                ..DeviceSettings::default()
            })
            .expect("gain");
        assert_eq!(api.channel(ffi::TUNER_A).tuner_params.gain.gr_db, 34);
        let reported = device
            .settings()
            .gains
            .iter()
            .find(|gain| gain.stage == caps::IF_GAIN_STAGE)
            .expect("if gain")
            .value_db;
        assert_eq!(reported, 25.0);
    }
}
