use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use num_complex::Complex;
use sdrmm_device::{DeviceDriver, DeviceRegistry, RxSink, SdrDevice, single_rx_sink};
use sdrmm_wire::{
    AdsbParams, AudioProcessing, ChannelSettings, DcArtifact, DecoderEvent, Duplex,
    MAX_TIME_MACHINE_SECONDS, NfmParams, ScanState, Sideband, SsbParams, StreamScope,
    TimeMachineAction, TimeMachineNode, TimeMachineStatus,
};

use super::*;

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

const SIGNAL_HZ: f64 = 100_100_000.0;
const SIGNAL_RATE_HZ: f64 = 240_000.0;

struct SignalDriver;

impl DeviceDriver for SignalDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![mock_info("signal", Some("MOCK-SIG"))]
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

#[tokio::test]
async fn device_fault_surfaces_and_removal_completes() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(DyingDriver));
    let engine = Engine::with_registry(registry, None);
    let mut events = engine.subscribe_events();
    let ds = engine.create_device_set("mock:dying").unwrap();

    wait_for_deviceset_event(&mut events, ds).await;

    let snap = engine.snapshot();
    assert_eq!(snap.device_sets[0].status, DeviceSetStatus::Error);
    assert!(
        snap.device_sets[0]
            .error
            .as_deref()
            .unwrap()
            .contains("mock stream died"),
        "fault message must surface: {:?}",
        snap.device_sets[0].error
    );
    assert_eq!(snap.device_sets[0].device.label, "Mock dying");
    assert_eq!(snap.device_sets[0].device.serial.as_deref(), Some("MOCK-1"));
    assert!(snap.device_sets[0].device.profile.is_none());

    let removal = {
        let engine = engine.clone();
        tokio::task::spawn_blocking(move || engine.remove_device_set(ds))
    };
    tokio::time::timeout(Duration::from_secs(5), removal)
        .await
        .expect("removal must not hang on a dead capture thread")
        .expect("join")
        .expect("remove ok");
}

#[tokio::test]
async fn fault_raised_before_insert_still_surfaces() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(InstantFailDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:instafail").unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let snap = engine.snapshot();
        let set = snap
            .device_sets
            .iter()
            .find(|s| s.id == ds)
            .expect("faulted set must stay listed");
        if set.status == DeviceSetStatus::Error {
            assert!(
                set.error
                    .as_deref()
                    .expect("error message")
                    .contains("died at start"),
                "fault message must surface: {:?}",
                set.error
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "set stuck in {:?} without surfacing the fault",
            set.status
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn probe_disappearance_faults_running_set_after_two_misses() {
    let present = Arc::new(AtomicBool::new(true));
    let mut registry = DeviceRegistry::new();
    registry.register(
        50,
        Box::new(VanishingDriver {
            present: present.clone(),
        }),
    );
    let engine = Engine::with_registry(registry, None);
    let mut events = engine.subscribe_events();
    let ds = engine.create_device_set("mock:vanish").unwrap();

    let mut known = None;
    let mut missing_once = HashSet::new();
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    assert_eq!(
        engine.snapshot().device_sets[0].status,
        DeviceSetStatus::Running,
        "present device must not be faulted"
    );

    present.store(false, Ordering::SeqCst);
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    assert_eq!(
        engine.snapshot().device_sets[0].status,
        DeviceSetStatus::Running,
        "one missed probe may be a transient enumerate hiccup"
    );

    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    let snap = engine.snapshot();
    assert_eq!(snap.device_sets[0].status, DeviceSetStatus::Error);
    assert!(
        snap.device_sets[0]
            .error
            .as_deref()
            .unwrap()
            .contains("disappeared from probe"),
        "unplug reason must surface: {:?}",
        snap.device_sets[0].error
    );
    wait_for_deviceset_event(&mut events, ds).await;
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_radio_another_program_holds_is_refused_by_name() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(BusyDriver));
    let engine = Engine::with_registry(registry, None);

    let refused = engine.create_device_set("mock:busy").unwrap_err();
    assert!(refused.is_conflict(), "{refused}");
    assert!(
        refused.to_string().contains("already in use"),
        "the reason must say the radio is taken, not just fail: {refused}"
    );
    assert!(
        engine.snapshot().device_sets.is_empty(),
        "a refused open must leave no half-built set behind"
    );
}

#[tokio::test]
async fn a_quiet_bus_is_enumerated_once_not_on_every_tick() {
    let probes = Arc::new(AtomicUsize::new(0));
    let mut registry = DeviceRegistry::new();
    registry.register(
        50,
        Box::new(CountingDriver {
            probes: probes.clone(),
        }),
    );
    let engine = Engine::with_registry(registry, None);

    let mut known = None;
    let mut missing_once = HashSet::new();
    let mut gate = crate::hotplug::ProbeGate::default();
    for _ in 0..5 {
        engine.hotplug_tick(&mut known, &mut missing_once, &mut gate, false);
    }

    assert_eq!(
        probes.load(Ordering::SeqCst),
        1,
        "vendor drivers must not be woken while the USB bus is unchanged"
    );

    engine.hotplug_tick(&mut known, &mut missing_once, &mut gate, true);
    assert_eq!(
        probes.load(Ordering::SeqCst),
        2,
        "a plugged or unplugged radio must be enumerated at once"
    );
}

#[tokio::test]
async fn hotplug_tick_emits_only_on_probe_change() {
    let mut registry = DeviceRegistry::new();
    registry.register(
        50,
        Box::new(FlappingDriver {
            probes: AtomicUsize::new(0),
        }),
    );
    let engine = Engine::with_registry(registry, None);
    let mut events = engine.subscribe_events();

    let mut known = None;
    let mut missing_once = HashSet::new();
    assert!(
        !engine.hotplug_tick_for_test(&mut known, &mut missing_once),
        "first probe is baseline"
    );
    assert!(
        engine.hotplug_tick_for_test(&mut known, &mut missing_once),
        "attach must be detected"
    );

    let ev = tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await
        .expect("event within timeout")
        .expect("event");
    assert!(matches!(
        ev,
        ServerEvent::StateChanged {
            scope: StateScope::Devices
        }
    ));

    assert!(
        !engine.hotplug_tick_for_test(&mut known, &mut missing_once),
        "steady state stays quiet"
    );
}

fn virtual_engine() -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(VIRTUAL_PRIORITY, Box::new(VirtualDriver::new()));
    Engine::with_registry(registry, None)
}

#[tokio::test]
async fn probes_virtual_device() {
    let engine = virtual_engine();
    assert!(
        engine
            .probe_devices()
            .iter()
            .any(|d| d.id() == "virtual:siggen")
    );
}

#[tokio::test]
async fn one_radio_opens_into_one_device_set() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let refused = engine.create_device_set("virtual:siggen").unwrap_err();
    assert!(
        matches!(&refused, EngineError::DeviceAlreadyOpen(device, held)
            if device == "virtual:siggen" && *held == ds),
        "expected a reopen refusal, got {refused}"
    );
    assert!(refused.is_conflict());
    assert!(!refused.is_bad_request());
    assert_eq!(engine.snapshot().device_sets.len(), 1);

    engine.remove_device_set(ds).unwrap();
    engine.create_device_set("virtual:siggen").unwrap();
}

