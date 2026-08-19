use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use num_complex::Complex;
use sdrmm_device::{DeviceDriver, DeviceRegistry, RxSink, SdrDevice, single_rx_sink};
use sdrmm_wire::{
    AdsbParams, AudioProcessing, ChannelParams, ChannelSettings, DcArtifact, DecoderEvent, Duplex,
    MAX_TIME_MACHINE_SECONDS, NfmParams, ScanState, Sideband, SsbParams, StreamScope,
    TimeMachineAction, TimeMachineNode, TimeMachineStatus,
};

use super::*;
use crate::planning::artifact_clears_channels;

mod channel_capture;
mod channels;
mod device_patch;
mod discovery;
mod front_end;
#[cfg(all(feature = "rtlsdr", feature = "hackrf", feature = "soapy"))]
mod hardware;
mod hotplug;
mod recording;
mod scanning;
mod time_machine;

fn mock_info(key: &str, serial: Option<&str>) -> DeviceInfo {
    DeviceInfo {
        driver: "mock".to_string(),
        key: key.to_string(),
        label: format!("Mock {key}"),
        serial: serial.map(str::to_string),
        profile: None,
    }
}

fn mock_settings() -> DeviceSettings {
    DeviceSettings {
        sample_rate: Some(2_048_000.0),
        ..DeviceSettings::default()
    }
}

fn ring_samples(sample_rate: f64) -> usize {
    crate::runtime::ring_capacity(sample_rate)
}

fn mock_ring() -> usize {
    ring_samples(mock_settings().sample_rate.unwrap_or_default())
}

fn empty_capabilities() -> Capabilities {
    Capabilities {
        freq_ranges: Vec::new(),
        sample_rates: Vec::new(),
        sample_rate_ranges: Vec::new(),
        gains: Vec::new(),
        antennas: Vec::new(),
        bandwidths: Vec::new(),
        bandwidth_ranges: Vec::new(),
        extra: Vec::new(),
        ppm: false,
        duplex: Duplex::RxOnly,
        rx_streams: 1,
        tx_streams: 0,
        per_stream: StreamScope::default(),
        directional: None,
        dc_artifact: DcArtifact::Operator,
        hardware_sweep: false,
        coherence: sdrmm_wire::Coherence::None,
    }
}

struct DyingDriver;

impl DeviceDriver for DyingDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![mock_info("dying", Some("MOCK-1"))]
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(DyingDevice {
            capabilities: empty_capabilities(),
            settings: mock_settings(),
            worker: None,
        }))
    }
}

struct DyingDevice {
    capabilities: Capabilities,
    settings: DeviceSettings,
    worker: Option<JoinHandle<()>>,
}

impl SdrDevice for DyingDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, _settings: &DeviceSettings) -> Result<(), DeviceError> {
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let mut sink = single_rx_sink(sinks)?;
        self.worker = Some(std::thread::spawn(move || {
            let block = [Complex::new(0.0f32, 0.0); 256];
            for _ in 0..3 {
                sink.push(&block);
            }
            sink.fail(DeviceError::Io("mock stream died".to_string()));
        }));
        Ok(())
    }

    fn rx_stop(&mut self) {
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

struct InstantFailDriver;

impl DeviceDriver for InstantFailDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![mock_info("instafail", None)]
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(InstantFailDevice {
            capabilities: empty_capabilities(),
            settings: mock_settings(),
        }))
    }
}

struct InstantFailDevice {
    capabilities: Capabilities,
    settings: DeviceSettings,
}

impl SdrDevice for InstantFailDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, _settings: &DeviceSettings) -> Result<(), DeviceError> {
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let mut sink = single_rx_sink(sinks)?;
        sink.fail(DeviceError::Io("died at start".to_string()));
        Ok(())
    }

    fn rx_stop(&mut self) {}
}

struct VanishingDriver {
    present: Arc<AtomicBool>,
}

impl DeviceDriver for VanishingDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        if self.present.load(Ordering::SeqCst) {
            vec![mock_info("vanish", None)]
        } else {
            Vec::new()
        }
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(SilentDevice {
            capabilities: empty_capabilities(),
            settings: mock_settings(),
        }))
    }
}

struct CountingDriver {
    probes: Arc<AtomicUsize>,
}

impl DeviceDriver for CountingDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        self.probes.fetch_add(1, Ordering::SeqCst);
        vec![mock_info("counted", None)]
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(SilentDevice {
            capabilities: empty_capabilities(),
            settings: mock_settings(),
        }))
    }
}

