use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;
use num_complex::Complex;
use sdrmm_device::{
    DeviceDriver, DeviceError, RxSink, SdrDevice, SweepPlan, SweepSink, Worker,
    check_stream_settings, single_rx_sink,
};
use sdrmm_recorder::scan_stems;
use sdrmm_wire::{
    Capabilities, Coherence, DcArtifact, DeviceInfo, DeviceSettings, Duplex, Range, StreamScope,
};

pub mod array;
mod playback;
pub use playback::{FilePlayback, LOOP_SETTING};

const DRIVER_ID: &str = "virtual";
const SIGGEN_KEY: &str = "siggen";
const FILE_KEY_PREFIX: &str = "file:";
const BLOCK_SECS: f64 = 0.025;

pub const NFM_CARRIER_OFFSET_HZ: f64 = 300_000.0;
pub const NFM_DEVIATION_HZ: f64 = 2_500.0;
pub const AM_CARRIER_OFFSET_HZ: f64 = -300_000.0;
pub const AM_MOD_DEPTH: f64 = 0.6;
pub const WFM_CARRIER_OFFSET_HZ: f64 = 600_000.0;
pub const WFM_DEVIATION_HZ: f64 = 75_000.0;
pub const MOD_TONE_HZ: f64 = 1_000.0;

pub const STREAM_MARKER_SPACING_HZ: f64 = 50_000.0;

/// Where the pretend firmware sweep parks its one carrier, so a sweep that works is a sweep that
/// finds it.
pub const SWEEP_MARKER_HZ: f64 = 101_000_000.0;
const SWEEP_BLOCK_SAMPLES: usize = 4_096;

#[must_use]
pub fn stream_marker_offset_hz(stream: u32) -> f64 {
    STREAM_MARKER_SPACING_HZ * (f64::from(stream) + 1.0)
}

const MOD_CARRIER_AMP: f64 = 0.20;

const DEFAULT_CENTER_HZ: f64 = 100_000_000.0;
const DEFAULT_SAMPLE_RATE_HZ: f64 = 2_048_000.0;

const NOISE_SEED: u64 = 0x5DEE_CE66_D00D_1234;

pub struct VirtualDriver {
    recordings_dir: Option<PathBuf>,
    synthetic_devices: bool,
    playback_speed: f64,
}

fn checked_speed(playback_speed: f64) -> f64 {
    assert!(
        playback_speed.is_finite() && playback_speed >= 1.0,
        "playback speed must be finite and at least real time"
    );
    playback_speed
}

impl Default for VirtualDriver {
    fn default() -> Self {
        Self::for_build(None)
    }
}

impl VirtualDriver {
    #[must_use]
    pub fn new() -> Self {
        Self::configured(None, true, 1.0)
    }

    #[must_use]
    pub fn with_recordings(dir: PathBuf) -> Self {
        Self::configured(Some(dir), true, 1.0)
    }

    #[must_use]
    pub fn with_accelerated_recordings(dir: PathBuf, playback_speed: f64) -> Self {
        Self::configured(Some(dir), true, checked_speed(playback_speed))
    }

    #[must_use]
    pub fn for_build(recordings_dir: Option<PathBuf>) -> Self {
        Self::for_build_accelerated(recordings_dir, 1.0)
    }

    #[must_use]
    pub fn for_build_accelerated(recordings_dir: Option<PathBuf>, playback_speed: f64) -> Self {
        Self::configured(
            recordings_dir,
            cfg!(debug_assertions),
            checked_speed(playback_speed),
        )
    }

    fn configured(
        recordings_dir: Option<PathBuf>,
        synthetic_devices: bool,
        playback_speed: f64,
    ) -> Self {
        Self {
            recordings_dir,
            synthetic_devices,
            playback_speed,
        }
    }

    fn siggen_info() -> DeviceInfo {
        DeviceInfo {
            driver: DRIVER_ID.to_string(),
            key: SIGGEN_KEY.to_string(),
            label: "Signal Generator (virtual)".to_string(),
            serial: None,
            profile: Some(siggen_capabilities().profile()),
        }
    }

    fn marker_info(shape: &MarkerShape) -> DeviceInfo {
        DeviceInfo {
            driver: DRIVER_ID.to_string(),
            key: shape.key.to_string(),
            label: shape.label.to_string(),
            serial: None,
            profile: Some(marker_capabilities(shape).profile()),
        }
    }

    fn playback_info(stem: &Path) -> Option<DeviceInfo> {
        let stem_str = stem.to_str()?;
        let name = stem.file_name()?.to_str()?;
        Some(DeviceInfo {
            driver: DRIVER_ID.to_string(),
            key: format!("{FILE_KEY_PREFIX}{stem_str}"),
            label: format!("{name} (recording)"),
            serial: None,
            profile: None,
        })
    }
}

impl DeviceDriver for VirtualDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        let mut infos = Vec::new();
        if self.synthetic_devices {
            infos.push(Self::siggen_info());
            infos.extend(MARKER_SHAPES.iter().map(Self::marker_info));
        }
        if let Some(dir) = &self.recordings_dir
            && let Ok(stems) = scan_stems(dir)
        {
            infos.extend(stems.iter().filter_map(|stem| Self::playback_info(stem)));
        }
        infos
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        if let Some(stem) = info.key.strip_prefix(FILE_KEY_PREFIX) {
            return Ok(Box::new(FilePlayback::open_at_speed(
                Path::new(stem),
                self.playback_speed,
            )?));
        }
        if !self.synthetic_devices {
            return Err(DeviceError::NotFound(format!("{DRIVER_ID}:{}", info.key)));
        }
        if info.key == SIGGEN_KEY {
            return Ok(Box::new(SigGen::new()));
        }
        match MARKER_SHAPES.iter().find(|shape| shape.key == info.key) {
            Some(shape) => Ok(Box::new(MarkerGen::new(shape))),
            None => Err(DeviceError::NotFound(format!("{DRIVER_ID}:{}", info.key))),
        }
    }
}

#[must_use]
pub fn render(sample_rate: f64, n: usize) -> Vec<Complex<f32>> {
    let mut block = vec![Complex::new(0.0, 0.0); n];
    Generator::new().fill(&mut block, sample_rate);
    block
}

#[derive(Clone, Copy)]
struct SigParams {
    sample_rate: f64,
}

pub struct SigGen {
    capabilities: Capabilities,
    settings: DeviceSettings,
    shared: Arc<ArcSwap<SigParams>>,
    worker: Worker,
    sweeper: Worker,
}