#[tokio::test]
async fn spectrum_flows_with_a_visible_tone() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let mut rx = engine.subscribe_spectrum(ds, 0).unwrap();

    let snap = tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .expect("spectrum within timeout")
        .expect("snapshot");
    assert_eq!(snap.db.len(), 4096);

    let mut sorted = snap.db.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2];
    let peak = *sorted.last().unwrap();
    assert!(
        peak - median > 20.0,
        "expected tone peak above floor (peak {peak}, median {median})"
    );

    engine.remove_device_set(ds).unwrap();
    assert!(engine.snapshot().device_sets.is_empty());
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

#[test]
fn an_lo_offset_no_channel_sits_on_is_left_where_it_was_asked_for() {
    let placed = plan_front_end(
        &tuner_caps(),
        &offset_settings(250_000.0),
        &[parked(1, 0.0)],
    )
    .lo_offset_hz;
    assert_eq!(placed, 250_000.0);
}

#[test]
fn the_lo_steps_aside_when_a_decoder_is_parked_on_it() {
    let settings = offset_settings(250_000.0);
    let on_the_artifact = parked(1, -250_000.0);
    assert!(
        !artifact_clears_channels(
            250_000.0,
            &settings,
            &tuner_caps(),
            std::slice::from_ref(&on_the_artifact)
        ),
        "the test channel was not actually sitting on the artifact"
    );

    let placed = plan_front_end(
        &tuner_caps(),
        &settings,
        std::slice::from_ref(&on_the_artifact),
    )
    .lo_offset_hz;
    assert_ne!(placed, 250_000.0, "the LO stayed under the decoder");
    assert!(
        artifact_clears_channels(
            placed,
            &settings,
            &tuner_caps(),
            std::slice::from_ref(&on_the_artifact)
        ),
        "the LO moved to another spot that is still inside the decoder"
    );
    assert!(
        placed.abs() <= sdrmm_wire::lo_offset_limit_hz(2_400_000.0),
        "the LO was pushed outside the tuner's flat passband"
    );
}

#[test]
fn the_lo_finds_a_gap_between_several_decoders() {
    let settings = offset_settings(250_000.0);
    let crowded = [
        parked(1, -250_000.0),
        parked(2, 250_000.0),
        parked(3, -187_500.0),
        parked(4, 187_500.0),
    ];
    let placed = plan_front_end(&tuner_caps(), &settings, &crowded).lo_offset_hz;
    assert!(
        artifact_clears_channels(placed, &settings, &tuner_caps(), &crowded),
        "no placement cleared four decoders, settled on {placed} Hz"
    );
}

#[test]
fn an_offset_that_was_never_asked_for_is_not_invented_to_dodge_a_channel() {
    let placed =
        plan_front_end(&tuner_caps(), &offset_settings(0.0), &[parked(1, 0.0)]).lo_offset_hz;
    assert_eq!(placed, 0.0);
}

#[test]
fn a_radio_that_has_not_said_where_it_is_tuned_keeps_its_lo_where_it_is() {
    let untuned = DeviceSettings {
        center_hz: None,
        ..offset_settings(250_000.0)
    };
    assert_eq!(
        plan_front_end(&tuner_caps(), &untuned, &[]).lo_offset_hz,
        0.0,
        "a centre the front end cannot displace was displaced anyway"
    );
}

