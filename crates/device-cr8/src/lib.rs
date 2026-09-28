use std::{
    cell::UnsafeCell,
    ffi::{c_int, c_void},
    sync::Arc,
};

use sdrmm_device::{DeviceDriver, DeviceError, RxSink, SdrDevice, check_stream_settings};
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings};

mod api;
mod caps;
mod ffi;
mod settings;

pub use api::{Cr8Api, DevHandle, library_candidates, load_error, shared};
pub use caps::{CLOCK_EXTERNAL, CLOCK_INTERNAL, CLOCK_SETTING, capabilities, profile};

pub const DRIVER_ID: &str = "cr8";

const BUFFER_SAMPLES: usize = 65_536;

pub struct Cr8Driver {
    api: Option<Arc<dyn Cr8Api>>,
}

impl Default for Cr8Driver {
    fn default() -> Self {
        Self::new()
    }
}

impl Cr8Driver {
    #[must_use]
    pub fn new() -> Self {
        Self { api: None }
    }

    #[must_use]
    pub fn with_api(api: Arc<dyn Cr8Api>) -> Self {
        Self { api: Some(api) }
    }

    fn api(&self) -> Option<Arc<dyn Cr8Api>> {
        match &self.api {
            Some(api) => Some(api.clone()),
            None => shared().map(|loaded| loaded as Arc<dyn Cr8Api>),
        }
    }
}

fn info(serial: &str) -> DeviceInfo {
    DeviceInfo {
        driver: DRIVER_ID.to_owned(),
        key: serial.to_owned(),
        label: format!("Dragon Labs CR-8 {serial}"),
        serial: Some(serial.to_owned()),
        profile: Some(caps::profile()),
    }
}

impl DeviceDriver for Cr8Driver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        let Some(api) = self.api() else {
            return Vec::new();
        };
        match api.serials() {
            Ok(serials) => serials.iter().map(|serial| info(serial)).collect(),
            Err(error) => {
                tracing::warn!(%error, "could not list CR-8 devices");
                Vec::new()
            }
        }
    }

    fn open(&self, wanted: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        let api = self
            .api()
            .ok_or_else(|| DeviceError::NotFound(wanted.id()))?;
        let handle = api.open(&wanted.key)?;
        let versions = api.versions(handle);
        tracing::info!(
            serial = %wanted.key,
            hardware = format!("{}.{}", versions.hw_ver_major, versions.hw_ver_minor),
            firmware = format!(
                "{}.{}.{}",
                versions.fw_ver_major, versions.fw_ver_minor, versions.fw_ver_build
            ),
            "opened a CR-8"
        );
        Ok(Box::new(Cr8Device::new(api, handle)))
    }
}

struct Lanes {
    sinks: UnsafeCell<Vec<RxSink>>,
}

unsafe impl Sync for Lanes {}

impl Lanes {
    const fn new(sinks: Vec<RxSink>) -> Self {
        Self {
            sinks: UnsafeCell::new(sinks),
        }
    }
}

pub struct Cr8Device {
    api: Arc<dyn Cr8Api>,
    handle: DevHandle,
    capabilities: Capabilities,
    settings: DeviceSettings,
    lanes: Option<Arc<Lanes>>,
    retained: Vec<Arc<Lanes>>,
    stop_failed: bool,
}

impl Cr8Device {
    fn new(api: Arc<dyn Cr8Api>, handle: DevHandle) -> Self {
        Self {
            api,
            handle,
            capabilities: caps::capabilities(),
            settings: DeviceSettings {
                sample_rate: Some(ffi::SAMPLE_RATE_HZ),
                ..DeviceSettings::default()
            },
            lanes: None,
            retained: Vec::new(),
            stop_failed: false,
        }
    }
}