impl Default for SigGen {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use]
fn siggen_capabilities() -> Capabilities {
    Capabilities {
        freq_ranges: vec![Range {
            min: 0.0,
            max: 6_000_000_000.0,
            step: None,
        }],
        sample_rates: vec![
            250_000.0,
            1_024_000.0,
            2_000_000.0,
            2_048_000.0,
            2_400_000.0,
            3_200_000.0,
        ],
        sample_rate_ranges: Vec::new(),
        gains: Vec::new(),
        antennas: vec!["RX".to_string()],
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
        hardware_sweep: true,
        coherence: sdrmm_wire::Coherence::None,
    }
}

/// Every tuning the sweep visits, in the order the firmware would walk them.
fn sweep_centers(plan: &SweepPlan) -> Vec<f64> {
    let step = plan.sample_rate_hz;
    let mut centers = Vec::new();
    for band in &plan.bands {
        let mut center = band.start_hz + step / 2.0;
        while center - step / 2.0 < band.stop_hz {
            centers.push(center);
            center += step;
        }
    }
    centers
}

impl SigGen {
    #[must_use]
    pub fn new() -> Self {
        Self {
            capabilities: siggen_capabilities(),
            settings: default_settings(),
            shared: Arc::new(ArcSwap::from_pointee(SigParams {
                sample_rate: DEFAULT_SAMPLE_RATE_HZ,
            })),
            worker: Worker::new(),
            sweeper: Worker::new(),
        }
    }

    fn sample_rate(&self) -> f64 {
        self.settings.sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE_HZ)
    }
}

fn default_settings() -> DeviceSettings {
    DeviceSettings {
        center_hz: Some(DEFAULT_CENTER_HZ),
        sample_rate: Some(DEFAULT_SAMPLE_RATE_HZ),
        ..DeviceSettings::default()
    }
}

fn validate_tune(
    capabilities: &Capabilities,
    settings: &DeviceSettings,
) -> Result<(), DeviceError> {
    check_stream_settings(settings, capabilities)?;
    if let Some(rate) = settings.sample_rate
        && (rate <= 0.0 || !capabilities.sample_rates.contains(&rate))
    {
        return Err(DeviceError::Unsupported(format!("sample_rate {rate}")));
    }
    let in_range = |f: f64| {
        capabilities
            .freq_ranges
            .iter()
            .any(|r| r.min <= f && f <= r.max)
    };
    if let Some(f) = settings.center_hz
        && !in_range(f)
    {
        return Err(DeviceError::Unsupported(format!(
            "center_hz {f} outside tuner range"
        )));
    }
    for entry in &settings.streams {
        if let Some(f) = entry.center_hz
            && !in_range(f)
        {
            return Err(DeviceError::Unsupported(format!(
                "streams[{}].center_hz {f} outside tuner range",
                entry.stream
            )));
        }
    }
    Ok(())
}

impl SdrDevice for SigGen {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        validate_tune(&self.capabilities, settings)?;
        self.settings.merge_from(settings);
        self.shared.store(Arc::new(SigParams {
            sample_rate: self.sample_rate(),
        }));
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let mut sink = single_rx_sink(sinks)?;
        let shared = self.shared.clone();
        self.worker.start("sdrmm-siggen-rx", move |running| {
            let mut generator = Generator::new();
            let mut block: Vec<Complex<f32>> = Vec::new();
            let mut next = Instant::now();
            while running.load(Ordering::Acquire) {
                let params = *shared.load_full();
                let n = ((params.sample_rate * BLOCK_SECS).round() as usize).max(1);
                block.resize(n, Complex::new(0.0, 0.0));
                generator.fill(&mut block, params.sample_rate);
                sink.push(&block);

                next += Duration::from_secs_f64(n as f64 / params.sample_rate);
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else {
                    next = now;
                }
            }
        })
    }

    fn rx_stop(&mut self) {
        self.worker.stop();
    }

    fn sweep_start(&mut self, plan: &SweepPlan, mut sink: SweepSink) -> Result<(), DeviceError> {
        plan.check()?;
        let centers = sweep_centers(plan);
        if centers.is_empty() {
            return Err(DeviceError::Unsupported(
                "the sweep bands hold no tuning at this sample rate".to_string(),
            ));
        }
        let sample_rate = plan.sample_rate_hz;
        self.sweeper.start("sdrmm-siggen-sweep", move |running| {
            let mut generator = Generator::stream_marker(0);
            let mut block = vec![Complex::new(0.0f32, 0.0); SWEEP_BLOCK_SAMPLES];
            let mut next = Instant::now();
            let mut at = 0usize;
            while running.load(Ordering::Acquire) {
                let center = centers[at];
                at = (at + 1) % centers.len();
                generator.set_marker_offset_hz(SWEEP_MARKER_HZ - center);
                generator.fill(&mut block, sample_rate);
                sink.push(center, &block);

                next += Duration::from_secs_f64(block.len() as f64 / sample_rate);
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else {
                    next = now;
                }
            }
        })
    }

    fn sweep_stop(&mut self) {
        self.sweeper.stop();
    }
}

pub struct MarkerShape {
    pub key: &'static str,
    pub label: &'static str,
    pub duplex: Duplex,
    pub rx_streams: u32,
    pub tx_streams: u32,
    pub per_stream: StreamScope,
    pub coherence: Coherence,
}

pub const MARKER_SHAPES: [MarkerShape; 3] = [
    MarkerShape {
        key: "array4",
        label: "Coherent Array ×4 (virtual)",
        duplex: Duplex::RxOnly,
        rx_streams: 4,
        tx_streams: 0,
        per_stream: StreamScope {
            tuning: false,
            gain: true,
            antenna: false,
        },
        coherence: Coherence::PhaseCoherent,
    },
    MarkerShape {
        key: "transceiver",
        label: "Transceiver 2×2 (virtual)",
        duplex: Duplex::Full,
        rx_streams: 2,
        tx_streams: 2,
        per_stream: StreamScope {
            tuning: true,
            gain: true,
            antenna: true,
        },
        coherence: Coherence::None,
    },
    MarkerShape {
        key: "halfduplex",
        label: "Half-duplex 1×1 (virtual)",
        duplex: Duplex::Half,
        rx_streams: 1,
        tx_streams: 1,
        per_stream: StreamScope {
            tuning: false,
            gain: false,
            antenna: false,
        },
        coherence: Coherence::None,
    },
];

fn marker_capabilities(shape: &MarkerShape) -> Capabilities {
    Capabilities {
        duplex: shape.duplex,
        rx_streams: shape.rx_streams,
        tx_streams: shape.tx_streams,
        per_stream: shape.per_stream,
        coherence: shape.coherence,
        extra: if shape.coherence.has_phase() {
            array::extra_settings()
        } else {
            Vec::new()
        },
        hardware_sweep: false,
        ..siggen_capabilities()
    }
}