fn managed_caps() -> Capabilities {
    Capabilities {
        dc_artifact: DcArtifact::Managed,
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

#[test]
fn managed_hardware_parks_its_artifact_without_being_asked() {
    let plan = plan_front_end(&managed_caps(), &untouched_settings(), &[parked(1, 0.0)]);
    assert_ne!(
        plan.lo_offset_hz, 0.0,
        "the artifact was left under the tune frequency"
    );
    assert!(
        plan.dc_block,
        "the artifact was moved clear but never removed"
    );
    assert!(
        artifact_clears_channels(
            plan.lo_offset_hz,
            &untouched_settings(),
            &managed_caps(),
            &[parked(1, 0.0)]
        ),
        "the artifact landed inside the decoder it was moved to avoid"
    );
}

#[test]
fn managed_hardware_ignores_the_operator_settings_it_does_not_show() {
    let asked = DeviceSettings {
        lo_offset_hz: Some(0.0),
        dc_block: Some(false),
        ..untouched_settings()
    };
    let plan = plan_front_end(&managed_caps(), &asked, &[]);
    assert_ne!(plan.lo_offset_hz, 0.0);
    assert!(plan.dc_block);
}

#[test]
fn the_blocker_stays_off_while_the_artifact_has_nowhere_to_go() {
    let boxed_in = Capabilities {
        freq_ranges: vec![sdrmm_wire::Range {
            min: 100e6,
            max: 100e6,
            step: None,
        }],
        ..managed_caps()
    };
    let plan = plan_front_end(&boxed_in, &untouched_settings(), &[parked(1, 0.0)]);
    assert_eq!(
        plan.lo_offset_hz, 0.0,
        "the LO was displaced to a frequency the tuner cannot reach"
    );
    assert!(
        !plan.dc_block,
        "the blocker notched the centre bin with a decoder sitting on it"
    );
}

#[test]
fn unknown_hardware_still_answers_to_its_settings() {
    let asked = DeviceSettings {
        dc_block: Some(true),
        ..offset_settings(250_000.0)
    };
    let plan = plan_front_end(&tuner_caps(), &asked, &[]);
    assert_eq!(plan.lo_offset_hz, 250_000.0);
    assert!(plan.dc_block);

    let quiet = DeviceSettings {
        dc_block: Some(false),
        ..offset_settings(0.0)
    };
    let plan = plan_front_end(&tuner_caps(), &quiet, &[]);
    assert_eq!(plan.lo_offset_hz, 0.0);
    assert!(!plan.dc_block);
}

#[tokio::test]
async fn a_channel_added_over_the_lo_pushes_it_aside() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:halfduplex").unwrap();
    const OFFSET: f64 = 250_000.0;
    engine
        .patch_device(
            ds,
            DeviceSettings {
                lo_offset_hz: Some(OFFSET),
                ..DeviceSettings::default()
            },
        )
        .unwrap();

    let mut rx = engine.subscribe_spectrum(ds, 0).unwrap();
    let before = snapshot_once(&mut rx, |s| (s.center_hz - s.lo_hz - OFFSET).abs() < 1.0).await;
    assert_eq!(before.center_hz - before.lo_hz, OFFSET);

    engine.add_channel(ds, 0, nfm_settings(-OFFSET)).unwrap();

    let after = snapshot_once(&mut rx, |s| (s.center_hz - s.lo_hz - OFFSET).abs() > 1.0).await;
    assert_eq!(
        after.center_hz, before.center_hz,
        "moving the LO dragged the operator's centre with it"
    );

    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn offset_tuning_moves_the_lo_without_moving_what_is_on_the_air() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:halfduplex").unwrap();
    let mut rx = engine.subscribe_spectrum(ds, 0).unwrap();

    let centred = snapshot_once(&mut rx, |s| s.lo_hz == s.center_hz).await;
    let marker = peak_hz(&centred);

    const OFFSET: f64 = 250_000.0;
    engine
        .patch_device(
            ds,
            DeviceSettings {
                lo_offset_hz: Some(OFFSET),
                ..DeviceSettings::default()
            },
        )
        .unwrap();

    let displaced = snapshot_once(&mut rx, |s| (s.center_hz - s.lo_hz - OFFSET).abs() < 1.0).await;
    assert_eq!(
        displaced.center_hz, centred.center_hz,
        "the frequency the operator asked for moved with the LO"
    );

    let bin_hz = f64::from(displaced.span_hz) / displaced.db.len() as f64;
    let moved = marker - peak_hz(&displaced);
    assert!(
        (moved - OFFSET).abs() <= MARKER_SPREAD_HZ + bin_hz,
        "the marker should follow the LO down by {OFFSET} Hz, it moved {moved} Hz"
    );

    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn blocking_dc_leaves_a_carrier_off_centre_alone() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:halfduplex").unwrap();
    let mut rx = engine.subscribe_spectrum(ds, 0).unwrap();

    let plain = snapshot_once(&mut rx, |s| !s.db.is_empty()).await;
    let marker = peak_hz(&plain);

    engine
        .patch_device(
            ds,
            DeviceSettings {
                dc_block: Some(true),
                ..DeviceSettings::default()
            },
        )
        .unwrap();

    let blocked = snapshot_once(&mut rx, |s| s.seq > plain.seq + 4).await;
    let bin_hz = f64::from(blocked.span_hz) / blocked.db.len() as f64;
    assert!(
        (peak_hz(&blocked) - marker).abs() <= MARKER_SPREAD_HZ + bin_hz,
        "the dc blocker moved a carrier that was never at dc"
    );

    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn create_emits_state_changed() {
    let engine = virtual_engine();
    let mut events = engine.subscribe_events();
    let ds = engine.create_device_set("virtual:siggen").unwrap();

    let ev = tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await
        .expect("event within timeout")
        .expect("event");
    assert!(matches!(
        ev,
        ServerEvent::StateChanged {
            scope: StateScope::All
        }
    ));
    engine.remove_device_set(ds).unwrap();
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

#[tokio::test]
async fn channel_crud_updates_state() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let ch = engine.add_channel(ds, 0, nfm_settings(0.0)).unwrap();
    assert_eq!(engine.snapshot().device_sets[0].channels.len(), 1);
    engine.remove_channel(ds, ch).unwrap();
    assert!(engine.snapshot().device_sets[0].channels.is_empty());
    assert!(engine.remove_channel(ds, 999).is_err());
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn live_position_survives_a_channel_rate_rebuild() {
    let mut registry = DeviceRegistry::new();
    registry.register(VIRTUAL_PRIORITY, Box::new(AdsbTestDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("test-adsb:surface").unwrap();
    let ch = engine
        .add_channel(
            ds,
            0,
            ChannelSettings {
                offset_hz: 0.0,
                squelch_db: None,
                squelch_auto_db: None,
                params: ChannelParams::Adsb(AdsbParams::default()),
                audio: Default::default(),
            },
        )
        .unwrap();
    let fix = PositionFix {
        latitude: 52.52,
        longitude: 13.405,
        altitude_m: Some(40.0),
        accuracy_m: Some(3.0),
        speed_mps: Some(12.0),
        track_deg: Some(90.0),
        time: "2026-08-14T12:00:00Z".to_owned(),
    };
    engine
        .update_channel_position(ds, ch, Some(fix.clone()))
        .unwrap();

    engine
        .patch_channel(
            ds,
            ch,
            ChannelSettings {
                offset_hz: 0.0,
                squelch_db: None,
                squelch_auto_db: None,
                params: ChannelParams::Adsb(AdsbParams {
                    crc_fix: false,
                    ref_lat: Some(0.0),
                    ref_lon: Some(0.0),
                }),
                audio: Default::default(),
            },
        )
        .unwrap();
    assert_eq!(
        engine.lock().device_sets[&ds].media[&ch].position.as_ref(),
        Some(&fix)
    );

    engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(2_400_000.0),
                ..Default::default()
            },
        )
        .unwrap();

    assert_eq!(
        engine.lock().device_sets[&ds].media[&ch].position.as_ref(),
        Some(&fix)
    );

    let mut decoded = engine.subscribe_decoded();
    let record = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let record = decoded.recv().await.expect("decoded stream");
            if matches!(&record.event, DecoderEvent::Adsb(message) if message.lat.is_some()) {
                break record;
            }
        }
    })
    .await
    .expect("post-rebuild local position");
    let DecoderEvent::Adsb(message) = record.event else {
        unreachable!()
    };
    assert!((message.lat.expect("latitude") - fix.latitude).abs() < 0.01);
    assert!((message.lon.expect("longitude") - fix.longitude).abs() < 0.01);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn add_channel_rejects_out_of_passband_offset() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let err = engine
        .add_channel(ds, 0, nfm_settings(1_100_000.0))
        .unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(engine.snapshot().device_sets[0].channels.is_empty());
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn add_channel_rejects_audio_settings_outside_their_controls() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let bad: [AudioProcessing; 3] = [
        AudioProcessing {
            blanker: sdrmm_wire::NoiseBlankerSettings {
                enabled: true,
                threshold: 0.2,
            },
            ..AudioProcessing::default()
        },
        AudioProcessing {
            filter: sdrmm_wire::AudioFilterSettings {
                enabled: true,
                low_hz: 3_000.0,
                high_hz: 300.0,
            },
            ..AudioProcessing::default()
        },
        AudioProcessing {
            notches: vec![sdrmm_wire::NotchSettings {
                freq_hz: 1_000.0,
                width_hz: f64::NAN,
            }],
            ..AudioProcessing::default()
        },
    ];
    for audio in bad {
        let settings = ChannelSettings {
            audio: audio.clone(),
            ..nfm_settings(0.0)
        };
        let err = engine.add_channel(ds, 0, settings).unwrap_err();
        assert!(
            err.is_bad_request(),
            "{audio:?}: expected bad request, {err}"
        );
    }
    assert!(engine.snapshot().device_sets[0].channels.is_empty());
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_channel_with_no_audio_refuses_an_audio_chain() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let settings = ChannelSettings {
        offset_hz: 0.0,
        squelch_db: None,
        squelch_auto_db: None,
        params: ChannelParams::Pocsag(sdrmm_wire::PocsagParams::default()),
        audio: AudioProcessing {
            agc: sdrmm_wire::AudioAgcMode::Fast,
            ..AudioProcessing::default()
        },
    };
    let err = engine.add_channel(ds, 0, settings).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn patching_the_audio_chain_reaches_the_running_channel() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let ch = engine.add_channel(ds, 0, nfm_settings(0.0)).unwrap();
    let patched = ChannelSettings {
        audio: AudioProcessing {
            auto_notch: true,
            agc: sdrmm_wire::AudioAgcMode::Slow,
            ..AudioProcessing::default()
        },
        ..nfm_settings(0.0)
    };
    engine.patch_channel(ds, ch, patched.clone()).unwrap();
    let live = &engine.snapshot().device_sets[0].channels[0].settings;
    assert_eq!(live.audio, patched.audio);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn patch_channel_rejects_missing_channel() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let err = engine.patch_channel(ds, 7, nfm_settings(0.0)).unwrap_err();
    assert!(err.is_not_found(), "expected not found, got {err}");
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn rate_change_stranding_a_channel_is_rejected_before_device_io() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine.add_channel(ds, 0, nfm_settings(900_000.0)).unwrap();
    let err = engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(250_000.0),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert_eq!(
        engine.snapshot().device_sets[0].settings.sample_rate,
        Some(2_048_000.0)
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_rate_rebuild_and_remove_never_strands_a_channel() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    for i in 0..40u32 {
        let ch = engine.add_channel(ds, 0, nfm_settings(100_000.0)).unwrap();
        let rate = if i % 2 == 0 { 2_400_000.0 } else { 2_048_000.0 };
        let patch = {
            let engine = engine.clone();
            tokio::task::spawn_blocking(move || {
                engine.patch_device(
                    ds,
                    DeviceSettings {
                        sample_rate: Some(rate),
                        ..Default::default()
                    },
                )
            })
        };
        let remove = {
            let engine = engine.clone();
            tokio::task::spawn_blocking(move || engine.remove_channel(ds, ch))
        };
        let (patch, remove) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(patch, remove)
        })
        .await
        .unwrap_or_else(|_| panic!("iteration {i}: patch_device/remove_channel deadlocked"));
        patch.expect("join").expect("patch ok");
        remove.expect("join").expect("remove ok");
        assert!(
            engine.snapshot().device_sets[0].channels.is_empty(),
            "iteration {i}: channel survived its removal"
        );
    }
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn ring_overrun_surfaces_in_state_and_emits_event() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(FloodingDriver));
    let engine = Engine::with_registry(registry, None);
    let mut events = engine.subscribe_events();
    let ds = engine.create_device_set("mock:flood").unwrap();

    let snap = engine.snapshot();
    assert!(
        snap.device_sets[0].overruns >= mock_ring() as u64,
        "flooded ring must report drops, got {}",
        snap.device_sets[0].overruns
    );

    let mut known = None;
    let mut missing_once = HashSet::new();
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    wait_for_deviceset_event(&mut events, ds).await;

    let mut quiet = engine.subscribe_events();
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    assert!(
        matches!(quiet.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
        "tick without overrun growth must not emit"
    );
    engine.remove_device_set(ds).unwrap();
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

#[tokio::test]
async fn record_start_stop_produces_a_finalized_sigmf_pair() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let mut events = engine.subscribe_events();
    let ds = engine.create_device_set("virtual:siggen").unwrap();

    engine.start_recording(ds, 0).unwrap();
    wait_for_deviceset_event(&mut events, ds).await;
    let live = wait_for_recorded_samples(&engine, ds, 1).await;
    assert!(!live.file.is_empty());
    live.started_at.parse::<jiff::Timestamp>().unwrap();
    assert_eq!(live.error, None);

    let finalized = engine.stop_recording(ds).unwrap();
    assert_eq!(finalized.error, None);
    assert!(finalized.samples > 0);
    assert_eq!(
        finalized.bytes,
        finalized.samples * sdrmm_recorder::BYTES_PER_SAMPLE
    );
    assert!(engine.snapshot().device_sets[0].recording.is_none());

    let reader = sdrmm_recorder::SigmfReader::open(&finalized.stem).unwrap();
    assert_eq!(reader.total_samples(), finalized.samples);
    assert_eq!(reader.meta().global.sample_rate, Some(2_048_000.0));
    assert_eq!(reader.meta().captures[0].frequency, Some(100_000_000.0));

    let playback_id = format!("virtual:file:{}", finalized.stem.display());
    assert!(engine.probe_devices().iter().any(|d| d.id() == playback_id));
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn active_recording_persists_live_position_in_sigmf_metadata() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine.start_recording(ds, 0).unwrap();
    wait_for_recorded_samples(&engine, ds, 1).await;

    engine
        .update_recording_position(
            ds,
            Some(PositionFix {
                latitude: 52.52,
                longitude: 13.405,
                altitude_m: Some(40.0),
                accuracy_m: Some(3.0),
                speed_mps: Some(5.0),
                track_deg: Some(90.0),
                time: "2026-08-14T12:00:00Z".to_owned(),
            }),
        )
        .unwrap();

    let finalized = engine.stop_recording(ds).unwrap();
    let reader = sdrmm_recorder::SigmfReader::open(&finalized.stem).unwrap();
    let capture = reader.meta().captures.last().unwrap();
    assert_eq!(
        capture.geolocation.as_ref().unwrap().coordinates,
        vec![13.405, 52.52, 40.0]
    );
    assert_eq!(capture.datetime.as_deref(), Some("2026-08-14T12:00:00Z"));
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn recording_position_rejects_an_idle_device_set() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();

    let error = engine.update_recording_position(ds, None).unwrap_err();
    assert!(matches!(
        error,
        EngineError::Recording(message) if message == "not recording"
    ));
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn recording_position_update_does_not_block_or_panic_during_stop() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine.start_recording(ds, 0).unwrap();
    wait_for_recorded_samples(&engine, ds, 1).await;

    let (stopped_tx, stopped_rx) = std::sync::mpsc::channel();
    let stopping_engine = engine.clone();
    std::thread::spawn(move || {
        stopped_tx.send(stopping_engine.stop_recording(ds)).unwrap();
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match engine.update_recording_position(ds, None) {
            Err(EngineError::Recording(message)) if message == "not recording" => break,
            Ok(()) | Err(EngineError::Recording(_)) => {}
            Err(error) => panic!("unexpected position update error: {error}"),
        }
        assert!(
            Instant::now() < deadline,
            "recording stop did not release position state"
        );
        std::thread::yield_now();
    }
    stopped_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("recording stop completed")
        .unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn double_start_and_idle_stop_are_rejected() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();

    let err = engine.stop_recording(ds).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");

    engine.start_recording(ds, 0).unwrap();
    let err = engine.start_recording(ds, 0).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");

    engine.stop_recording(ds).unwrap();
    let err = engine.stop_recording(ds).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn start_without_a_recordings_dir_is_rejected() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let err = engine.start_recording(ds, 0).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn rate_patch_is_rejected_while_recording_center_retune_is_captured() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine.start_recording(ds, 0).unwrap();
    let before = wait_for_recorded_samples(&engine, ds, 1).await;

    let err = engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(2_400_000.0),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::Recording(_)));
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    let snap = engine.snapshot();
    assert_eq!(snap.device_sets[0].settings.sample_rate, Some(2_048_000.0));
    assert!(
        snap.device_sets[0].recording.is_some(),
        "rejected patch must not kill the recording"
    );

    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(88_500_000.0),
                ..Default::default()
            },
        )
        .unwrap();
    wait_for_recorded_samples(
        &engine,
        ds,
        before.samples + ring_samples(2_048_000.0) as u64 + 200_000,
    )
    .await;
    let finalized = engine.stop_recording(ds).unwrap();
    engine.remove_device_set(ds).unwrap();

    let reader = sdrmm_recorder::SigmfReader::open(&finalized.stem).unwrap();
    let captures = &reader.meta().captures;
    assert_eq!(captures.len(), 2, "retune must append one capture segment");
    assert_eq!(captures[1].frequency, Some(88_500_000.0));
    assert!(captures[1].sample_start > 0);
}

