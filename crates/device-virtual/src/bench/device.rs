use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

use arc_swap::ArcSwap;
use sdrmm_device::{
    DeviceError, RxSink, SdrDevice,
    schedule::{self, Latency},
};
use sdrmm_wire::{
    Agc, Capabilities, DeviceInfo, DeviceSettings, Duplex, ExtraSetting, ExtraValue, GainKind,
    GainStage, GainValue, NoiseSource, Range,
};

use super::{
    BEARING_SETTING, BLOCK_LEN, BenchWorld, RADIUS_SETTING,
    lane::{self, LaneContext},
    scene::{BenchDeviceSpec, Scene},
};
use crate::{DRIVER_ID, validate_tune};

pub(crate) const BENCH_RATES: [f64; 4] = [250_000.0, 1_024_000.0, 2_048_000.0, 2_400_000.0];
const DEFAULT_RATE_HZ: f64 = 2_400_000.0;
const DEFAULT_CENTER_HZ: f64 = 100_000_000.0;
const MIN_CENTER_HZ: f64 = 1_000_000.0;
const MAX_CENTER_HZ: f64 = 6_000_000_000.0;
const GAIN: Range = Range {
    min: 0.0,
    max: 50.0,
    step: Some(1.0),
};
const RADIUS: Range = Range {
    min: 0.05,
    max: 10.0,
    step: Some(0.01),
};
const BEARING: Range = Range {
    min: 0.0,
    max: 360.0,
    step: Some(0.1),
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DeviceTune {
    pub(crate) sample_rate: f64,
    pub(crate) radio_center_hz: f64,
    pub(crate) centers_hz: Vec<f64>,
    pub(crate) gains_db: Vec<f64>,
}

impl DeviceTune {
    fn of(settings: &DeviceSettings, capabilities: &Capabilities) -> Self {
        let radio_center_hz = settings.center_hz.unwrap_or(DEFAULT_CENTER_HZ);
        let lanes = (0..capabilities.rx_streams)
            .map(|stream| settings.for_stream(stream, &capabilities.per_stream));
        let (centers_hz, gains_db) = lanes
            .map(|lane| {
                (
                    lane.center_hz.unwrap_or(radio_center_hz),
                    lane.gain(GainKind::Tuner.name()).unwrap_or(0.0),
                )
            })
            .unzip();
        Self {
            sample_rate: settings.sample_rate.unwrap_or(DEFAULT_RATE_HZ),
            radio_center_hz,
            centers_hz,
            gains_db,
        }
    }

    pub(crate) fn center(&self, lane: usize) -> f64 {
        self.centers_hz
            .get(lane)
            .copied()
            .unwrap_or(self.radio_center_hz)
    }

    pub(crate) fn gain(&self, lane: usize) -> f64 {
        self.gains_db.get(lane).copied().unwrap_or(0.0)
    }
}

fn measured_radius(spec: &BenchDeviceSpec, scene: &Scene) -> Option<f64> {
    let elements = scene
        .positions
        .get(spec.first_element..spec.first_element + spec.lanes.len())?;
    if elements.len() < 2 {
        return None;
    }
    let count = elements.len() as f64;
    let centre = [0, 1, 2].map(|axis| elements.iter().map(|p| p[axis]).sum::<f64>() / count);
    let spread = elements
        .iter()
        .map(|p| {
            let d = [p[0] - centre[0], p[1] - centre[1], p[2] - centre[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        })
        .sum::<f64>();
    Some(spread / count)
}

pub(crate) fn capabilities(spec: &BenchDeviceSpec) -> Capabilities {
    let mut extra = vec![ExtraSetting::range(
        BEARING_SETTING,
        "Bearing",
        BEARING,
        "°",
    )];
    if spec.lanes.len() >= 2 {
        extra.push(ExtraSetting::range(RADIUS_SETTING, "Radius", RADIUS, "m"));
    }
    Capabilities {
        freq_ranges: vec![Range {
            min: MIN_CENTER_HZ,
            max: MAX_CENTER_HZ,
            step: None,
        }],
        sample_rates: BENCH_RATES.to_vec(),
        sample_rate_ranges: Vec::new(),
        gains: vec![GainStage::new(GainKind::Tuner, GAIN)],
        antennas: vec!["RX".to_string()],
        bandwidths: Vec::new(),
        bandwidth_ranges: Vec::new(),
        bandwidth_auto: false,
        bias_tee: false,
        agc: Agc::None,
        extra,
        ppm: false,
        duplex: Duplex::RxOnly,
        rx_streams: u32::try_from(spec.lanes.len()).unwrap_or(u32::MAX),
        tx_streams: 0,
        per_stream: spec.per_stream,
        directional: None,
        dc_artifact: spec.dc_artifact,
        hardware_sweep: false,
        coherence: spec.coherence,
        noise_source: spec.noise_source,
        retune_keeps_phase: spec.retune_keeps_phase,
        rx_stream_choices: Vec::new(),
    }
}

pub(crate) fn info(spec: &BenchDeviceSpec) -> DeviceInfo {
    DeviceInfo {
        driver: DRIVER_ID.to_string(),
        key: spec.key.clone(),
        label: spec.label.clone(),
        serial: None,
        profile: Some(capabilities(spec).profile()),
    }
}

fn default_settings(spec: &BenchDeviceSpec, scene: &Scene) -> DeviceSettings {
    let mut extra = Vec::new();
    if let Some(emitter) = scene.emitters.first() {
        extra.push(ExtraValue {
            name: BEARING_SETTING.to_string(),
            value: emitter.azimuth_deg.into(),
        });
    }
    if let Some(radius) = measured_radius(spec, scene).filter(|_| spec.lanes.len() >= 2) {
        extra.push(ExtraValue {
            name: RADIUS_SETTING.to_string(),
            value: radius.into(),
        });
    }
    DeviceSettings {
        center_hz: Some(DEFAULT_CENTER_HZ),
        sample_rate: Some(DEFAULT_RATE_HZ),
        extra,
        ..DeviceSettings::default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Extras {
    bearing_deg: Option<f64>,
    radius_m: Option<f64>,
}

fn bounded(extra: &ExtraValue, range: Range) -> Result<f64, DeviceError> {
    match extra.value.as_f64() {
        Some(value) if (range.min..=range.max).contains(&value) => Ok(value),
        _ => Err(DeviceError::Unsupported(format!(
            "`{}` must be a number in {}..={}, got {}",
            extra.name, range.min, range.max, extra.value
        ))),
    }
}

fn extras(settings: &DeviceSettings, capabilities: &Capabilities) -> Result<Extras, DeviceError> {
    let mut parsed = Extras::default();
    for extra in &settings.extra {
        let offered = capabilities
            .extra
            .iter()
            .any(|setting| setting.name() == extra.name);
        match extra.name.as_str() {
            BEARING_SETTING if offered => parsed.bearing_deg = Some(bounded(extra, BEARING)?),
            RADIUS_SETTING if offered => parsed.radius_m = Some(bounded(extra, RADIUS)?),
            other => return Err(DeviceError::Unsupported(format!("extra `{other}`"))),
        }
    }
    Ok(parsed)
}

fn check_gains(settings: &DeviceSettings) -> Result<(), DeviceError> {
    let stages = settings
        .gains
        .iter()
        .chain(settings.streams.iter().flat_map(|stream| &stream.gains));
    for gain in stages {
        check_gain(gain)?;
    }
    Ok(())
}

fn check_gain(gain: &GainValue) -> Result<(), DeviceError> {
    if gain.stage != GainKind::Tuner.name() {
        return Err(DeviceError::Unsupported(format!(
            "gain stage `{}`: this radio has one {} stage",
            gain.stage,
            GainKind::Tuner.name()
        )));
    }
    if !(GAIN.min..=GAIN.max).contains(&gain.value_db) {
        return Err(DeviceError::Unsupported(format!(
            "gain {} dB outside {}..={} dB",
            gain.value_db, GAIN.min, GAIN.max
        )));
    }
    Ok(())
}

fn check_agc(settings: &DeviceSettings) -> Result<(), DeviceError> {
    if settings.agc.as_ref().is_some_and(|agc| agc.on) {
        return Err(DeviceError::Unsupported(
            "agc: this radio has no AGC".to_string(),
        ));
    }
    Ok(())
}

#[derive(Default)]
struct LaneThreads {
    running: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
}

impl LaneThreads {
    fn start(&mut self, lanes: Vec<(String, LaneContext, RxSink)>) -> Result<(), DeviceError> {
        if !self.handles.is_empty() {
            return Err(DeviceError::AlreadyStreaming);
        }
        self.running.store(true, Ordering::Release);
        for (name, ctx, sink) in lanes {
            let running = self.running.clone();
            let spawned = std::thread::Builder::new()
                .name(name.clone())
                .spawn(move || {
                    schedule::claim(Latency::Critical);
                    lane::run(ctx, sink, &running);
                });
            match spawned {
                Ok(handle) => self.handles.push(handle),
                Err(err) => {
                    self.stop();
                    return Err(DeviceError::Io(format!("spawn {name}: {err}")));
                }
            }
        }
        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::Release);
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }
}

pub(crate) struct BenchDevice {
    world: Arc<BenchWorld>,
    slot: usize,
    capabilities: Capabilities,
    settings: DeviceSettings,
    tune: Arc<ArcSwap<DeviceTune>>,
    threads: LaneThreads,
}

impl BenchDevice {
    pub(crate) fn open(world: Arc<BenchWorld>, key: &str) -> Result<Self, DeviceError> {
        let slot = world.find(key)?;
        if let Some(problem) = world.problem(slot) {
            return Err(DeviceError::Unsupported(problem));
        }
        world.quiet(slot);
        let spec = world.spec(slot);
        let capabilities = capabilities(spec);
        let settings = default_settings(spec, &world.scene());
        let tune = Arc::new(ArcSwap::from_pointee(DeviceTune::of(
            &settings,
            &capabilities,
        )));
        Ok(Self {
            world,
            slot,
            capabilities,
            settings,
            tune,
            threads: LaneThreads::default(),
        })
    }

    fn key(&self) -> &str {
        &self.world.spec(self.slot).key
    }
}

impl SdrDevice for BenchDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn in_flight_samples(&self) -> u64 {
        BLOCK_LEN as u64
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        validate_tune(&self.capabilities, settings)?;
        check_gains(settings)?;
        check_agc(settings)?;
        let extras = extras(settings, &self.capabilities)?;
        self.world
            .place(self.slot, extras.bearing_deg, extras.radius_m)?;
        self.settings.merge_from(settings);
        self.tune
            .store(Arc::new(DeviceTune::of(&self.settings, &self.capabilities)));
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let expected = self.capabilities.rx_streams as usize;
        if sinks.len() != expected {
            return Err(DeviceError::Unsupported(format!(
                "this device has {expected} rx streams, got {} sinks",
                sinks.len()
            )));
        }
        if let Some(problem) = self.world.problem(self.slot) {
            return Err(DeviceError::Unsupported(problem));
        }
        let origin_s = self.world.true_time_s();
        let key = self.key().to_owned();
        let lanes = sinks
            .into_iter()
            .enumerate()
            .map(|(lane, sink)| {
                let ctx = LaneContext {
                    world: self.world.clone(),
                    slot: self.slot,
                    lane,
                    tune: self.tune.clone(),
                    origin_s,
                };
                (format!("sdrmm-bench-{key}-{lane}"), ctx, sink)
            })
            .collect();
        self.threads.start(lanes)
    }

    fn rx_stop(&mut self) {
        self.threads.stop();
    }

    fn set_noise_source(&mut self, on: bool) -> Result<(), DeviceError> {
        if self.capabilities.noise_source == NoiseSource::None {
            return Err(DeviceError::Unsupported(
                "this radio carries no calibration noise source".to_string(),
            ));
        }
        self.world.switch_noise(self.slot, on);
        Ok(())
    }
}

impl Drop for BenchDevice {
    fn drop(&mut self) {
        self.threads.stop();
        self.world.quiet(self.slot);
    }
}