fn marker_offsets(settings: &DeviceSettings, capabilities: &Capabilities) -> Vec<f64> {
    let radio_center = settings.center_hz.unwrap_or(DEFAULT_CENTER_HZ);
    (0..capabilities.rx_streams)
        .map(|stream| {
            let lane_center = settings
                .for_stream(stream, &capabilities.per_stream)
                .center_hz
                .unwrap_or(radio_center);
            stream_marker_offset_hz(stream) + radio_center - lane_center
        })
        .collect()
}

pub struct MarkerGen {
    capabilities: Capabilities,
    settings: DeviceSettings,
    shared: Arc<ArcSwap<MarkerParams>>,
    worker: Worker,
}

struct MarkerParams {
    sample_rate: f64,
    marker_offsets: Vec<f64>,
    array: array::ArrayParams,
    lane_phase: Vec<f64>,
}

impl MarkerGen {
    fn new(shape: &MarkerShape) -> Self {
        let capabilities = marker_capabilities(shape);
        let mut settings = default_settings();
        if capabilities.coherence.has_phase() {
            settings.extra = array::default_extra();
        }
        let shared = Arc::new(ArcSwap::from_pointee(marker_params(
            &settings,
            &capabilities,
        )));
        Self {
            capabilities,
            settings,
            shared,
            worker: Worker::new(),
        }
    }
}

fn marker_params(settings: &DeviceSettings, capabilities: &Capabilities) -> MarkerParams {
    let params = array::read(settings);
    let lanes = capabilities.rx_streams as usize;
    let center_hz = settings.center_hz.unwrap_or(DEFAULT_CENTER_HZ);
    let lane_phase = (0..lanes)
        .map(|lane| {
            let steer = array::steering_phase(lane, lanes, &params, center_hz);
            if params.scramble {
                steer + array::scramble_phase(lane, center_hz)
            } else {
                steer
            }
        })
        .collect();
    MarkerParams {
        sample_rate: settings.sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE_HZ),
        marker_offsets: marker_offsets(settings, capabilities),
        array: params,
        lane_phase,
    }
}

impl SdrDevice for MarkerGen {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        validate_tune(&self.capabilities, settings)?;
        if self.capabilities.coherence == Coherence::None && !settings.extra.is_empty() {
            return Err(DeviceError::Unsupported(format!(
                "extra `{}`",
                settings.extra[0].name
            )));
        }
        if self.capabilities.coherence != Coherence::None {
            array::validate(settings)?;
        }
        self.settings.merge_from(settings);
        let params = marker_params(&self.settings, &self.capabilities);
        if self.capabilities.coherence != Coherence::None {
            self.capabilities.coherence = if params.array.scramble {
                Coherence::TimeSync
            } else {
                Coherence::PhaseCoherent
            };
        }
        self.shared.store(Arc::new(params));
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
        let shared = self.shared.clone();
        let coherent = self.capabilities.coherence != Coherence::None;
        self.worker.start("sdrmm-marker-rx", move |running| {
            let mut lanes: Vec<(Generator, RxSink)> = sinks
                .into_iter()
                .enumerate()
                .map(|(stream, sink)| (Generator::stream_marker(stream as u32), sink))
                .collect();
            let mut block: Vec<Complex<f32>> = Vec::new();
            let mut field = array::ArrayField::new();
            let mut common: Vec<Complex<f64>> = Vec::new();
            let mut next = Instant::now();
            while running.load(Ordering::Acquire) {
                let params = shared.load_full();
                let n = ((params.sample_rate * BLOCK_SECS).round() as usize).max(1);
                block.resize(n, Complex::new(0.0, 0.0));
                let wavefront = coherent && params.array.carries_wavefront();
                if wavefront {
                    field.fill(&mut common, n, params.sample_rate);
                }
                for (stream, (generator, sink)) in lanes.iter_mut().enumerate() {
                    let offset = params
                        .marker_offsets
                        .get(stream)
                        .copied()
                        .unwrap_or_else(|| stream_marker_offset_hz(stream as u32));
                    generator.set_marker_offset_hz(offset);
                    generator.fill(&mut block, params.sample_rate);
                    if wavefront {
                        let phase = params.lane_phase.get(stream).copied().unwrap_or(0.0);
                        field.add_lane(
                            stream,
                            &mut block,
                            &common,
                            phase,
                            &params.array,
                            params.sample_rate,
                        );
                    }
                    sink.push(&block);
                }
                if wavefront {
                    field.commit(&common);
                }

                next += Duration::from_secs_f64(n as f64 / params.sample_rate);
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else {
                    next = now;
                }
            }
        })
    }

    fn rx_stop(&mut self) {
        self.worker.stop();
    }
}

struct ModCarrier {
    offset_hz: f64,
    amp: f64,
    block_amp: f64,
    kind: ModKind,
    carrier_phase: f64,
    mod_phase: f64,
}

#[derive(Clone, Copy)]
enum ModKind {
    Fm { deviation_hz: f64 },
    Am { depth: f64 },
}

impl ModCarrier {
    fn new(offset_hz: f64, kind: ModKind) -> Self {
        Self {
            offset_hz,
            amp: MOD_CARRIER_AMP,
            block_amp: 0.0,
            kind,
            carrier_phase: 0.0,
            mod_phase: 0.0,
        }
    }

    fn occupied_half_width_hz(&self) -> f64 {
        match self.kind {
            ModKind::Fm { deviation_hz } => deviation_hz + MOD_TONE_HZ,
            ModKind::Am { .. } => MOD_TONE_HZ,
        }
    }
}

struct Generator {
    tones: Vec<(f64, f64, f64)>,
    drift_amp: f64,
    drift_phase: f64,
    lfo_phase: f64,
    carriers: Vec<ModCarrier>,
    noise: Xorshift,
}

impl Generator {
    fn new() -> Self {
        Self {
            tones: vec![(0.15, 0.30, 0.0), (-0.30, 0.16, 0.0), (0.05, 0.10, 0.0)],
            drift_amp: 0.20,
            drift_phase: 0.0,
            lfo_phase: 0.0,
            carriers: vec![
                ModCarrier::new(
                    NFM_CARRIER_OFFSET_HZ,
                    ModKind::Fm {
                        deviation_hz: NFM_DEVIATION_HZ,
                    },
                ),
                ModCarrier::new(
                    AM_CARRIER_OFFSET_HZ,
                    ModKind::Am {
                        depth: AM_MOD_DEPTH,
                    },
                ),
                ModCarrier::new(
                    WFM_CARRIER_OFFSET_HZ,
                    ModKind::Fm {
                        deviation_hz: WFM_DEVIATION_HZ,
                    },
                ),
            ],
            noise: Xorshift::new(NOISE_SEED),
        }
    }