#[tokio::test]
async fn device_fault_finalizes_the_recording() {
    let dir = tempfile::TempDir::new().unwrap();
    let die = Arc::new(AtomicBool::new(false));
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(FaultOnDemandDriver { die: die.clone() }));
    let engine = Engine::with_registry(registry, Some(dir.path().to_path_buf()));
    let ds = engine.create_device_set("mock:ondemand").unwrap();

    engine.start_recording(ds, 0).unwrap();
    let live = wait_for_recorded_samples(&engine, ds, 1).await;

    let mut events = engine.subscribe_events();
    die.store(true, Ordering::SeqCst);
    let mut saw_recordings = false;
    let mut saw_device_set = false;
    while !(saw_recordings && saw_device_set) {
        let ev = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("event within timeout")
            .expect("event");
        match ev {
            ServerEvent::StateChanged {
                scope: StateScope::Recordings,
            } => saw_recordings = true,
            ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(id),
            } if id == ds => saw_device_set = true,
            _ => {}
        }
    }

    let snap = engine.snapshot();
    assert_eq!(snap.device_sets[0].status, DeviceSetStatus::Error);
    assert!(
        snap.device_sets[0].recording.is_none(),
        "fault must finalize and clear the recording"
    );
    let reader = sdrmm_recorder::SigmfReader::open(&dir.path().join(&live.file)).unwrap();
    assert!(reader.total_samples() > 0);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn recording_growth_rides_the_hotplug_tick() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine.start_recording(ds, 0).unwrap();
    wait_for_recorded_samples(&engine, ds, 1).await;

    let mut events = engine.subscribe_events();
    let mut known = None;
    let mut missing_once = HashSet::new();
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    wait_for_deviceset_event(&mut events, ds).await;

    engine.stop_recording(ds).unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn start_during_rate_patch_cannot_commit_a_wrong_rate_recording() {
    let dir = tempfile::TempDir::new().unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut registry = DeviceRegistry::new();
    registry.register(
        50,
        Box::new(BlockingApplyDriver {
            entered_tx,
            release_rx: Mutex::new(Some(release_rx)),
        }),
    );
    let engine = Engine::with_registry(registry, Some(dir.path().to_path_buf()));
    let ds = engine.create_device_set("mock:blocking").unwrap();

    let patch = {
        let engine = engine.clone();
        tokio::task::spawn_blocking(move || {
            engine.patch_device(
                ds,
                DeviceSettings {
                    sample_rate: Some(2_400_000.0),
                    ..Default::default()
                },
            )
        })
    };
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();

    let err = engine.start_recording(ds, 0).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(err.to_string().contains("in flight"), "{err}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);

    release_tx.send(()).unwrap();
    patch.await.expect("join").expect("patch ok");
    let snap = engine.snapshot();
    assert_eq!(snap.device_sets[0].settings.sample_rate, Some(2_400_000.0));
    assert!(snap.device_sets[0].recording.is_none());

    engine.start_recording(ds, 0).unwrap();
    let finalized = engine.stop_recording(ds).unwrap();
    let reader = sdrmm_recorder::SigmfReader::open(&finalized.stem).unwrap();
    assert_eq!(reader.meta().global.sample_rate, Some(2_400_000.0));
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn engine_drop_finalizes_a_live_recording() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine.start_recording(ds, 0).unwrap();
    let live = wait_for_recorded_samples(&engine, ds, 1).await;

    drop(engine);

    let stem = dir.path().join(&live.file);
    assert!(
        sdrmm_recorder::meta_path(&stem).exists(),
        "drop must join the writer and finalize the pair"
    );
    assert!(
        !dir.path()
            .join(format!("{}.sigmf-meta.tmp", live.file))
            .exists(),
        "no breadcrumb may survive an orderly teardown"
    );
    let reader = sdrmm_recorder::SigmfReader::open(&stem).unwrap();
    assert!(reader.total_samples() > 0);
}

#[tokio::test]
async fn shutdown_finalizes_recordings_emits_scopes_and_is_idempotent() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine.start_recording(ds, 0).unwrap();
    let live = wait_for_recorded_samples(&engine, ds, 1).await;

    let mut events = engine.subscribe_events();
    engine.shutdown();
    assert!(engine.snapshot().device_sets.is_empty());
    let mut saw_all = false;
    let mut saw_recordings = false;
    while !(saw_all && saw_recordings) {
        let ev = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("event within timeout")
            .expect("event");
        match ev {
            ServerEvent::StateChanged {
                scope: StateScope::All,
            } => saw_all = true,
            ServerEvent::StateChanged {
                scope: StateScope::Recordings,
            } => saw_recordings = true,
            _ => {}
        }
    }
    sdrmm_recorder::SigmfReader::open(&dir.path().join(&live.file)).unwrap();

    engine.shutdown();
    drop(engine);
}