unsafe extern "C" fn deliver(
    samples: *mut *mut ffi::Complex,
    count: usize,
    drops: usize,
    ctx: *mut c_void,
) {
    if ctx.is_null() || samples.is_null() {
        return;
    }
    let lanes = unsafe { &*ctx.cast::<Lanes>() };
    let sinks = unsafe { &mut *lanes.sinks.get() };
    for (lane, sink) in sinks.iter_mut().enumerate() {
        if drops > 0 {
            sink.dropped(drops as u64);
        }
        let channel = unsafe { *samples.add(lane) };
        if channel.is_null() || count == 0 {
            continue;
        }
        let block = unsafe {
            std::slice::from_raw_parts(channel.cast::<num_complex::Complex<f32>>(), count)
        };
        sink.push(block);
    }
}

impl SdrDevice for Cr8Device {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn in_flight_samples(&self) -> u64 {
        2 * BUFFER_SAMPLES as u64
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        if std::mem::take(&mut self.stop_failed) {
            return Err(DeviceError::Io("CR-8 did not stop".to_owned()));
        }
        check_stream_settings(settings, &self.capabilities)?;
        let plan = settings::plan(settings, &self.settings, &self.capabilities)?;
        for step in &plan {
            step.run(self.api.as_ref(), self.handle)?;
        }
        self.settings.merge_from(settings);
        self.settings.sample_rate = Some(ffi::SAMPLE_RATE_HZ);
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        if self.lanes.is_some() {
            return Err(DeviceError::AlreadyStreaming);
        }
        let expected = self.capabilities.rx_streams as usize;
        if sinks.len() != expected {
            return Err(DeviceError::Unsupported(format!(
                "this device has {expected} rx streams, got {} sinks",
                sinks.len()
            )));
        }
        self.api.enable(self.handle, ffi::CHAN_ALL)?;
        let lanes = Arc::new(Lanes::new(sinks));
        let ctx = Arc::as_ptr(&lanes).cast::<c_void>().cast_mut();
        self.lanes = Some(lanes);
        let started = self.api.start(self.handle, BUFFER_SAMPLES, deliver, ctx);
        if started.is_err() {
            self.lanes = None;
        }
        started
    }

    fn rx_stop(&mut self) {
        if let Err(error) = self.api.stop(self.handle) {
            tracing::error!(%error, "the CR-8 did not stop");
            self.retained.extend(self.lanes.take());
            self.stop_failed = true;
        }
        if let Err(error) = self.api.disable(self.handle, ffi::CHAN_ALL) {
            tracing::warn!(%error, "the CR-8 kept its channels enabled");
        }
        self.lanes = None;
    }
}

impl Drop for Cr8Device {
    fn drop(&mut self) {
        if self.lanes.is_some() {
            self.rx_stop();
        }
        self.api.close(self.handle);
        self.retained.clear();
    }
}