    fn stream_marker(stream: u32) -> Self {
        Self {
            tones: Vec::new(),
            drift_amp: 0.0,
            drift_phase: 0.0,
            lfo_phase: 0.0,
            carriers: vec![ModCarrier::new(
                stream_marker_offset_hz(stream),
                ModKind::Fm {
                    deviation_hz: NFM_DEVIATION_HZ,
                },
            )],
            noise: Xorshift::new(NOISE_SEED ^ (u64::from(stream) << 32)),
        }
    }

    fn set_marker_offset_hz(&mut self, offset_hz: f64) {
        for carrier in &mut self.carriers {
            carrier.offset_hz = offset_hz;
        }
    }

    fn fill(&mut self, block: &mut [Complex<f32>], sample_rate: f64) {
        use std::f64::consts::TAU;

        let hz_to_w = TAU / sample_rate;
        let lfo_w = 0.08 * hz_to_w;
        let noise_amp = 0.012;
        let mod_w = MOD_TONE_HZ * hz_to_w;

        let nyquist = 0.5 * sample_rate;
        for carrier in &mut self.carriers {
            carrier.block_amp =
                if carrier.offset_hz.abs() + carrier.occupied_half_width_hz() <= nyquist {
                    carrier.amp
                } else {
                    0.0
                };
        }

        for slot in block.iter_mut() {
            let mut re = 0.0f64;
            let mut im = 0.0f64;
            for (frac, amp, phase) in &mut self.tones {
                let w = TAU * *frac;
                *phase += w;
                re += *amp * phase.cos();
                im += *amp * phase.sin();
            }

            if self.drift_amp > 0.0 {
                self.drift_phase += TAU * 0.35 * self.lfo_phase.sin();
                self.lfo_phase += lfo_w;
                re += self.drift_amp * self.drift_phase.cos();
                im += self.drift_amp * self.drift_phase.sin();
            }

            for carrier in &mut self.carriers {
                let (inst_hz, envelope) = match carrier.kind {
                    ModKind::Fm { deviation_hz } => (
                        carrier.offset_hz + deviation_hz * carrier.mod_phase.sin(),
                        1.0,
                    ),
                    ModKind::Am { depth } => {
                        (carrier.offset_hz, 1.0 + depth * carrier.mod_phase.sin())
                    }
                };
                carrier.mod_phase += mod_w;
                carrier.carrier_phase += inst_hz * hz_to_w;
                re += carrier.block_amp * envelope * carrier.carrier_phase.cos();
                im += carrier.block_amp * envelope * carrier.carrier_phase.sin();
            }

            re += noise_amp * self.noise.next_bipolar();
            im += noise_amp * self.noise.next_bipolar();

            *slot = Complex::new(re as f32, im as f32);
        }

        for (_, _, phase) in &mut self.tones {
            *phase = phase.rem_euclid(TAU);
        }
        self.drift_phase = self.drift_phase.rem_euclid(TAU);
        self.lfo_phase = self.lfo_phase.rem_euclid(TAU);
        for carrier in &mut self.carriers {
            carrier.carrier_phase = carrier.carrier_phase.rem_euclid(TAU);
            carrier.mod_phase = carrier.mod_phase.rem_euclid(TAU);
        }
    }
}

struct Xorshift(u64);

impl Xorshift {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_bipolar(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        let bits = (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f64;
        bits / (1u64 << 23) as f64 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use std::{
        f64::consts::TAU,
        sync::{OnceLock, mpsc},
        time::Duration,
    };

    use sdrmm_wire::{GainValue, StreamSettings};

    use super::*;

    const SPECTRUM_FS: f64 = 2_400_000.0;
    const SPECTRUM_LEN: usize = 1 << 20;

    fn power_spectrum(n: usize, sample_rate: f64) -> Vec<f64> {
        let mut generator = Generator::new();
        let mut block = vec![Complex::new(0.0f32, 0.0f32); n];
        generator.fill(&mut block, sample_rate);
        spectrum_of(&block)
    }

    fn spectrum_of(samples: &[Complex<f32>]) -> Vec<f64> {
        let n = samples.len();
        let mut buf: Vec<Complex<f64>> = samples
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let w = 0.5 - 0.5 * (TAU * i as f64 / n as f64).cos();
                Complex::new(f64::from(s.re) * w, f64::from(s.im) * w)
            })
            .collect();
        rustfft::FftPlanner::new()
            .plan_fft_forward(n)
            .process(&mut buf);
        buf.iter().map(|c| c.norm_sqr()).collect()
    }

    fn band_power(power: &[f64], sample_rate: f64, center_hz: f64, half_width_hz: f64) -> f64 {
        let n = power.len() as f64;
        let bin_hz = sample_rate / n;
        power
            .iter()
            .enumerate()
            .filter(|(k, _)| {
                let k = *k as f64;
                let freq = if k <= n / 2.0 { k } else { k - n } * bin_hz;
                (freq - center_hz).abs() <= half_width_hz
            })
            .map(|(_, p)| p)
            .sum()
    }