#[tokio::test]
async fn writer_fault_surfaces_in_state_via_the_hotplug_tick() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine.start_recording(ds, 0).unwrap();
    wait_for_recorded_samples(&engine, ds, 1).await;

    {
        let mut inner = engine.lock();
        let state = inner.device_sets.get_mut(&ds).unwrap();
        state
            .recording
            .as_ref()
            .unwrap()
            .shared
            .fail("recording write failed: injected".to_string());
    }

    let mut events = engine.subscribe_events();
    let mut known = None;
    let mut missing_once = HashSet::new();
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    wait_for_deviceset_event(&mut events, ds).await;

    let rec = engine.snapshot().device_sets[0].recording.clone().unwrap();
    assert_eq!(
        rec.error.as_deref(),
        Some("recording write failed: injected")
    );

    let finalized = engine.stop_recording(ds).unwrap();
    assert_eq!(
        finalized.error.as_deref(),
        Some("recording write failed: injected")
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn record_start_on_a_missing_set_is_not_found_even_without_a_recordings_dir() {
    let engine = virtual_engine();
    let err = engine.start_recording(99, 0).unwrap_err();
    assert!(err.is_not_found(), "expected not found, got {err}");
}

#[tokio::test]
async fn record_start_io_failure_is_a_server_error_not_a_bad_request() {
    let blocker = tempfile::NamedTempFile::new().unwrap();
    let mut registry = DeviceRegistry::new();
    registry.register(VIRTUAL_PRIORITY, Box::new(VirtualDriver::new()));
    let engine = Engine::with_registry(registry, Some(blocker.path().join("recordings")));
    let ds = engine.create_device_set("virtual:siggen").unwrap();

    let err = engine.start_recording(ds, 0).unwrap_err();
    assert!(matches!(err, EngineError::RecordingIo(_)), "got {err}");
    assert!(!err.is_bad_request() && !err.is_not_found());
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn validate_honors_configured_bandwidth_and_sideband() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(250_000.0),
                ..Default::default()
            },
        )
        .unwrap();

    let usb = |offset_hz: f64| ChannelSettings {
        offset_hz,
        squelch_db: None,
        squelch_auto_db: None,
        params: ChannelParams::Ssb(SsbParams {
            sideband: Sideband::Usb,
            bandwidth_hz: 10_000.0,
        }),
        audio: Default::default(),
    };
    let wide_nfm = |offset_hz: f64| ChannelSettings {
        offset_hz,
        squelch_db: None,
        squelch_auto_db: None,
        params: ChannelParams::Nfm(NfmParams {
            bandwidth_hz: 25_000.0,
            ..NfmParams::default()
        }),
        audio: Default::default(),
    };

    let err = engine.add_channel(ds, 0, usb(120_000.0)).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    let err = engine.add_channel(ds, 0, wide_nfm(118_000.0)).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(engine.snapshot().device_sets[0].channels.is_empty());

    engine.add_channel(ds, 0, usb(-124_000.0)).unwrap();
    engine.add_channel(ds, 0, wide_nfm(112_000.0)).unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn patch_retunes_without_error() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(88_500_000.0),
                sample_rate: Some(2_400_000.0),
                ..Default::default()
            },
        )
        .unwrap();
    let snap = engine.snapshot();
    assert_eq!(snap.device_sets[0].settings.center_hz, Some(88_500_000.0));
    assert_eq!(snap.device_sets[0].settings.sample_rate, Some(2_400_000.0));
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn faulted_set_reconnects_and_restores_its_channels() {
    let die = Arc::new(AtomicBool::new(false));
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(FaultOnDemandDriver { die: die.clone() }));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:ondemand").unwrap();
    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(145_000_000.0),
                ..DeviceSettings::default()
            },
        )
        .unwrap();
    let ch = engine
        .add_channel(
            ds,
            0,
            ChannelSettings {
                offset_hz: 25_000.0,
                squelch_db: None,
                squelch_auto_db: None,
                params: ChannelParams::Nfm(NfmParams::default()),
                audio: Default::default(),
            },
        )
        .unwrap();
    let mut audio = engine.subscribe_audio(ds, ch).unwrap();

    let mut events = engine.subscribe_events();
    die.store(true, Ordering::SeqCst);
    loop {
        wait_for_deviceset_event(&mut events, ds).await;
        if engine.snapshot().device_sets[0].status == DeviceSetStatus::Error {
            break;
        }
    }

    die.store(false, Ordering::SeqCst);
    let mut known = None;
    let mut missing_once = HashSet::new();
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);

    let set = &engine.snapshot().device_sets[0];
    assert_eq!(set.status, DeviceSetStatus::Running);
    assert_eq!(set.error, None);
    assert_eq!(set.settings.center_hz, Some(145_000_000.0));
    assert_eq!(set.channels.len(), 1);
    assert_eq!(set.channels[0].id, ch);
    assert_eq!(set.channels[0].settings.offset_hz, 25_000.0);

    let packet = tokio::time::timeout(Duration::from_secs(10), audio.recv())
        .await
        .expect("audio within timeout")
        .expect("audio packet after reconnect");
    assert!(!packet.opus.is_empty());
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_patch_reports_what_the_device_holds_not_what_was_asked() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SnappingDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:snapping").unwrap();

    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(100_400_000.0),
                gains: vec![sdrmm_wire::GainValue {
                    stage: "LNA".to_string(),
                    value_db: 13.0,
                }],
                ..DeviceSettings::default()
            },
        )
        .unwrap();

    let set = &engine.snapshot().device_sets[0];
    assert_eq!(set.settings.center_hz, Some(100_000_000.0));
    assert_eq!(
        set.settings
            .gains
            .iter()
            .find(|g| g.stage == "LNA")
            .map(|g| g.value_db),
        Some(16.0),
        "the request was echoed instead of the device's own value"
    );

    engine
        .patch_device(
            ds,
            DeviceSettings {
                antenna: Some("RX2".to_string()),
                ..DeviceSettings::default()
            },
        )
        .unwrap();
    let set = &engine.snapshot().device_sets[0];
    assert_eq!(set.settings.antenna.as_deref(), Some("RX2"));
    assert_eq!(set.settings.center_hz, Some(100_000_000.0));
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_device_that_reports_no_sample_rate_is_refused() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(RatelessDriver));
    let engine = Engine::with_registry(registry, None);

    let err = engine.create_device_set("mock:rateless").unwrap_err();
    assert!(err.to_string().contains("sample rate"), "{err}");
    assert!(engine.snapshot().device_sets.is_empty());
}