struct RatelessDriver;

impl DeviceDriver for RatelessDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![mock_info("rateless", None)]
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(SilentDevice {
            capabilities: empty_capabilities(),
            settings: DeviceSettings::default(),
        }))
    }
}

struct SilentDevice {
    capabilities: Capabilities,
    settings: DeviceSettings,
}

impl SdrDevice for SilentDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, _settings: &DeviceSettings) -> Result<(), DeviceError> {
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        single_rx_sink(sinks).map(|_| ())
    }

    fn rx_stop(&mut self) {}
}

struct FloodingDriver;

impl DeviceDriver for FloodingDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![mock_info("flood", None)]
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(FloodingDevice {
            capabilities: empty_capabilities(),
            settings: mock_settings(),
        }))
    }
}

struct FloodingDevice {
    capabilities: Capabilities,
    settings: DeviceSettings,
}

impl SdrDevice for FloodingDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, _settings: &DeviceSettings) -> Result<(), DeviceError> {
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let mut sink = single_rx_sink(sinks)?;
        let block = vec![Complex::new(0.0f32, 0.0); mock_ring() * 2];
        sink.push(&block);
        Ok(())
    }

    fn rx_stop(&mut self) {}
}

struct FaultOnDemandDriver {
    die: Arc<AtomicBool>,
}

impl DeviceDriver for FaultOnDemandDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![mock_info("ondemand", None)]
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(FaultOnDemandDevice {
            capabilities: empty_capabilities(),
            settings: mock_settings(),
            die: self.die.clone(),
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
        }))
    }
}

struct FaultOnDemandDevice {
    capabilities: Capabilities,
    settings: DeviceSettings,
    die: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl SdrDevice for FaultOnDemandDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, _settings: &DeviceSettings) -> Result<(), DeviceError> {
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let mut sink = single_rx_sink(sinks)?;
        let die = self.die.clone();
        let stop = self.stop.clone();
        self.worker = Some(std::thread::spawn(move || {
            let block = [Complex::new(0.1f32, 0.0); 2_048];
            while !stop.load(Ordering::SeqCst) {
                if die.load(Ordering::SeqCst) {
                    sink.fail(DeviceError::Io("mock stream died".to_string()));
                    return;
                }
                sink.push(&block);
                std::thread::sleep(Duration::from_millis(2));
            }
        }));
        Ok(())
    }

    fn rx_stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

struct BlockingApplyDriver {
    entered_tx: mpsc::Sender<()>,
    release_rx: Mutex<Option<mpsc::Receiver<()>>>,
}

impl DeviceDriver for BlockingApplyDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![mock_info("blocking", None)]
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(BlockingApplyDevice {
            capabilities: empty_capabilities(),
            settings: mock_settings(),
            entered_tx: self.entered_tx.clone(),
            release_rx: self.release_rx.lock().unwrap().take(),
        }))
    }
}

struct BlockingApplyDevice {
    capabilities: Capabilities,
    settings: DeviceSettings,
    entered_tx: mpsc::Sender<()>,
    release_rx: Option<mpsc::Receiver<()>>,
}

impl SdrDevice for BlockingApplyDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        if settings.sample_rate.is_some() {
            let _ = self.entered_tx.send(());
            if let Some(rx) = &self.release_rx {
                let _ = rx.recv();
            }
        }
        self.settings.merge_from(settings);
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        single_rx_sink(sinks).map(|_| ())
    }

    fn rx_stop(&mut self) {}
}

/// A radio that advertises a firmware sweep it will not actually start, which is the case that
/// used to leave the device set holding a runtime with no radio in it.
struct RefusedSweepDriver;

impl DeviceDriver for RefusedSweepDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![mock_info("refuses-sweep", Some("MOCK-NOSWEEP"))]
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(RefusedSweepDevice(SignalDevice {
            capabilities: Capabilities {
                freq_ranges: vec![sdrmm_wire::Range {
                    min: 80_000_000.0,
                    max: 120_000_000.0,
                    step: None,
                }],
                sample_rates: vec![SIGNAL_RATE_HZ],
                hardware_sweep: true,
                coherence: sdrmm_wire::Coherence::None,
                ..empty_capabilities()
            },
            settings: DeviceSettings {
                center_hz: Some(100_000_000.0),
                sample_rate: Some(SIGNAL_RATE_HZ),
                ..DeviceSettings::default()
            },
            center: Arc::new(Mutex::new(100_000_000.0)),
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
        })))
    }
}