    fn shared_spectrum() -> &'static [f64] {
        static SPECTRUM: OnceLock<Vec<f64>> = OnceLock::new();
        SPECTRUM.get_or_init(|| power_spectrum(SPECTRUM_LEN, SPECTRUM_FS))
    }

    #[test]
    fn modulated_carriers_land_at_their_offsets() {
        let power = shared_spectrum();
        for (offset, half_width) in [
            (NFM_CARRIER_OFFSET_HZ, 10_000.0),
            (AM_CARRIER_OFFSET_HZ, 10_000.0),
            (WFM_CARRIER_OFFSET_HZ, 100_000.0),
        ] {
            let in_band = band_power(power, SPECTRUM_FS, offset, half_width);
            let reference = band_power(
                power,
                SPECTRUM_FS,
                offset + offset.signum() * 4.0 * half_width,
                half_width,
            );
            assert!(
                in_band > 100.0 * reference,
                "no carrier near {offset} Hz: in-band {in_band:.3e}, reference {reference:.3e}"
            );
        }
    }

    #[test]
    fn modulation_bandwidths_are_roughly_right() {
        let power = shared_spectrum();

        let wfm_wide = band_power(power, SPECTRUM_FS, WFM_CARRIER_OFFSET_HZ, 100_000.0);
        let wfm_narrow = band_power(power, SPECTRUM_FS, WFM_CARRIER_OFFSET_HZ, 10_000.0);
        assert!(
            wfm_narrow < 0.5 * wfm_wide,
            "WFM energy not wideband: {wfm_narrow:.3e} of {wfm_wide:.3e} within ±10 kHz"
        );

        for (offset, label) in [(NFM_CARRIER_OFFSET_HZ, "NFM"), (AM_CARRIER_OFFSET_HZ, "AM")] {
            let wide = band_power(power, SPECTRUM_FS, offset, 30_000.0);
            let narrow = band_power(power, SPECTRUM_FS, offset, 10_000.0);
            assert!(
                narrow > 0.95 * wide,
                "{label} energy not confined to ±10 kHz: {narrow:.3e} of {wide:.3e}"
            );
        }
    }

    #[test]
    fn output_is_identical_regardless_of_block_size() {
        let fs = 2_048_000.0;
        let mut whole = vec![Complex::new(0.0f32, 0.0f32); 8192];
        Generator::new().fill(&mut whole, fs);

        let mut generator = Generator::new();
        let mut chunked = vec![Complex::new(0.0f32, 0.0f32); 8192];
        for chunk in chunked.chunks_mut(999) {
            generator.fill(chunk, fs);
        }

        for (i, (a, b)) in whole.iter().zip(&chunked).enumerate() {
            assert!(
                (a.re - b.re).abs() < 1e-4 && (a.im - b.im).abs() < 1e-4,
                "sample {i} diverged: {a} vs {b}"
            );
        }
    }

    #[test]
    fn carrier_past_nyquist_is_muted_not_aliased() {
        let fs = 1_024_000.0;
        let power = power_spectrum(1 << 19, fs);
        let aliased = band_power(&power, fs, -424_000.0, 100_000.0);
        let nfm = band_power(&power, fs, NFM_CARRIER_OFFSET_HZ, 10_000.0);
        assert!(
            nfm > 100.0 * aliased,
            "WFM aliased past Nyquist: {aliased:.3e} vs NFM {nfm:.3e}"
        );
    }

    #[test]
    fn probe_lists_siggen_and_marker_radios() {
        let d = VirtualDriver::new();
        let infos = d.probe();
        assert_eq!(infos.len(), 1 + MARKER_SHAPES.len());
        assert_eq!(infos[0].id(), "virtual:siggen");
        assert_eq!(infos[1].id(), "virtual:array4");
        assert_eq!(infos[2].id(), "virtual:transceiver");
        assert_eq!(infos[3].id(), "virtual:halfduplex");
    }

    fn assert_synthetic_policy(d: &VirtualDriver, enabled: bool) {
        let infos = d.probe();
        let expected = std::iter::once(VirtualDriver::siggen_info())
            .chain(MARKER_SHAPES.iter().map(VirtualDriver::marker_info));
        for info in expected {
            assert_eq!(
                infos.iter().any(|probed| probed.key == info.key),
                enabled,
                "{} has the wrong probe visibility",
                info.id()
            );
            if enabled {
                d.open(&info).unwrap();
            } else {
                assert!(matches!(
                    d.open(&info),
                    Err(DeviceError::NotFound(id)) if id == info.id()
                ));
            }
        }
    }

    #[test]
    fn application_build_policy_matches_the_profile() {
        assert_synthetic_policy(&VirtualDriver::for_build(None), cfg!(debug_assertions));
        assert_synthetic_policy(&VirtualDriver::default(), cfg!(debug_assertions));
        assert_synthetic_policy(&VirtualDriver::configured(None, true, 1.0), true);
        assert_synthetic_policy(&VirtualDriver::configured(None, false, 1.0), false);
    }

    #[test]
    fn probe_lists_finalized_recordings() {
        let dir = tempfile::TempDir::new().unwrap();
        let stem = dir.path().join("capture");
        let mut writer =
            sdrmm_recorder::SigmfWriter::create(&stem, 250_000.0, 100_000_000.0, "test").unwrap();
        writer.write_block(&[Complex::new(0.5, -0.5)]).unwrap();
        writer.finalize().unwrap();
        drop(
            sdrmm_recorder::SigmfWriter::create(
                &dir.path().join("crashed"),
                250_000.0,
                100_000_000.0,
                "test",
            )
            .unwrap(),
        );

        let d = VirtualDriver::with_recordings(dir.path().to_path_buf());
        let infos = d.probe();
        assert_eq!(infos.len(), 2 + MARKER_SHAPES.len());
        assert_eq!(infos[0].id(), "virtual:siggen");
        let recording = infos.last().unwrap();
        assert_eq!(recording.id(), format!("virtual:file:{}", stem.display()));
        assert_eq!(recording.label, "capture (recording)");
        assert!(recording.serial.is_none());
        d.open(recording).unwrap();

        let production = VirtualDriver::configured(Some(dir.path().to_path_buf()), false, 1.0);
        let production_infos = production.probe();
        assert_eq!(production_infos.len(), 1);
        assert_eq!(production_infos[0].id(), recording.id());
        production.open(&production_infos[0]).unwrap();
        assert_synthetic_policy(&production, false);

        assert!(matches!(
            d.open(&DeviceInfo {
                driver: "virtual".to_string(),
                key: format!("file:{}", dir.path().join("crashed").display()),
                label: String::new(),
                serial: None,
                profile: None,
            }),
            Err(DeviceError::NotFound(_))
        ));
    }

    fn open_virtual(key: &str) -> Box<dyn SdrDevice> {
        let driver = VirtualDriver::new();
        let infos = driver.probe();
        let info = infos.iter().find(|info| info.key == key).expect("probed");
        driver.open(info).expect("open")
    }

    #[test]
    fn marker_radios_report_their_shapes_at_probe_and_open() {
        let driver = VirtualDriver::new();
        let infos = driver.probe();
        for shape in &MARKER_SHAPES {
            let info = infos.iter().find(|info| info.key == shape.key).unwrap();
            assert_eq!(info.label, shape.label);
            assert_eq!(info.id(), format!("virtual:{}", shape.key));
            let profile = info.profile.as_ref().unwrap();
            assert_eq!(profile.duplex, shape.duplex, "{}", shape.key);
            assert_eq!(profile.rx_streams, shape.rx_streams, "{}", shape.key);
            assert_eq!(profile.tx_streams, shape.tx_streams, "{}", shape.key);
            assert_eq!(profile.per_stream, shape.per_stream, "{}", shape.key);

            let dev = driver.open(info).unwrap();
            let caps = dev.capabilities();
            assert_eq!(caps.duplex, shape.duplex, "{}", shape.key);
            assert_eq!(caps.rx_streams, shape.rx_streams, "{}", shape.key);
            assert_eq!(caps.tx_streams, shape.tx_streams, "{}", shape.key);
            assert_eq!(caps.per_stream, shape.per_stream, "{}", shape.key);
            assert_eq!(caps.sample_rates, siggen_capabilities().sample_rates);
            assert_eq!(caps.freq_ranges, siggen_capabilities().freq_ranges);
        }
    }

    #[test]
    fn marker_rx_start_requires_one_sink_per_stream() {
        let mut dev = open_virtual("array4");
        for wrong in [0usize, 1, 3, 5] {
            let sinks = (0..wrong).map(|_| RxSink::new(|_, _| {})).collect();
            match dev.rx_start(sinks) {
                Err(DeviceError::Unsupported(message)) => {
                    assert!(message.contains('4'), "{message}");
                }
                other => panic!("{wrong} sinks must be Unsupported, got {other:?}"),
            }
        }
        dev.rx_start((0..4).map(|_| RxSink::new(|_, _| {})).collect())
            .unwrap();
        dev.rx_stop();
        dev.rx_start((0..4).map(|_| RxSink::new(|_, _| {})).collect())
            .unwrap();
        dev.rx_stop();
    }

    #[test]
    fn siggen_refuses_more_than_one_sink() {
        let mut dev = SigGen::new();
        let sinks = vec![RxSink::new(|_, _| {}), RxSink::new(|_, _| {})];
        assert!(matches!(
            dev.rx_start(sinks),
            Err(DeviceError::Unsupported(_))
        ));
        dev.rx_start(vec![RxSink::new(|_, _| {})]).unwrap();
        dev.rx_stop();
    }

    #[test]
    fn each_stream_carries_its_own_marker() {
        const STREAMS: usize = 4;
        const N: usize = 1 << 17;

        let mut dev = open_virtual("array4");
        let mut receivers = Vec::new();
        let sinks = (0..STREAMS)
            .map(|_| {
                let (tx, rx) = mpsc::channel::<Vec<Complex<f32>>>();
                receivers.push(rx);
                RxSink::new(move |s, _| {
                    let _ = tx.send(s.to_vec());
                })
            })
            .collect();
        dev.rx_start(sinks).unwrap();
        let rate = dev.settings().sample_rate.unwrap();
        let streams: Vec<Vec<Complex<f32>>> = receivers
            .iter()
            .map(|rx| {
                let mut samples = Vec::new();
                while samples.len() < N {
                    samples.extend(rx.recv_timeout(Duration::from_secs(2)).unwrap());
                }
                samples.truncate(N);
                samples
            })
            .collect();
        dev.rx_stop();

        for (stream, samples) in streams.iter().enumerate() {
            let power = spectrum_of(samples);
            let own = band_power(
                &power,
                rate,
                stream_marker_offset_hz(stream as u32),
                10_000.0,
            );
            for other in 0..STREAMS {
                if other == stream {
                    continue;
                }
                let leaked = band_power(
                    &power,
                    rate,
                    stream_marker_offset_hz(other as u32),
                    10_000.0,
                );
                assert!(
                    own > 100.0 * leaked,
                    "stream {stream}: marker {other} not distinguishable \
                     (own {own:.3e}, other {leaked:.3e})"
                );
            }
        }
    }

    #[test]
    fn siggen_refuses_any_streams_entry() {
        let mut dev = SigGen::new();
        let delta = DeviceSettings {
            streams: vec![StreamSettings {
                stream: 0,
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        };
        match dev.apply(&delta) {
            Err(DeviceError::Unsupported(message)) => {
                assert!(message.contains("streams[0]"), "{message}");
            }
            other => panic!("a streams entry on the siggen must be Unsupported, got {other:?}"),
        }
        assert!(dev.settings().streams.is_empty());
    }

    #[test]
    fn array4_refuses_a_per_stream_center_but_takes_per_stream_gain() {
        let mut dev = open_virtual("array4");
        let retune = DeviceSettings {
            streams: vec![StreamSettings {
                stream: 1,
                center_hz: Some(101_000_000.0),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        };
        match dev.apply(&retune) {
            Err(DeviceError::Unsupported(message)) => {
                assert!(message.contains("center_hz"), "{message}");
            }
            other => panic!("a per-stream centre on the array must be Unsupported, got {other:?}"),
        }
        assert!(dev.settings().streams.is_empty());

        dev.apply(&DeviceSettings {
            streams: vec![StreamSettings {
                stream: 1,
                gains: vec![GainValue {
                    stage: "GAIN".to_string(),
                    value_db: 12.0,
                }],
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        })
        .unwrap();
        let streams = &dev.settings().streams;
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].stream, 1);
        assert_eq!(
            streams[0].gains,
            vec![GainValue {
                stage: "GAIN".to_string(),
                value_db: 12.0,
            }]
        );
    }

    #[test]
    fn transceiver_validates_a_per_stream_center_like_the_radio_wide_one() {
        let mut dev = open_virtual("transceiver");
        let out_of_range = DeviceSettings {
            streams: vec![StreamSettings {
                stream: 1,
                center_hz: Some(7_000_000_000.0),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        };
        match dev.apply(&out_of_range) {
            Err(DeviceError::Unsupported(message)) => {
                assert!(message.contains("streams[1]"), "{message}");
            }
            other => panic!("an out-of-range lane centre must be Unsupported, got {other:?}"),
        }
        assert!(dev.settings().streams.is_empty());

        dev.apply(&DeviceSettings {
            streams: vec![StreamSettings {
                stream: 1,
                center_hz: Some(433_920_000.0),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        })
        .unwrap();
        assert_eq!(dev.settings().streams[0].center_hz, Some(433_920_000.0));
    }

    #[test]
    fn a_per_stream_retune_moves_only_that_lanes_marker() {
        const N: usize = 1 << 17;
        const RETUNE_HZ: f64 = 50_000.0;

        let mut dev = open_virtual("transceiver");
        dev.apply(&DeviceSettings {
            streams: vec![StreamSettings {
                stream: 1,
                center_hz: Some(DEFAULT_CENTER_HZ - RETUNE_HZ),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        })
        .unwrap();

        let mut receivers = Vec::new();
        let sinks = (0..2)
            .map(|_| {
                let (tx, rx) = mpsc::channel::<Vec<Complex<f32>>>();
                receivers.push(rx);
                RxSink::new(move |s, _| {
                    let _ = tx.send(s.to_vec());
                })
            })
            .collect();
        dev.rx_start(sinks).unwrap();
        let rate = dev.settings().sample_rate.unwrap();
        let lanes: Vec<Vec<Complex<f32>>> = receivers
            .iter()
            .map(|rx| {
                let mut samples = Vec::new();
                while samples.len() < N {
                    samples.extend(rx.recv_timeout(Duration::from_secs(2)).unwrap());
                }
                samples.truncate(N);
                samples
            })
            .collect();
        dev.rx_stop();

        let power = spectrum_of(&lanes[0]);
        let own = band_power(&power, rate, stream_marker_offset_hz(0), 10_000.0);
        let shifted = band_power(
            &power,
            rate,
            stream_marker_offset_hz(0) + RETUNE_HZ,
            10_000.0,
        );
        assert!(
            own > 100.0 * shifted,
            "lane 0 moved with lane 1's retune (own {own:.3e}, shifted {shifted:.3e})"
        );

        let power = spectrum_of(&lanes[1]);
        let moved = band_power(
            &power,
            rate,
            stream_marker_offset_hz(1) + RETUNE_HZ,
            10_000.0,
        );
        let old = band_power(&power, rate, stream_marker_offset_hz(1), 10_000.0);
        assert!(
            moved > 100.0 * old,
            "lane 1's marker did not follow its centre (moved {moved:.3e}, old {old:.3e})"
        );
    }

    fn capture_lanes(
        dev: &mut Box<dyn SdrDevice>,
        lanes: usize,
        want: usize,
    ) -> Vec<Vec<Complex<f32>>> {
        let mut receivers = Vec::new();
        let sinks = (0..lanes)
            .map(|_| {
                let (tx, rx) = mpsc::channel::<Vec<Complex<f32>>>();
                receivers.push(rx);
                RxSink::new(move |s, _| {
                    let _ = tx.send(s.to_vec());
                })
            })
            .collect();
        dev.rx_start(sinks).unwrap();
        let captured = receivers
            .iter()
            .map(|rx| {
                let mut samples = Vec::new();
                while samples.len() < want {
                    samples.extend(rx.recv_timeout(Duration::from_secs(5)).unwrap());
                }
                samples.truncate(want);
                samples
            })
            .collect();
        dev.rx_stop();
        captured
    }

    fn cross_spectrum(
        reference: &[Complex<f32>],
        lane: &[Complex<f32>],
        rate: f64,
    ) -> Vec<Complex<f64>> {
        let n = reference.len();
        let bin = |samples: &[Complex<f32>]| -> Vec<Complex<f64>> {
            let mut buf: Vec<Complex<f64>> = samples
                .iter()
                .map(|s| Complex::new(f64::from(s.re), f64::from(s.im)))
                .collect();
            rustfft::FftPlanner::new()
                .plan_fft_forward(n)
                .process(&mut buf);
            buf
        };
        let a = bin(reference);
        let b = bin(lane);
        let bin_hz = rate / n as f64;
        let low = ((array::WAVEFRONT_OFFSET_HZ - 2.0 * array::WAVEFRONT_WINDOW_HZ) / bin_hz).floor()
            as usize;
        let high = ((array::WAVEFRONT_OFFSET_HZ + 2.0 * array::WAVEFRONT_WINDOW_HZ) / bin_hz).ceil()
            as usize;
        (low..=high).map(|k| b[k] * a[k].conj()).collect()
    }

    fn wavefront_phase(reference: &[Complex<f32>], lane: &[Complex<f32>], rate: f64) -> f64 {
        cross_spectrum(reference, lane, rate)
            .iter()
            .sum::<Complex<f64>>()
            .arg()
    }

    /// A pure phase offset leaves the cross spectrum flat; a delay tilts it, one bin at a time.
    fn wavefront_delay_samples(
        reference: &[Complex<f32>],
        lane: &[Complex<f32>],
        rate: f64,
    ) -> f64 {
        let cross = cross_spectrum(reference, lane, rate);
        let floor = cross.iter().map(|c| c.norm()).fold(0.0, f64::max) * 0.1;
        let tilt: Complex<f64> = cross
            .windows(2)
            .filter(|w| w[0].norm() > floor && w[1].norm() > floor)
            .map(|w| w[1] * w[0].conj())
            .sum();
        -tilt.arg() * reference.len() as f64 / TAU
    }

    fn wrap(angle: f64) -> f64 {
        let mut a = angle;
        while a > std::f64::consts::PI {
            a -= TAU;
        }
        while a < -std::f64::consts::PI {
            a += TAU;
        }
        a
    }

    fn array_settings(extra: Vec<(&str, serde_json::Value)>) -> DeviceSettings {
        DeviceSettings {
            extra: extra
                .into_iter()
                .map(|(name, value)| sdrmm_wire::ExtraValue {
                    name: name.to_string(),
                    value,
                })
                .collect(),
            ..DeviceSettings::default()
        }
    }

    #[test]
    fn a_set_wavefront_bearing_lands_on_the_analytic_steering_vector() {
        const N: usize = 1 << 15;
        let bearing = 137.0;
        let radius = 0.35;
        let mut dev = open_virtual("array4");
        dev.apply(&DeviceSettings {
            sample_rate: Some(1_024_000.0),
            center_hz: Some(300_000_000.0),
            ..array_settings(vec![
                (array::BEARING_SETTING, bearing.into()),
                (array::RADIUS_SETTING, radius.into()),
            ])
        })
        .unwrap();
        let rate = dev.settings().sample_rate.unwrap();
        let lanes = capture_lanes(&mut dev, 4, N);

        let params = array::ArrayParams {
            bearing_deg: bearing,
            radius_m: radius,
            ..array::ArrayParams::default()
        };
        for lane in 1..4 {
            let measured = wavefront_phase(&lanes[0], &lanes[lane], rate);
            let want = array::steering_phase(lane, 4, &params, 300_000_000.0)
                - array::steering_phase(0, 4, &params, 300_000_000.0);
            assert!(
                wrap(measured - want).abs() < 0.05,
                "lane {lane}: measured {measured:.4} rad, steering vector says {want:.4}"
            );
        }
    }

    #[test]
    fn scrambling_moves_phase_on_every_retune_and_leaves_the_lanes_aligned() {
        const N: usize = 1 << 14;
        let mut dev = open_virtual("array4");
        let base = DeviceSettings {
            sample_rate: Some(1_024_000.0),
            ..array_settings(vec![
                (array::SCRAMBLE_SETTING, true.into()),
                (array::RADIUS_SETTING, 0.35.into()),
            ])
        };
        dev.apply(&base).unwrap();
        assert_eq!(dev.capabilities().coherence, Coherence::TimeSync);
        let rate = dev.settings().sample_rate.unwrap();

        let mut phases = Vec::new();
        for center in [200_000_000.0, 240_000_000.0] {
            dev.apply(&DeviceSettings {
                center_hz: Some(center),
                ..DeviceSettings::default()
            })
            .unwrap();
            let lanes = capture_lanes(&mut dev, 4, N);
            phases.push(wavefront_phase(&lanes[0], &lanes[1], rate));
            let delay = wavefront_delay_samples(&lanes[0], &lanes[1], rate);
            assert!(
                delay.abs() < 5.0,
                "a shared clock leaves no delay the wavefront could resolve, measured {delay}"
            );
        }
        assert!(
            wrap(phases[0] - phases[1]).abs() > 0.1,
            "phase survived a retune: {phases:?}"
        );
    }

    #[test]
    fn a_non_coherent_marker_radio_takes_no_array_settings() {
        let mut dev = open_virtual("transceiver");
        assert_eq!(dev.capabilities().coherence, Coherence::None);
        assert!(dev.capabilities().extra.is_empty());
        let err = dev.apply(&array_settings(vec![(array::BEARING_SETTING, 10.0.into())]));
        assert!(matches!(err, Err(DeviceError::Unsupported(_))), "{err:?}");
    }

    #[test]
    fn marker_offsets_follow_each_lanes_own_center() {
        let caps = marker_capabilities(&MARKER_SHAPES[1]);
        let mut settings = default_settings();
        assert_eq!(marker_offsets(&settings, &caps), vec![50_000.0, 100_000.0]);

        settings.merge_from(&DeviceSettings {
            streams: vec![StreamSettings {
                stream: 1,
                center_hz: Some(DEFAULT_CENTER_HZ - 50_000.0),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        });
        assert_eq!(marker_offsets(&settings, &caps), vec![50_000.0, 150_000.0]);

        settings.merge_from(&DeviceSettings {
            center_hz: Some(DEFAULT_CENTER_HZ + 25_000.0),
            ..DeviceSettings::default()
        });
        assert_eq!(marker_offsets(&settings, &caps), vec![50_000.0, 175_000.0]);

        let array = marker_capabilities(&MARKER_SHAPES[0]);
        assert_eq!(
            marker_offsets(&settings, &array),
            vec![50_000.0, 100_000.0, 150_000.0, 200_000.0]
        );
    }

    #[test]
    fn render_is_deterministic() {
        let a = render(2_048_000.0, 4096);
        let b = render(2_048_000.0, 4096);
        for (i, (x, y)) in a.iter().zip(&b).enumerate() {
            assert_eq!(x.re.to_bits(), y.re.to_bits(), "re mismatch at {i}");
            assert_eq!(x.im.to_bits(), y.im.to_bits(), "im mismatch at {i}");
        }
    }

    #[test]
    fn render_matches_the_streamed_siggen() {
        let n = 8192;
        let rendered = render(250_000.0, n);

        let mut dev = SigGen::new();
        dev.apply(&DeviceSettings {
            sample_rate: Some(250_000.0),
            ..DeviceSettings::default()
        })
        .unwrap();
        let (tx, rx) = mpsc::channel::<Vec<Complex<f32>>>();
        dev.rx_start(vec![RxSink::new(move |s, _| {
            let _ = tx.send(s.to_vec());
        })])
        .unwrap();
        let mut streamed = Vec::new();
        while streamed.len() < n {
            streamed.extend(rx.recv_timeout(Duration::from_secs(2)).unwrap());
        }
        dev.rx_stop();

        for (i, (a, b)) in rendered.iter().zip(&streamed).enumerate() {
            assert!(
                (a.re - b.re).abs() < 1e-4 && (a.im - b.im).abs() < 1e-4,
                "sample {i} diverged: {a} vs {b}"
            );
        }
    }

    #[test]
    fn rejects_unsupported_rate() {
        let mut dev = SigGen::new();
        let bad = DeviceSettings {
            sample_rate: Some(999.0),
            ..DeviceSettings::default()
        };
        assert!(dev.apply(&bad).is_err());
    }

    #[test]
    fn rejects_out_of_range_center() {
        let mut dev = SigGen::new();
        for bad in [7_000_000_000.0, -5_000_000_000.0, f64::NAN] {
            let err = dev.apply(&DeviceSettings {
                center_hz: Some(bad),
                ..DeviceSettings::default()
            });
            assert!(
                matches!(err, Err(DeviceError::Unsupported(_))),
                "center {bad} must be rejected"
            );
        }
        assert_eq!(dev.settings().center_hz, Some(100_000_000.0));
        dev.apply(&DeviceSettings {
            center_hz: Some(5_900_000_000.0),
            ..DeviceSettings::default()
        })
        .unwrap();
        assert_eq!(dev.settings().center_hz, Some(5_900_000_000.0));
    }

    #[test]
    fn apply_stores_every_field_for_state_round_trips() {
        let mut dev = SigGen::new();
        dev.apply(&DeviceSettings {
            ppm: Some(1.5),
            antenna: Some("RX".to_string()),
            bandwidth: Some(1_500_000.0),
            gains: vec![GainValue {
                stage: "LNA".to_string(),
                value_db: 16.0,
            }],
            ..DeviceSettings::default()
        })
        .unwrap();

        let settings = dev.settings();
        assert_eq!(settings.ppm, Some(1.5));
        assert_eq!(settings.antenna.as_deref(), Some("RX"));
        assert_eq!(settings.bandwidth, Some(1_500_000.0));
        assert_eq!(settings.gains.len(), 1);
        assert_eq!(settings.center_hz, Some(100_000_000.0));
    }

    #[test]
    fn streams_finite_samples_then_stops() {
        let mut dev = SigGen::new();
        dev.apply(&DeviceSettings {
            sample_rate: Some(250_000.0),
            ..DeviceSettings::default()
        })
        .unwrap();

        let (tx, rx) = mpsc::channel::<usize>();
        dev.rx_start(vec![RxSink::new(move |s, _| {
            let _ = tx.send(s.len());
        })])
        .unwrap();

        let mut total = 0;
        for _ in 0..3 {
            if let Ok(n) = rx.recv_timeout(Duration::from_secs(2)) {
                total += n;
            }
        }
        dev.rx_stop();
        assert!(total > 0, "expected streamed samples");

        dev.rx_start(vec![RxSink::new(|_, _| {})]).unwrap();
        dev.rx_stop();
    }

    #[test]
    fn output_is_finite_and_bounded() {
        let mut generator = Generator::new();
        let mut block = vec![Complex::new(0.0, 0.0); 4096];
        generator.fill(&mut block, 2_048_000.0);
        for s in &block {
            assert!(s.re.is_finite() && s.im.is_finite());
            assert!(s.norm() < 2.0, "unexpectedly large sample");
        }
    }
}