#[must_use]
pub fn channel_mask(lane: usize) -> c_int {
    if lane >= ffi::CHANNEL_COUNT {
        return 0;
    }
    1 << lane
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::{c_int, c_void},
        path::{Path, PathBuf},
        sync::{
            Arc, Mutex, PoisonError,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };

    use sdrmm_device::{DeviceError, RxSink};
    use sdrmm_wire::{
        AgcSetting, BandwidthSetting, DeviceSettings, ExtraValue, GainKind, GainUnit, GainValue,
        StreamSettings,
    };

    use super::*;
    use crate::settings::{Step, plan};

    #[derive(Default)]
    struct Recorder {
        serials: Vec<String>,
        calls: Mutex<Vec<String>>,
        path: PathBuf,
        refuse_stop: AtomicBool,
        ctx: AtomicUsize,
    }

    impl Recorder {
        fn note(&self, call: String) {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(call);
        }

        fn calls(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl Cr8Api for Recorder {
        fn library_path(&self) -> &Path {
            &self.path
        }

        fn serials(&self) -> Result<Vec<String>, DeviceError> {
            Ok(self.serials.clone())
        }

        fn open(&self, serial: &str) -> Result<DevHandle, DeviceError> {
            if self.serials.iter().any(|known| known == serial) {
                self.note(format!("open {serial}"));
                Ok(DevHandle(std::ptr::dangling_mut()))
            } else {
                Err(DeviceError::NotFound(format!("cr8:{serial}")))
            }
        }

        fn close(&self, _dev: DevHandle) {
            self.note("close".to_owned());
        }

        fn versions(&self, _dev: DevHandle) -> ffi::DevInfo {
            ffi::DevInfo::default()
        }

        fn start(
            &self,
            _dev: DevHandle,
            buffer: usize,
            _callback: ffi::Callback,
            ctx: *mut c_void,
        ) -> Result<(), DeviceError> {
            self.ctx.store(ctx as usize, Ordering::SeqCst);
            self.note(format!("start {buffer}"));
            Ok(())
        }

        fn stop(&self, _dev: DevHandle) -> Result<(), DeviceError> {
            self.note("stop".to_owned());
            if self.refuse_stop.load(Ordering::SeqCst) {
                return Err(DeviceError::Io("the worker thread is stuck".to_owned()));
            }
            Ok(())
        }

        fn enable(&self, _dev: DevHandle, channels: c_int) -> Result<(), DeviceError> {
            self.note(format!("enable {channels:#x}"));
            Ok(())
        }

        fn disable(&self, _dev: DevHandle, channels: c_int) -> Result<(), DeviceError> {
            self.note(format!("disable {channels:#x}"));
            Ok(())
        }

        fn set_freq(
            &self,
            _dev: DevHandle,
            channels: c_int,
            freq_hz: f64,
            coherent: bool,
        ) -> Result<(), DeviceError> {
            self.note(format!("freq {channels:#x} {freq_hz} coherent={coherent}"));
            Ok(())
        }

        fn set_lna_gain(
            &self,
            _dev: DevHandle,
            channels: c_int,
            gain: i32,
        ) -> Result<(), DeviceError> {
            self.note(format!("lna {channels:#x} {gain}"));
            Ok(())
        }

        fn set_mixer_gain(
            &self,
            _dev: DevHandle,
            channels: c_int,
            gain: i32,
        ) -> Result<(), DeviceError> {
            self.note(format!("mixer {channels:#x} {gain}"));
            Ok(())
        }

        fn set_vga_gain(
            &self,
            _dev: DevHandle,
            channels: c_int,
            gain: i32,
        ) -> Result<(), DeviceError> {
            self.note(format!("vga {channels:#x} {gain}"));
            Ok(())
        }

        fn set_clock(&self, _dev: DevHandle, clock: c_int) -> Result<(), DeviceError> {
            self.note(format!("clock {clock}"));
            Ok(())
        }
    }

    fn recorder(serials: &[&str]) -> Arc<Recorder> {
        Arc::new(Recorder {
            serials: serials.iter().map(|serial| (*serial).to_owned()).collect(),
            ..Recorder::default()
        })
    }

    #[test]
    fn a_machine_without_the_library_finds_no_radios() {
        assert!(Cr8Driver::new().probe().is_empty());
    }

    #[test]
    fn every_serial_the_library_lists_is_offered_as_one_eight_lane_radio() {
        let api = recorder(&["DL0001", "DL0002"]);
        let driver = Cr8Driver::with_api(api);
        let found = driver.probe();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].id(), "cr8:DL0001");
        let profile = found[0].profile.as_ref().expect("a profile");
        assert_eq!(profile.rx_streams, 8);
        assert_eq!(profile.sample_rates, vec![ffi::SAMPLE_RATE_HZ]);
        assert!(!profile.per_stream.tuning, "eight channels, one frequency");
    }

    #[test]
    fn the_capabilities_say_the_lanes_share_a_synthesizer() {
        let capabilities = capabilities();
        assert_eq!(capabilities.rx_streams, 8);
        assert_eq!(capabilities.coherence, sdrmm_wire::Coherence::PhaseCoherent);
        assert_eq!(capabilities.gains.len(), 3);
        assert!(capabilities.per_stream.gain, "gain is per channel");
    }

    #[test]
    fn a_radio_that_is_not_plugged_in_is_refused_by_name() {
        let driver = Cr8Driver::with_api(recorder(&["DL0001"]));
        let missing = DeviceInfo {
            driver: DRIVER_ID.to_owned(),
            key: "DL9999".to_owned(),
            label: String::new(),
            serial: None,
            profile: None,
        };
        let Err(DeviceError::NotFound(named)) = driver.open(&missing) else {
            panic!("a serial the library does not list must be refused");
        };
        assert!(named.contains("DL9999"), "{named}");
    }

    #[test]
    fn tuning_moves_every_channel_together() {
        let capabilities = capabilities();
        let steps = plan(
            &DeviceSettings {
                center_hz: Some(433.92e6),
                ..DeviceSettings::default()
            },
            &DeviceSettings::default(),
            &capabilities,
        )
        .expect("a plain retune");
        assert_eq!(
            steps,
            vec![Step::Tune {
                channels: ffi::CHAN_ALL,
                freq_hz: 433.92e6
            }]
        );
    }

    #[test]
    fn a_gain_meant_for_one_channel_reaches_only_that_channel() {
        let capabilities = capabilities();
        let steps = plan(
            &DeviceSettings {
                gains: vec![GainValue {
                    stage: "LNA".to_owned(),
                    value_db: 9.0,
                }],
                streams: vec![StreamSettings {
                    stream: 2,
                    center_hz: None,
                    tuning: None,
                    gains: vec![GainValue {
                        stage: "VGA".to_owned(),
                        value_db: 40.0,
                    }],
                    antenna: None,
                    agc: None,
                }],
                ..DeviceSettings::default()
            },
            &DeviceSettings::default(),
            &capabilities,
        )
        .expect("gains for everyone and for one");
        assert_eq!(
            steps,
            vec![
                Step::Lna {
                    channels: ffi::CHAN_ALL,
                    gain: 9
                },
                Step::Vga {
                    channels: 0b100,
                    gain: 15
                },
            ],
            "the stream gain is clamped to what the stage can reach"
        );
    }

    #[test]
    fn a_stage_the_radio_does_not_have_is_refused_by_name() {
        let capabilities = capabilities();
        let Err(DeviceError::Unsupported(message)) = plan(
            &DeviceSettings {
                gains: vec![GainValue {
                    stage: "IF".to_owned(),
                    value_db: 3.0,
                }],
                ..DeviceSettings::default()
            },
            &DeviceSettings::default(),
            &capabilities,
        ) else {
            panic!("an unknown gain stage must be refused");
        };
        assert!(message.contains("IF"), "{message}");
    }

    #[test]
    fn the_one_sample_rate_the_radio_has_is_the_only_one_accepted() {
        let capabilities = capabilities();
        assert!(
            plan(
                &DeviceSettings {
                    sample_rate: Some(ffi::SAMPLE_RATE_HZ),
                    ..DeviceSettings::default()
                },
                &DeviceSettings::default(),
                &capabilities,
            )
            .is_ok()
        );
        let Err(DeviceError::Unsupported(message)) = plan(
            &DeviceSettings {
                sample_rate: Some(2.4e6),
                ..DeviceSettings::default()
            },
            &DeviceSettings::default(),
            &capabilities,
        ) else {
            panic!("the CR-8 has one rate and settings that ask for another must say so");
        };
        assert!(message.contains("12.5"), "{message}");
    }

    #[test]
    fn the_clock_source_is_chosen_before_anything_is_tuned_to_it() {
        let capabilities = capabilities();
        let steps = plan(
            &DeviceSettings {
                center_hz: Some(100e6),
                extra: vec![ExtraValue {
                    name: CLOCK_SETTING.to_owned(),
                    value: serde_json::Value::String(CLOCK_EXTERNAL.to_owned()),
                }],
                ..DeviceSettings::default()
            },
            &DeviceSettings::default(),
            &capabilities,
        )
        .expect("an external reference and a frequency");
        assert_eq!(steps[0], Step::Clock(ffi::CLOCK_EXTERNAL));
        assert!(matches!(steps[1], Step::Tune { .. }));
    }

    #[test]
    fn a_stream_the_radio_does_not_have_is_refused_by_number() {
        let capabilities = capabilities();
        let Err(DeviceError::Unsupported(message)) = plan(
            &DeviceSettings {
                streams: vec![StreamSettings {
                    stream: 9,
                    center_hz: None,
                    tuning: None,
                    gains: vec![GainValue {
                        stage: "LNA".to_owned(),
                        value_db: 1.0,
                    }],
                    antenna: None,
                    agc: None,
                }],
                ..DeviceSettings::default()
            },
            &DeviceSettings::default(),
            &capabilities,
        ) else {
            panic!("stream nine on an eight-channel radio must be refused");
        };
        assert!(message.contains('9'), "{message}");
    }

    #[test]
    fn starting_enables_every_channel_and_stopping_gives_them_back() {
        let api = recorder(&["DL0001"]);
        let mut device = Cr8Device::new(api.clone(), DevHandle(std::ptr::dangling_mut()));
        let sinks = (0..8).map(|_| RxSink::new(|_, _| {})).collect();
        device.rx_start(sinks).expect("starts");
        device.rx_stop();
        assert_eq!(
            api.calls(),
            vec![
                "enable 0xff".to_owned(),
                format!("start {BUFFER_SAMPLES}"),
                "stop".to_owned(),
                "disable 0xff".to_owned(),
            ]
        );
    }

    #[test]
    fn the_wrong_number_of_sinks_is_refused_by_count() {
        let mut device = Cr8Device::new(recorder(&["DL0001"]), DevHandle(std::ptr::dangling_mut()));
        let Err(DeviceError::Unsupported(message)) = device.rx_start(vec![RxSink::new(|_, _| {})])
        else {
            panic!("one sink for an eight-lane radio must be refused");
        };
        assert!(message.contains('8'), "{message}");
    }

    #[test]
    fn a_buffer_the_library_could_not_deliver_shows_up_as_a_gap_on_every_lane() {
        let seen: Arc<Mutex<Vec<(usize, u64, usize)>>> = Arc::new(Mutex::new(Vec::new()));
        let sinks: Vec<RxSink> = (0..8)
            .map(|lane| {
                let seen = seen.clone();
                RxSink::new(move |block: &[num_complex::Complex<f32>], index| {
                    seen.lock().unwrap_or_else(PoisonError::into_inner).push((
                        lane,
                        index,
                        block.len(),
                    ));
                })
            })
            .collect();
        let lanes = Arc::new(Lanes::new(sinks));
        let ctx = Arc::as_ptr(&lanes).cast::<c_void>().cast_mut();

        let mut buffers: Vec<Vec<ffi::Complex>> =
            (0..8).map(|_| vec![ffi::Complex::default(); 4]).collect();
        let mut pointers: Vec<*mut ffi::Complex> = buffers
            .iter_mut()
            .map(|buffer| buffer.as_mut_ptr())
            .collect();
        unsafe { deliver(pointers.as_mut_ptr(), 4, 0, ctx) };
        unsafe { deliver(pointers.as_mut_ptr(), 4, 100, ctx) };

        let seen = seen.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let lane_three: Vec<(u64, usize)> = seen
            .iter()
            .filter(|(lane, _, _)| *lane == 3)
            .map(|(_, index, count)| (*index, *count))
            .collect();
        assert_eq!(
            lane_three,
            vec![(0, 4), (104, 4)],
            "the hundred samples the radio lost are stepped over, not silently closed up"
        );
        assert_eq!(seen.len(), 16, "every lane hears about every buffer");
    }

    struct Buffers {
        _data: Vec<Vec<ffi::Complex>>,
        pointers: Vec<*mut ffi::Complex>,
    }

    fn buffers(count: usize) -> Buffers {
        let mut data: Vec<Vec<ffi::Complex>> = (0..8)
            .map(|_| vec![ffi::Complex::default(); count])
            .collect();
        let pointers = data.iter_mut().map(|buffer| buffer.as_mut_ptr()).collect();
        Buffers {
            _data: data,
            pointers,
        }
    }

    struct Noted(Arc<Recorder>, usize);

    impl Drop for Noted {
        fn drop(&mut self) {
            self.0.note(format!("sink {} dropped", self.1));
        }
    }

    fn counting_sinks(api: &Arc<Recorder>) -> (Vec<RxSink>, Arc<Vec<AtomicUsize>>) {
        let counts: Arc<Vec<AtomicUsize>> = Arc::new((0..8).map(|_| AtomicUsize::new(0)).collect());
        let sinks = (0..8)
            .map(|lane| {
                let counts = counts.clone();
                let noted = Noted(api.clone(), lane);
                RxSink::new(move |block: &[num_complex::Complex<f32>], _| {
                    let _ = &noted;
                    counts[lane].fetch_add(block.len(), Ordering::SeqCst);
                })
            })
            .collect();
        (sinks, counts)
    }

    #[test]
    fn a_failed_stop_keeps_the_callback_context_alive() {
        let api = recorder(&["DL0001"]);
        api.refuse_stop.store(true, Ordering::SeqCst);
        let mut device = Cr8Device::new(api.clone(), DevHandle(std::ptr::dangling_mut()));
        let (sinks, counts) = counting_sinks(&api);
        device.rx_start(sinks).expect("starts");
        device.rx_stop();
        assert!(device.lanes.is_none());
        assert_eq!(
            device.retained.len(),
            1,
            "the vendor thread may still call back"
        );
        let ctx = api.ctx.load(Ordering::SeqCst) as *mut c_void;
        let mut block = buffers(16);
        unsafe { deliver(block.pointers.as_mut_ptr(), 16, 0, ctx) };
        assert!(
            counts
                .iter()
                .all(|count| count.load(Ordering::SeqCst) == 16)
        );
        let Err(DeviceError::Io(message)) = device.apply(&DeviceSettings::default()) else {
            panic!("the failed stop must surface");
        };
        assert_eq!(message, "CR-8 did not stop");
        device
            .apply(&DeviceSettings::default())
            .expect("reported once");
        drop(device);
        let calls = api.calls();
        let closed = calls
            .iter()
            .position(|call| call == "close")
            .expect("closed");
        let released = calls
            .iter()
            .position(|call| call.starts_with("sink"))
            .expect("sinks released");
        assert!(closed < released, "{calls:?}");
    }

    #[test]
    fn a_second_start_never_frees_the_running_context() {
        let api = recorder(&["DL0001"]);
        let mut device = Cr8Device::new(api.clone(), DevHandle(std::ptr::dangling_mut()));
        let (sinks, counts) = counting_sinks(&api);
        device.rx_start(sinks).expect("starts");
        let (again, _) = counting_sinks(&api);
        assert!(matches!(
            device.rx_start(again),
            Err(DeviceError::AlreadyStreaming)
        ));
        let ctx = api.ctx.load(Ordering::SeqCst) as *mut c_void;
        let mut block = buffers(8);
        unsafe { deliver(block.pointers.as_mut_ptr(), 8, 0, ctx) };
        assert!(counts.iter().all(|count| count.load(Ordering::SeqCst) == 8));
        device.rx_stop();
    }

    #[test]
    fn a_clean_stop_releases_the_callback_context() {
        let api = recorder(&["DL0001"]);
        let mut device = Cr8Device::new(api.clone(), DevHandle(std::ptr::dangling_mut()));
        let (sinks, _) = counting_sinks(&api);
        device.rx_start(sinks).expect("starts");
        device.rx_stop();
        assert!(device.retained.is_empty());
        assert_eq!(
            api.calls()
                .iter()
                .filter(|call| call.starts_with("sink"))
                .count(),
            8
        );
        device
            .apply(&DeviceSettings::default())
            .expect("nothing to report");
    }

    #[test]
    fn deliver_reaches_every_lane_without_a_lock() {
        let api = recorder(&[]);
        let (sinks, counts) = counting_sinks(&api);
        let lanes = Arc::new(Lanes::new(sinks));
        let ctx = Arc::as_ptr(&lanes) as usize;
        let delivering = std::thread::spawn(move || {
            let mut block = buffers(32);
            for _ in 0..100 {
                unsafe { deliver(block.pointers.as_mut_ptr(), 32, 0, ctx as *mut c_void) };
            }
        });
        delivering.join().expect("the vendor thread");
        assert!(
            counts
                .iter()
                .all(|count| count.load(Ordering::SeqCst) == 3_200)
        );
        drop(lanes);
    }

    #[test]
    fn a_radio_holds_two_buffers_in_flight() {
        let device = Cr8Device::new(recorder(&["DL0001"]), DevHandle(std::ptr::dangling_mut()));
        assert_eq!(device.in_flight_samples(), 2 * BUFFER_SAMPLES as u64);
    }

    #[test]
    fn a_sample_is_laid_out_the_way_the_engine_reads_it() {
        assert_eq!(
            std::mem::size_of::<ffi::Complex>(),
            std::mem::size_of::<num_complex::Complex<f32>>()
        );
        assert_eq!(
            std::mem::align_of::<ffi::Complex>(),
            std::mem::align_of::<num_complex::Complex<f32>>()
        );
    }

    #[test]
    fn every_stage_is_a_firmware_step_index_named_by_its_kind() {
        let capabilities = capabilities();
        assert_eq!(
            capabilities
                .gains
                .iter()
                .map(|stage| (stage.name.as_str(), stage.kind, stage.unit))
                .collect::<Vec<_>>(),
            vec![
                ("LNA", GainKind::Lna, GainUnit::Index),
                ("MIX", GainKind::Mixer, GainUnit::Index),
                ("VGA", GainKind::Vga, GainUnit::Index),
            ]
        );
        assert_eq!(
            capabilities
                .extra
                .iter()
                .map(|setting| (setting.name(), setting.label()))
                .collect::<Vec<_>>(),
            vec![(CLOCK_SETTING, Some("Clock source"))]
        );
    }

    #[test]
    fn the_mixer_is_driven_by_its_canonical_name_only() {
        let capabilities = capabilities();
        let steps = plan(
            &DeviceSettings {
                gains: vec![GainValue::new(GainKind::Mixer, 7.0)],
                ..DeviceSettings::default()
            },
            &DeviceSettings::default(),
            &capabilities,
        )
        .expect("a mixer gain");
        assert_eq!(
            steps,
            vec![Step::Mixer {
                channels: ffi::CHAN_ALL,
                gain: 7
            }]
        );
        for spelling in ["Mixer", "mix", "lna"] {
            let refused = plan(
                &DeviceSettings {
                    gains: vec![GainValue {
                        stage: spelling.to_owned(),
                        value_db: 1.0,
                    }],
                    ..DeviceSettings::default()
                },
                &DeviceSettings::default(),
                &capabilities,
            );
            assert!(
                matches!(refused, Err(DeviceError::Unsupported(_))),
                "{spelling} is not a stage name"
            );
        }
    }

    #[test]
    fn what_the_radio_lacks_is_refused_by_name() {
        let capabilities = capabilities();
        for (delta, what) in [
            (
                DeviceSettings {
                    bandwidth: Some(BandwidthSetting::Auto),
                    ..DeviceSettings::default()
                },
                "filter",
            ),
            (
                DeviceSettings {
                    agc: Some(AgcSetting::switched(true)),
                    ..DeviceSettings::default()
                },
                "automatic gain",
            ),
            (
                DeviceSettings {
                    bias_tee: Some(true),
                    ..DeviceSettings::default()
                },
                "bias tee",
            ),
        ] {
            let Err(DeviceError::Unsupported(message)) =
                plan(&delta, &DeviceSettings::default(), &capabilities)
            else {
                panic!("{what} must be refused");
            };
            assert!(message.contains(what), "{message}");
        }
    }

    #[test]
    fn channels_are_named_by_the_bit_the_library_expects() {
        assert_eq!(channel_mask(0), 0b1);
        assert_eq!(channel_mask(7), 0b1000_0000);
        assert_eq!(channel_mask(8), 0, "there is no ninth channel");
    }
}