struct RefusedSweepDevice(SignalDevice);

impl SdrDevice for RefusedSweepDevice {
    fn capabilities(&self) -> &Capabilities {
        self.0.capabilities()
    }

    fn settings(&self) -> &DeviceSettings {
        self.0.settings()
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        self.0.apply(settings)
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        self.0.rx_start(sinks)
    }

    fn rx_stop(&mut self) {
        self.0.rx_stop();
    }

    fn sweep_start(
        &mut self,
        _plan: &sdrmm_device::SweepPlan,
        _sink: sdrmm_device::SweepSink,
    ) -> Result<(), DeviceError> {
        Err(DeviceError::Io(
            "the firmware refused the sweep".to_string(),
        ))
    }
}

const SIGNAL_HZ: f64 = 100_100_000.0;
const SIGNAL_RATE_HZ: f64 = 240_000.0;

struct SignalDriver;

impl DeviceDriver for SignalDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![
            mock_info("signal", Some("MOCK-SIG")),
            mock_info("signal2", Some("MOCK-SIG-2")),
        ]
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(SignalDevice {
            capabilities: Capabilities {
                freq_ranges: vec![sdrmm_wire::Range {
                    min: 80_000_000.0,
                    max: 120_000_000.0,
                    step: None,
                }],
                sample_rates: vec![SIGNAL_RATE_HZ],
                ..empty_capabilities()
            },
            settings: DeviceSettings {
                center_hz: Some(100_000_000.0),
                sample_rate: Some(SIGNAL_RATE_HZ),
                ..DeviceSettings::default()
            },
            center: Arc::new(Mutex::new(100_000_000.0)),
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
        }))
    }
}

struct SignalDevice {
    capabilities: Capabilities,
    settings: DeviceSettings,
    center: Arc<Mutex<f64>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl SdrDevice for SignalDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        if let Some(center) = settings.center_hz {
            *self
                .center
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = center;
        }
        self.settings.merge_from(settings);
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let mut sink = single_rx_sink(sinks)?;
        let center = self.center.clone();
        let stop = self.stop.clone();
        stop.store(false, Ordering::SeqCst);
        self.worker = Some(std::thread::spawn(move || {
            let mut phase = 0.0f64;
            let mut block = vec![Complex::new(0.0f32, 0.0); 2_048];
            while !stop.load(Ordering::SeqCst) {
                let center = *center
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let offset = SIGNAL_HZ - center;
                if offset.abs() >= SIGNAL_RATE_HZ / 2.0 {
                    block.fill(Complex::new(0.0, 0.0));
                } else {
                    let step = std::f64::consts::TAU * offset / SIGNAL_RATE_HZ;
                    for slot in &mut block {
                        phase = (phase + step).rem_euclid(std::f64::consts::TAU);
                        *slot = Complex::new(0.5 * phase.cos() as f32, 0.5 * phase.sin() as f32);
                    }
                }
                sink.push(&block);
                std::thread::sleep(Duration::from_millis(2));
            }
        }));
        Ok(())
    }

    fn rx_stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

struct AdsbTestDriver;

impl DeviceDriver for AdsbTestDriver {
    fn id(&self) -> &'static str {
        "test-adsb"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![DeviceInfo {
            driver: self.id().to_owned(),
            key: "surface".to_owned(),
            label: "Synthetic ADS-B".to_owned(),
            serial: None,
            profile: None,
        }]
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(AdsbTestDevice {
            capabilities: Capabilities {
                sample_rates: vec![2_000_000.0, 2_400_000.0],
                ..empty_capabilities()
            },
            settings: DeviceSettings {
                center_hz: Some(1_090_000_000.0),
                sample_rate: Some(2_000_000.0),
                ..DeviceSettings::default()
            },
            rate: Arc::new(Mutex::new(2_000_000.0)),
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
        }))
    }
}