#[tokio::test]
async fn a_snapped_rate_is_what_channels_are_rebuilt_on() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SnappingDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:snapping").unwrap();
    let channel = engine
        .add_channel(ds, 0, nfm_settings(0.0))
        .expect("hosted channel");

    engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(1_024_000.0),
                ..DeviceSettings::default()
            },
        )
        .unwrap();

    let set = &engine.snapshot().device_sets[0];
    assert_eq!(
        set.settings.sample_rate,
        Some(SNAPPED_RATE),
        "the request was echoed instead of the rate the device streams at"
    );
    assert!(
        set.channels.iter().any(|c| c.id == channel),
        "the channel did not survive the rebuild onto the device's rate"
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_faulted_set_releases_its_device_so_the_replug_can_reopen_it() {
    let claimed = Arc::new(AtomicBool::new(false));
    let die = Arc::new(AtomicBool::new(false));
    let mut registry = DeviceRegistry::new();
    registry.register(
        50,
        Box::new(ExclusiveDriver {
            claimed: claimed.clone(),
            die: die.clone(),
        }),
    );
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:exclusive").unwrap();
    assert!(claimed.load(Ordering::SeqCst), "the open must claim it");

    let mut events = engine.subscribe_events();
    die.store(true, Ordering::SeqCst);
    loop {
        wait_for_deviceset_event(&mut events, ds).await;
        if engine.snapshot().device_sets[0].status == DeviceSetStatus::Error {
            break;
        }
    }
    assert!(
        !claimed.load(Ordering::SeqCst),
        "the faulted set is still holding the device"
    );

    die.store(false, Ordering::SeqCst);
    let mut known = None;
    let mut missing_once = HashSet::new();
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    let set = &engine.snapshot().device_sets[0];
    assert_eq!(set.status, DeviceSetStatus::Running, "{:?}", set.error);
    assert!(claimed.load(Ordering::SeqCst));
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn reconnect_failure_reports_once_and_keeps_the_set_faulted() {
    let mut registry = DeviceRegistry::new();
    registry.register(
        50,
        Box::new(UnopenableDriver {
            opens: AtomicUsize::new(0),
        }),
    );
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:refuse").unwrap();
    engine.mark_device_fault(ds, DeviceError::Io("unplugged".to_string()));
    let mut events = engine.subscribe_events();

    let mut known = None;
    let mut missing_once = HashSet::new();
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    let set = &engine.snapshot().device_sets[0];
    assert_eq!(set.status, DeviceSetStatus::Error);
    let reported = set.error.clone().expect("reason");
    assert!(
        reported.contains("not reopenable") && reported.contains("still claimed"),
        "unhelpful reason: {reported}"
    );
    assert!(
        events.try_recv().is_ok(),
        "the first failure must reach clients"
    );

    while events.try_recv().is_ok() {}
    engine.hotplug_tick_for_test(&mut known, &mut missing_once);
    assert!(
        events.try_recv().is_err(),
        "an unchanged reason must not re-invalidate every client"
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn scan_finds_a_carrier_holds_and_owns_the_tuning() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:signal").unwrap();
    let ch = engine
        .add_channel(
            ds,
            0,
            ChannelSettings {
                offset_hz: 0.0,
                squelch_db: None,
                squelch_auto_db: None,
                params: ChannelParams::Nfm(NfmParams::default()),
                audio: Default::default(),
            },
        )
        .unwrap();

    let settings = sdrmm_wire::ScanSettings {
        ranges: vec![sdrmm_wire::ScanRange {
            start_hz: 100_000_000.0,
            stop_hz: 100_200_000.0,
            step_hz: 25_000.0,
        }],
        threshold_db: -60.0,
        dwell_ms: 60,
        resume_ms: 60_000,
        hold_channel: Some(ch),
        ..sdrmm_wire::ScanSettings::default()
    };
    let status = engine.start_scan(ds, settings).unwrap();
    assert_eq!(status.targets, 9);

    let err = engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(101_000_000.0),
                ..DeviceSettings::default()
            },
        )
        .unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(
        engine
            .start_scan(ds, sdrmm_wire::ScanSettings::default())
            .is_err()
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    let held = loop {
        let set = &engine.snapshot().device_sets[0];
        let scanner = set.scanner.clone().expect("scan listed on the set");
        assert_eq!(scanner.error, None, "scan failed");
        if scanner.state == ScanState::Holding {
            break (
                scanner,
                set.settings.center_hz.expect("center"),
                set.channels[0].settings.offset_hz,
            );
        }
        assert!(Instant::now() < deadline, "scan never found the carrier");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let (scanner, center_hz, offset_hz) = held;
    assert_eq!(scanner.current_hz, SIGNAL_HZ);
    assert!(scanner.hits >= 1);
    assert!(
        (center_hz + offset_hz - SIGNAL_HZ).abs() < 1.0,
        "hold channel parked at {} Hz, carrier at {SIGNAL_HZ} Hz",
        center_hz + offset_hz
    );

    let final_status = engine.stop_scan(ds).unwrap();
    assert_eq!(final_status.state, ScanState::Holding);
    assert!(
        engine.stop_scan(ds).is_err(),
        "double stop must be an error"
    );
    assert!(engine.snapshot().device_sets[0].scanner.is_none());
    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(101_000_000.0),
                ..DeviceSettings::default()
            },
        )
        .unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn removing_a_scanning_set_tears_the_scan_down() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:signal").unwrap();
    engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                ranges: vec![sdrmm_wire::ScanRange {
                    start_hz: 100_000_000.0,
                    stop_hz: 100_400_000.0,
                    step_hz: 25_000.0,
                }],
                threshold_db: 100.0,
                dwell_ms: 40,
                ..sdrmm_wire::ScanSettings::default()
            },
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    engine.remove_device_set(ds).unwrap();
    assert!(engine.snapshot().device_sets.is_empty());
}