struct AdsbTestDevice {
    capabilities: Capabilities,
    settings: DeviceSettings,
    rate: Arc<Mutex<f64>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl SdrDevice for AdsbTestDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        if let Some(rate) = settings.sample_rate {
            *self
                .rate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = rate;
        }
        self.settings.merge_from(settings);
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let mut sink = single_rx_sink(sinks)?;
        let frame = sdrmm_channels::testgen::adsb::squitter(
            0x3C_6444,
            sdrmm_channels::testgen::adsb::me_airborne_position(9_000, 52.52, 13.405, false),
        );
        let frames = std::iter::repeat_n(frame, 16).collect::<Vec<_>>();
        let rate = self.rate.clone();
        let stop = self.stop.clone();
        stop.store(false, Ordering::SeqCst);
        self.worker = Some(std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let sample_rate = *rate
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let samples =
                    sdrmm_channels::testgen::adsb::transmission(&frames, 30.0, 0.8, sample_rate);
                sink.push(&samples);
                std::thread::sleep(Duration::from_millis(2));
            }
        }));
        Ok(())
    }

    fn rx_stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct SnappingDriver;

const SNAPPED_RATE: f64 = 2_400_000.0;

impl DeviceDriver for SnappingDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![mock_info("snapping", None)]
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(SnappingDevice {
            capabilities: empty_capabilities(),
            settings: mock_settings(),
        }))
    }
}

struct SnappingDevice {
    capabilities: Capabilities,
    settings: DeviceSettings,
}

impl SdrDevice for SnappingDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        let mut snapped = settings.clone();
        if let Some(center) = snapped.center_hz {
            snapped.center_hz = Some((center / 1_000_000.0).round() * 1_000_000.0);
        }
        for gain in &mut snapped.gains {
            gain.value_db = (gain.value_db / 8.0).round() * 8.0;
        }
        if snapped.sample_rate.is_some() {
            snapped.sample_rate = Some(SNAPPED_RATE);
        }
        self.settings.merge_from(&snapped);
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        single_rx_sink(sinks).map(|_| ())
    }

    fn rx_stop(&mut self) {}
}

struct ExclusiveDriver {
    claimed: Arc<AtomicBool>,
    die: Arc<AtomicBool>,
}

impl DeviceDriver for ExclusiveDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![mock_info("exclusive", Some("MOCK-X"))]
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        if self.claimed.swap(true, Ordering::SeqCst) {
            return Err(DeviceError::Io("device is busy".to_string()));
        }
        Ok(Box::new(ExclusiveDevice {
            capabilities: empty_capabilities(),
            settings: mock_settings(),
            claimed: self.claimed.clone(),
            die: self.die.clone(),
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
        }))
    }
}

struct ExclusiveDevice {
    capabilities: Capabilities,
    settings: DeviceSettings,
    claimed: Arc<AtomicBool>,
    die: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for ExclusiveDevice {
    fn drop(&mut self) {
        self.claimed.store(false, Ordering::SeqCst);
    }
}

impl SdrDevice for ExclusiveDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        self.settings.merge_from(settings);
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let mut sink = single_rx_sink(sinks)?;
        let die = self.die.clone();
        let stop = self.stop.clone();
        self.worker = Some(std::thread::spawn(move || {
            let block = [Complex::new(0.1f32, 0.0); 2_048];
            while !stop.load(Ordering::SeqCst) {
                if die.load(Ordering::SeqCst) {
                    sink.fail(DeviceError::Io("mock stream died".to_string()));
                    return;
                }
                sink.push(&block);
                std::thread::sleep(Duration::from_millis(2));
            }
        }));
        Ok(())
    }

    fn rx_stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

struct UnopenableDriver {
    opens: AtomicUsize,
}

impl DeviceDriver for UnopenableDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![mock_info("refuse", None)]
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        if self.opens.fetch_add(1, Ordering::SeqCst) == 0 {
            return Ok(Box::new(SilentDevice {
                capabilities: empty_capabilities(),
                settings: mock_settings(),
            }));
        }
        Err(DeviceError::Io(
            "still claimed by another process".to_string(),
        ))
    }
}

struct BusyDriver;

impl DeviceDriver for BusyDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![mock_info("busy", None)]
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Err(DeviceError::InUse(
            "usb_claim_interface error -6".to_string(),
        ))
    }
}

struct FlappingDriver {
    probes: AtomicUsize,
}

impl DeviceDriver for FlappingDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        let n = self.probes.fetch_add(1, Ordering::SeqCst);
        let mut out = vec![mock_info("a", None)];
        if n >= 1 {
            out.push(mock_info("b", None));
        }
        out
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Err(DeviceError::NotFound(info.id()))
    }
}

async fn wait_for_deviceset_event(events: &mut broadcast::Receiver<ServerEvent>, ds: u32) {
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("event within timeout")
            .expect("event");
        if matches!(
            ev,
            ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(id)
            } if id == ds
        ) {
            return;
        }
    }
}