#[tokio::test]
async fn scan_rejects_targets_the_tuner_cannot_reach() {
    let mut registry = DeviceRegistry::new();
    registry.register(50, Box::new(SignalDriver));
    let engine = Engine::with_registry(registry, None);
    let ds = engine.create_device_set("mock:signal").unwrap();
    let err = engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                frequencies: vec![2_400_000_000.0],
                ..sdrmm_wire::ScanSettings::default()
            },
        )
        .unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(
        err.to_string().contains("tuning range"),
        "unhelpful message: {err}"
    );
    let err = engine
        .start_scan(
            ds,
            sdrmm_wire::ScanSettings {
                frequencies: vec![100_000_000.0],
                hold_channel: Some(42),
                ..sdrmm_wire::ScanSettings::default()
            },
        )
        .unwrap_err();
    assert!(err.is_not_found(), "expected not found, got {err}");
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn channel_levels_are_measured_and_pushed_without_invalidating_state() {
    let engine = virtual_engine();
    let ds = engine
        .create_device_set("virtual:siggen")
        .expect("device set");
    let mut events = engine.event_tx.subscribe();
    let channel = engine
        .add_channel(
            ds,
            0,
            ChannelSettings {
                offset_hz: 0.0,
                squelch_db: None,
                squelch_auto_db: None,
                params: ChannelParams::Nfm(NfmParams::default()),
                audio: Default::default(),
            },
        )
        .expect("channel");

    let mut measured = f32::NEG_INFINITY;
    for _ in 0..200 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        let levels = engine.channel_levels(ds);
        if let Some(level) = levels.iter().find(|entry| entry.channel == channel) {
            measured = level.level_db;
            if measured > sdrmm_dsp::LEVEL_FLOOR_DB {
                break;
            }
        }
    }
    assert!(
        measured > sdrmm_dsp::LEVEL_FLOOR_DB,
        "the meter never rose off its floor (read {measured} dB)"
    );
    assert!(measured <= 0.0, "a level above full scale: {measured} dB");

    let levels = engine.channel_levels(ds);
    assert_eq!(levels.len(), 1);
    assert!(
        levels[0].peak_db >= levels[0].level_db,
        "the peak sits below the level it is a peak of"
    );

    while events.try_recv().is_ok() {}
    engine.level_tick();
    let mut pushed = None;
    while let Ok(event) = events.try_recv() {
        assert!(
            !matches!(event, ServerEvent::StateChanged { .. }),
            "metering invalidated client state"
        );
        if let ServerEvent::ChannelLevels { device_set, levels } = event {
            pushed = Some((device_set, levels));
        }
    }
    let (device_set, levels) = pushed.expect("the tick pushed no levels");
    assert_eq!(device_set, ds);
    assert_eq!(levels.len(), 1);
    assert_eq!(levels[0].channel, channel);

    assert!(engine.channel_levels(ds + 999).is_empty());

    engine.remove_channel(ds, channel).expect("remove channel");
    assert!(!engine.device_sets_with_channels().contains(&ds));
    assert!(engine.channel_levels(ds).is_empty());
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

#[tokio::test]
async fn a_channel_baseband_recording_lands_as_a_sigmf_pair_at_the_channel_rate() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let ch = engine.add_channel(ds, 0, nfm_settings(120_000.0)).unwrap();

    let live = engine.start_channel_baseband_recording(ds, ch).unwrap();
    assert!(live.file.starts_with(&format!("bb_{ds}_{ch}_")));
    assert_eq!(live.error, None);
    assert!(
        engine.start_channel_baseband_recording(ds, ch).is_err(),
        "one baseband recording per channel"
    );
    wait_for_baseband_samples(&engine, ds, ch, 4_800).await;

    let finalized = engine.stop_channel_baseband_recording(ds, ch).unwrap();
    assert_eq!(finalized.error, None);
    assert!(finalized.samples >= 4_800);
    assert_eq!(
        finalized.bytes,
        finalized.samples * sdrmm_recorder::BYTES_PER_SAMPLE
    );
    assert!(
        engine.snapshot().device_sets[0].channels[0]
            .baseband_recording
            .is_none()
    );

    let stem = dir.path().join(&finalized.file);
    let reader = sdrmm_recorder::SigmfReader::open(&stem).unwrap();
    assert_eq!(reader.meta().global.sample_rate, Some(48_000.0));
    assert_eq!(
        reader.meta().captures[0].frequency,
        Some(100_120_000.0),
        "a channel's baseband is centred on the channel, not the radio"
    );
    assert_eq!(reader.total_samples(), finalized.samples);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn removing_a_channel_finishes_the_baseband_it_was_writing() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let ch = engine.add_channel(ds, 0, nfm_settings(0.0)).unwrap();
    let live = engine.start_channel_baseband_recording(ds, ch).unwrap();
    wait_for_baseband_samples(&engine, ds, ch, 480).await;

    engine.remove_channel(ds, ch).unwrap();
    assert!(engine.stop_channel_baseband_recording(ds, ch).is_err());

    let stem = dir.path().join(&live.file);
    let reader = sdrmm_recorder::SigmfReader::open(&stem).expect("the pair was finalized");
    assert!(reader.total_samples() >= 480);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_channel_network_export_carries_that_channel_and_not_the_radio() {
    let engine = virtual_engine();
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let ch = engine.add_channel(ds, 0, nfm_settings(0.0)).unwrap();
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let settings = NetworkExportSettings {
        address: socket.local_addr().unwrap().to_string(),
        ..NetworkExportSettings::default()
    };

    let live = engine
        .start_channel_network_export(ds, ch, "net".to_owned(), settings.clone())
        .unwrap();
    assert_eq!(
        live.sample_rate, 48_000,
        "the channel's rate, not the radio's"
    );
    assert_eq!(live.node, "net");
    assert!(
        engine
            .start_channel_network_export(ds, ch, "other".to_owned(), settings.clone())
            .is_err(),
        "one export per channel"
    );

    let mut buffer = [0u8; 2_048];
    let read = socket.recv(&mut buffer).expect("datagram");
    assert!(
        read > 0 && read.is_multiple_of(8),
        "cf32 pairs arrive whole"
    );

    assert!(
        engine
            .stop_channel_network_export(ds, ch, "someone-else")
            .is_err(),
        "another node cannot stop this export"
    );
    let done = engine.stop_channel_network_export(ds, ch, "net").unwrap();
    assert!(done.bytes > 0);
    assert_eq!(done.error, None);
    assert!(
        engine.snapshot().device_sets[0].channels[0]
            .network_export
            .is_none()
    );
    engine.remove_device_set(ds).unwrap();
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

#[tokio::test]
async fn the_time_machine_captures_the_seconds_that_already_went_past() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let settings = TimeMachineNode { history_seconds: 2 };

    let armed = engine
        .control_time_machine(ds, "tm".to_owned(), 0, TimeMachineAction::Arm, settings)
        .unwrap();
    assert_eq!(armed.capacity_samples, 2 * 2_048_000);
    assert_eq!(armed.history_seconds, 2);
    assert!(armed.capture.is_none());
    assert!(
        engine
            .control_time_machine(ds, "tm2".to_owned(), 0, TimeMachineAction::Arm, settings)
            .is_err(),
        "one time machine per radio"
    );

    let held = wait_for_history(&engine, ds, 2_048_000).await;
    assert!(
        held.held_samples >= 2_048_000,
        "a second of history at least"
    );

    let capturing = engine
        .control_time_machine(ds, "tm".to_owned(), 0, TimeMachineAction::Capture, settings)
        .unwrap();
    let capture = capturing.capture.expect("a capture is running");
    assert!(capture.file.starts_with(&format!("tm_{ds}_")));

    let stopped = engine
        .control_time_machine(ds, "tm".to_owned(), 0, TimeMachineAction::Stop, settings)
        .unwrap();
    let finished = stopped.capture.expect("the stop reports what it wrote");
    assert_eq!(finished.file, capture.file);
    assert!(finished.samples >= 2_048_000);
    assert!(
        engine.snapshot().device_sets[0]
            .time_machine
            .as_ref()
            .is_some_and(|history| history.capture.is_none()),
        "the armed history stays, its capture does not"
    );

    let stem = dir.path().join(&capture.file);
    let reader = sdrmm_recorder::SigmfReader::open(&stem).expect("finalized pair");
    assert!(
        reader.total_samples() >= 2_048_000,
        "the buffered past never reached the file: {} samples",
        reader.total_samples()
    );
    assert_eq!(reader.meta().global.sample_rate, Some(2_048_000.0));

    let disarmed = engine
        .control_time_machine(ds, "tm".to_owned(), 0, TimeMachineAction::Disarm, settings)
        .unwrap();
    assert_eq!(disarmed.node, "tm");
    assert!(engine.snapshot().device_sets[0].time_machine.is_none());
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn an_armed_time_machine_locks_the_sample_rate_and_refuses_a_window_that_will_not_fit() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();

    let too_wide = engine
        .control_time_machine(
            ds,
            "tm".to_owned(),
            0,
            TimeMachineAction::Arm,
            TimeMachineNode {
                history_seconds: MAX_TIME_MACHINE_SECONDS,
            },
        )
        .unwrap_err();
    assert!(
        too_wide.to_string().contains("MiB"),
        "the refusal names the memory it would take: {too_wide}"
    );

    engine
        .control_time_machine(
            ds,
            "tm".to_owned(),
            0,
            TimeMachineAction::Arm,
            TimeMachineNode { history_seconds: 1 },
        )
        .unwrap();
    let locked = engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(1_024_000.0),
                ..DeviceSettings::default()
            },
        )
        .unwrap_err();
    assert!(
        locked.to_string().contains("disarm it first"),
        "the refusal says what to do: {locked}"
    );

    engine
        .control_time_machine(
            ds,
            "tm".to_owned(),
            0,
            TimeMachineAction::Disarm,
            TimeMachineNode::default(),
        )
        .unwrap();
    engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(1_024_000.0),
                ..DeviceSettings::default()
            },
        )
        .expect("a disarmed radio retunes its rate");
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_time_machine_action_names_the_node_that_owns_the_history() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let settings = TimeMachineNode { history_seconds: 1 };

    let idle = engine
        .control_time_machine(ds, "tm".to_owned(), 0, TimeMachineAction::Capture, settings)
        .unwrap_err();
    assert!(idle.to_string().contains("no time machine"));

    engine
        .control_time_machine(ds, "tm".to_owned(), 0, TimeMachineAction::Arm, settings)
        .unwrap();
    let stranger = engine
        .control_time_machine(
            ds,
            "other".to_owned(),
            0,
            TimeMachineAction::Capture,
            settings,
        )
        .unwrap_err();
    assert!(stranger.to_string().contains("belongs to node `tm`"));

    let idle_stop = engine
        .control_time_machine(ds, "tm".to_owned(), 0, TimeMachineAction::Stop, settings)
        .unwrap_err();
    assert!(idle_stop.to_string().contains("not laying down a capture"));
    engine.remove_device_set(ds).unwrap();
}

#[test]
fn the_builtin_registry_carries_every_backend_this_build_compiled_in() {
    let ids: Vec<&str> = builtin_registry(None)
        .driver_ids()
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    assert!(ids.contains(&"virtual"), "{ids:?}");
    #[cfg(feature = "rtlsdr")]
    assert!(ids.contains(&"rtlsdr"), "{ids:?}");
    #[cfg(feature = "hackrf")]
    assert!(ids.contains(&"hackrf"), "{ids:?}");
    #[cfg(feature = "soapy")]
    assert!(ids.contains(&"soapy"), "{ids:?}");
}

#[test]
fn soapy_hides_exactly_the_radios_this_build_drives_over_usb() {
    let handled = soapy_handled_natively();
    assert_eq!(handled.contains(&"rtlsdr"), cfg!(feature = "rtlsdr"));
    assert_eq!(handled.contains(&"hackrf"), cfg!(feature = "hackrf"));
    assert!(
        !handled.contains(&"sdrplay"),
        "the SDRplay driver reports unique serials and settles its duplicate by priority instead"
    );
}

#[cfg(all(feature = "rtlsdr", feature = "hackrf", feature = "soapy"))]
mod hardware;