fn virtual_engine() -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(VIRTUAL_PRIORITY, Box::new(VirtualDriver::new()));
    Engine::with_registry(registry, None)
}

/// The marker is an FM carrier, so its peak bin wanders across its own occupied width.
const MARKER_SPREAD_HZ: f64 =
    sdrmm_device_virtual::NFM_DEVIATION_HZ + sdrmm_device_virtual::MOD_TONE_HZ;

fn peak_hz(snap: &runtime::SpectrumSnapshot) -> f64 {
    let n = snap.db.len();
    let peak = snap
        .db
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i)
        .unwrap_or(0);
    snap.center_hz + (peak as f64 - n as f64 / 2.0) * f64::from(snap.span_hz) / n as f64
}

async fn snapshot_once(
    rx: &mut tokio::sync::broadcast::Receiver<runtime::SpectrumSnapshot>,
    accept: impl Fn(&runtime::SpectrumSnapshot) -> bool,
) -> runtime::SpectrumSnapshot {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match rx.recv().await {
                Ok(snap) if accept(&snap) => return snap,
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(e) => panic!("spectrum closed: {e}"),
            }
        }
    })
    .await
    .expect("a matching snapshot within the timeout")
}

fn tuner_caps() -> Capabilities {
    Capabilities {
        freq_ranges: vec![sdrmm_wire::Range {
            min: 24e6,
            max: 1_766e6,
            step: None,
        }],
        ..empty_capabilities()
    }
}

fn offset_settings(lo_offset_hz: f64) -> DeviceSettings {
    DeviceSettings {
        center_hz: Some(100e6),
        sample_rate: Some(2_400_000.0),
        lo_offset_hz: Some(lo_offset_hz),
        ..DeviceSettings::default()
    }
}

fn parked(id: u32, offset_hz: f64) -> ChannelInfo {
    ChannelInfo {
        id,
        stream: 0,
        settings: nfm_settings(offset_hz),
        audio_recording: None,
        baseband_recording: None,
        network_export: None,
    }
}

fn managed_caps() -> Capabilities {
    Capabilities {
        dc_artifact: DcArtifact::Managed,
        hardware_sweep: false,
        coherence: sdrmm_wire::Coherence::None,
        ..tuner_caps()
    }
}

fn untouched_settings() -> DeviceSettings {
    DeviceSettings {
        center_hz: Some(100e6),
        sample_rate: Some(2_400_000.0),
        ..DeviceSettings::default()
    }
}

fn nfm_settings(offset_hz: f64) -> ChannelSettings {
    ChannelSettings {
        offset_hz,
        squelch_db: None,
        squelch_auto_db: None,
        params: ChannelParams::Nfm(NfmParams::default()),
        audio: Default::default(),
    }
}

fn recording_engine(dir: &Path) -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(
        VIRTUAL_PRIORITY,
        Box::new(VirtualDriver::with_recordings(dir.to_path_buf())),
    );
    Engine::with_registry(registry, Some(dir.to_path_buf()))
}

async fn wait_for_recorded_samples(engine: &Engine, ds: u32, min: u64) -> RecordingStatus {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snap = engine.snapshot();
        let recording = snap
            .device_sets
            .iter()
            .find(|s| s.id == ds)
            .expect("set listed")
            .recording
            .clone();
        if let Some(rec) = recording
            && rec.samples >= min
        {
            return rec;
        }
        assert!(
            Instant::now() < deadline,
            "recording never reached {min} samples"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_baseband_samples(engine: &Engine, ds: u32, ch: u32, min: u64) -> RecordingStatus {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let recording = engine
            .snapshot()
            .device_sets
            .iter()
            .find(|set| set.id == ds)
            .and_then(|set| set.channels.iter().find(|channel| channel.id == ch))
            .and_then(|channel| channel.baseband_recording.clone());
        if let Some(recording) = recording
            && recording.samples >= min
        {
            return recording;
        }
        assert!(
            Instant::now() < deadline,
            "the channel's baseband never reached {min} samples"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_history(engine: &Engine, ds: u32, min: u64) -> TimeMachineStatus {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let history = engine
            .snapshot()
            .device_sets
            .iter()
            .find(|set| set.id == ds)
            .and_then(|set| set.time_machine.clone());
        if let Some(history) = history
            && history.held_samples >= min
        {
            return history;
        }
        assert!(
            Instant::now() < deadline,
            "the time machine never held {min} samples"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
