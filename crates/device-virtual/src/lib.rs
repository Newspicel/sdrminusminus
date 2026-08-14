//! `sdrmm-device-virtual` — backend (PLAN §6) providing developer-only signal generators and
//! always-available SigMF file playback (PLAN §3: playback lives here, SigMF IO in
//! `sdrmm-recorder`). This is
//! how CI, demo mode, and decoder golden tests run without hardware. The siggen synthesizes a
//! baseband IQ stream: a few fixed tones, one slowly drifting tone, and a white-noise floor
//! (the M0 spectrum path), plus NFM/AM/WFM carriers modulated by a 1 kHz tone that the M2
//! engine e2e tests demodulate. The plain tones sit at fixed fractions of the sample rate;
//! the modulated carriers sit at fixed Hz offsets from center — both ride along when the
//! device retunes, but Hz offsets let channels address the carriers at any sample rate.
//!
//! No hardware backend has several streams yet, so the multi-stream test radios
//! ([`MARKER_SHAPES`]) also live here: rx-only, half- and full-duplex shapes whose stream k
//! carries an NFM marker at [`stream_marker_offset_hz`]`(k)`, distinguishable enough for a
//! test to prove stream k reached channel k.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;
use num_complex::Complex;
use sdrmm_device::{
    DeviceDriver, DeviceError, RxSink, SdrDevice, Worker, check_stream_settings, single_rx_sink,
};
use sdrmm_recorder::scan_stems;
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings, Duplex, Range, StreamScope};

mod playback;
pub use playback::{FilePlayback, LOOP_SETTING};

const DRIVER_ID: &str = "virtual";
const SIGGEN_KEY: &str = "siggen";
/// Playback device keys are `file:<stem>`, so the full id `virtual:file:<stem>` embeds the
/// recording stem (the registry splits ids on the first `:` only).
const FILE_KEY_PREFIX: &str = "file:";
/// Target block duration; the capture thread paces itself to roughly real time.
const BLOCK_SECS: f64 = 0.025;

/// Offset of the NFM test carrier from the *current* center frequency. All `*_OFFSET_HZ`
/// are relative to center so the carriers retune with the device and tests can address them
/// the way channels do: as plain offsets, at any center frequency.
pub const NFM_CARRIER_OFFSET_HZ: f64 = 300_000.0;
/// NFM peak deviation of the 1 kHz modulating tone.
pub const NFM_DEVIATION_HZ: f64 = 2_500.0;
/// Offset of the AM test carrier from the current center frequency.
pub const AM_CARRIER_OFFSET_HZ: f64 = -300_000.0;
/// AM modulation depth of the 1 kHz tone.
pub const AM_MOD_DEPTH: f64 = 0.6;
/// Offset of the WFM test carrier from the current center frequency.
pub const WFM_CARRIER_OFFSET_HZ: f64 = 600_000.0;
/// WFM peak deviation of the 1 kHz modulating tone.
pub const WFM_DEVIATION_HZ: f64 = 75_000.0;
/// Modulating tone shared by all three carriers.
pub const MOD_TONE_HZ: f64 = 1_000.0;

/// Spacing of the per-stream markers on the multi-stream test radios: stream k's marker sits
/// at `(k+1) ×` this from center, so no marker lands on center or on another stream's offset.
pub const STREAM_MARKER_SPACING_HZ: f64 = 50_000.0;

/// Offset from center of `stream`'s marker carrier on the multi-stream test radios. An NFM
/// carrier ([`NFM_DEVIATION_HZ`], [`MOD_TONE_HZ`]) rather than a bare tone, so a channel on
/// stream k can *demodulate* its marker, not merely see energy at the offset.
#[must_use]
pub fn stream_marker_offset_hz(stream: u32) -> f64 {
    STREAM_MARKER_SPACING_HZ * (f64::from(stream) + 1.0)
}

/// Comparable to the 0.10–0.30 static tones so the carriers get a similar SNR.
const MOD_CARRIER_AMP: f64 = 0.20;

/// Tuning the synthetic radios power up with — the same defaults the engine assumes when a
/// device reports none, so hardware doubles and engine state agree from the first snapshot.
const DEFAULT_CENTER_HZ: f64 = 100_000_000.0;
const DEFAULT_SAMPLE_RATE_HZ: f64 = 2_048_000.0;

/// The siggen's noise seed; a stream marker generator derives its own from it, so no two
/// streams are sample-identical even where their markers are muted.
const NOISE_SEED: u64 = 0x5DEE_CE66_D00D_1234;

/// Driver that exposes developer signal generators when enabled, plus one playback device per
/// finalized SigMF recording when constructed with a recordings dir.
pub struct VirtualDriver {
    recordings_dir: Option<PathBuf>,
    synthetic_devices: bool,
}

impl VirtualDriver {
    /// A synthetic-only driver for hermetic tests and developer tooling.
    #[must_use]
    pub fn new() -> Self {
        Self::configured(None, true)
    }

    /// Synthetic devices plus playback for hermetic tests and developer tooling.
    #[must_use]
    pub fn with_recordings(dir: PathBuf) -> Self {
        Self::configured(Some(dir), true)
    }

    /// The driver policy used by application builds: debug builds expose the synthetic radios,
    /// while production builds retain only recording playback.
    #[must_use]
    pub fn for_build(recordings_dir: Option<PathBuf>) -> Self {
        Self::configured(recordings_dir, cfg!(debug_assertions))
    }

    fn configured(recordings_dir: Option<PathBuf>, synthetic_devices: bool) -> Self {
        Self {
            recordings_dir,
            synthetic_devices,
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
        // Non-UTF-8 stems cannot round-trip through the string device id; skip them.
        let stem_str = stem.to_str()?;
        let name = stem.file_name()?.to_str()?;
        Some(DeviceInfo {
            driver: DRIVER_ID.to_string(),
            key: format!("{FILE_KEY_PREFIX}{stem_str}"),
            label: format!("{name} (recording)"),
            serial: None,
            // The centre and rate are the recording's, and reading them means opening its
            // metadata: a probe walks the whole recordings directory, so it stays a directory
            // listing. Unknown until opened, which is what `None` says.
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
        // The hotplug prober calls this every 5 s: scan_stems is one readdir, no meta
        // parses. An unreadable dir hides the playback devices, never fails the probe.
        if let Some(dir) = &self.recordings_dir
            && let Ok(stems) = scan_stems(dir)
        {
            infos.extend(stems.iter().filter_map(|stem| Self::playback_info(stem)));
        }
        infos
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        if let Some(stem) = info.key.strip_prefix(FILE_KEY_PREFIX) {
            return Ok(Box::new(FilePlayback::open(Path::new(stem))?));
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

/// Deterministically render the first `n` samples of the siggen baseband at `sample_rate` —
/// the stream a freshly started [`SigGen`] produces. For xtask fixture synthesis and hermetic
/// tests. The output is center-independent: the modulated carriers sit at Hz offsets from
/// center, so retuning shifts what the offsets *mean*, never the baseband itself.
#[must_use]
pub fn render(sample_rate: f64, n: usize) -> Vec<Complex<f32>> {
    let mut block = vec![Complex::new(0.0, 0.0); n];
    Generator::new().fill(&mut block, sample_rate);
    block
}

/// Parameters the capture thread reads per block via [`ArcSwap`] (snapshot, no per-sample lock).
#[derive(Clone, Copy)]
struct SigParams {
    sample_rate: f64,
}

/// The virtual signal generator.
pub struct SigGen {
    capabilities: Capabilities,
    settings: DeviceSettings,
    shared: Arc<ArcSwap<SigParams>>,
    worker: Worker,
}

impl Default for SigGen {
    fn default() -> Self {
        Self::new()
    }
}

/// What the signal generator can do. A pure table, so the probe can hand it over without
/// opening anything — the picker filters templates against it like any other radio.
#[must_use]
fn siggen_capabilities() -> Capabilities {
    Capabilities {
        freq_ranges: vec![Range {
            min: 0.0,
            max: 6_000_000_000.0,
            step: None,
        }],
        // 2 Msps is here for one reason: it is the only rate ADS-B can run at (PLAN §18),
        // so without it the demo radio cannot carry the mode its own fixture decodes.
        sample_rates: vec![
            250_000.0,
            1_024_000.0,
            2_000_000.0,
            2_048_000.0,
            2_400_000.0,
            3_200_000.0,
        ],
        sample_rate_range: None,
        gains: Vec::new(),
        antennas: vec!["RX".to_string()],
        bandwidths: Vec::new(),
        extra: Vec::new(),
        ppm: false,
        duplex: Duplex::RxOnly,
        rx_streams: 1,
        tx_streams: 0,
        per_stream: StreamScope::default(),
        directional: None,
    }
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

/// Validation shared by the synthetic radios. The advertised capabilities are a contract:
/// accepting a rate or tune outside them would make these backends unfaithful doubles for
/// hardware that rejects it (device-soapy pre-flights the same checks), hiding the reject
/// path from every engine/server test.
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
    // A per-stream tuner is still this tuner: an override outside the range the radio-wide
    // dial refuses must be refused too.
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
        // Store every field via the one shared merge (`wire`), so PATCHed values round-trip
        // into state even for knobs the siggen has no behavior for (ppm, gains, bandwidth…).
        self.settings.merge_from(settings);
        // Publish the derived params so a live capture thread picks them up next block.
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

                // Pace to ~real time so spectrum fps and CPU stay realistic.
                next += Duration::from_secs_f64(n as f64 / params.sample_rate);
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else {
                    next = now; // fell behind; resync without accumulating debt
                }
            }
        })
    }

    fn rx_stop(&mut self) {
        self.worker.stop();
    }
}

/// One multi-stream test radio's shape. No hardware backend has several streams, so these
/// virtual radios are what carries the multi-stream path end to end: the matrix covers
/// rx-only, half- and full-duplex at more than one N×M.
pub struct MarkerShape {
    pub key: &'static str,
    pub label: &'static str,
    pub duplex: Duplex,
    pub rx_streams: u32,
    pub tx_streams: u32,
    /// Which settings each lane holds on its own. The matrix spans the cases per-stream
    /// settings must prove: shared tuning honoured, independent tuning working, none at all.
    pub per_stream: StreamScope,
}

/// The multi-stream test radios the driver probes, alongside the siggen and recordings.
pub const MARKER_SHAPES: [MarkerShape; 3] = [
    MarkerShape {
        key: "array4",
        label: "Coherent Array ×4 (virtual)",
        duplex: Duplex::RxOnly,
        rx_streams: 4,
        tx_streams: 0,
        // A coherent array shares one tuner reference by definition; per-channel gain is how
        // an array is levelled.
        per_stream: StreamScope {
            tuning: false,
            gain: true,
            antenna: false,
        },
    },
    MarkerShape {
        key: "transceiver",
        label: "Transceiver 2×2 (virtual)",
        duplex: Duplex::Full,
        rx_streams: 2,
        tx_streams: 2,
        // A synthesizer per lane: the shape that proves an independent retune moves exactly
        // one lane's marker.
        per_stream: StreamScope {
            tuning: true,
            gain: true,
            antenna: true,
        },
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
    },
];

/// Same tuning surface as the siggen — templates that match one match the other — with the
/// shape's stream counts, duplex and per-stream scope on top.
fn marker_capabilities(shape: &MarkerShape) -> Capabilities {
    Capabilities {
        duplex: shape.duplex,
        rx_streams: shape.rx_streams,
        tx_streams: shape.tx_streams,
        per_stream: shape.per_stream,
        ..siggen_capabilities()
    }
}

/// Where each lane's marker sits in that lane's own baseband. The marker carrier radiates at
/// `radio-wide centre + stream_marker_offset_hz(k)` — it rides the radio-wide dial exactly as
/// the siggen's carriers do — so a lane tuned apart from the radio (per-stream tuning) sees it
/// displaced by the difference, which is what lets a test prove lane k retuned and the others
/// did not. Where tuning is shared, `for_stream` resolves every lane to the radio-wide centre
/// and no marker ever moves.
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

/// A multi-stream test radio: stream k carries the shared noise floor plus its own NFM
/// marker at [`stream_marker_offset_hz`]`(k)` from the *radio-wide* centre, so a test can
/// prove stream k reached channel k by demodulating the marker's 1 kHz tone (or by band power
/// at the offset). On a shape with per-stream tuning, a lane tuned apart from the radio sees
/// its marker displaced by the difference ([`marker_offsets`]) — the observable that proves a
/// per-stream retune reached exactly one lane. The transmit side of the duplex-capable shapes
/// is declared but inert — like every radio's, `tx_start` stays behind the PLAN §12a
/// authorized-use gate nothing above the device crates crosses.
pub struct MarkerGen {
    capabilities: Capabilities,
    settings: DeviceSettings,
    shared: Arc<ArcSwap<MarkerParams>>,
    worker: Worker,
}

/// Parameters the marker capture thread reads per block: the shared clock — one radio, one
/// clock domain, whatever the lanes are tuned to — and each lane's [`marker_offsets`] entry.
struct MarkerParams {
    sample_rate: f64,
    marker_offsets: Vec<f64>,
}

impl MarkerGen {
    fn new(shape: &MarkerShape) -> Self {
        let capabilities = marker_capabilities(shape);
        let settings = default_settings();
        let shared = Arc::new(ArcSwap::from_pointee(MarkerParams {
            sample_rate: DEFAULT_SAMPLE_RATE_HZ,
            marker_offsets: marker_offsets(&settings, &capabilities),
        }));
        Self {
            capabilities,
            settings,
            shared,
            worker: Worker::new(),
        }
    }

    fn sample_rate(&self) -> f64 {
        self.settings.sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE_HZ)
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
        self.settings.merge_from(settings);
        self.shared.store(Arc::new(MarkerParams {
            sample_rate: self.sample_rate(),
            marker_offsets: marker_offsets(&self.settings, &self.capabilities),
        }));
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
        self.worker.start("sdrmm-marker-rx", move |running| {
            // Lane k feeds sinks[k]: the vec order *is* the stream order the engine binds by.
            let mut lanes: Vec<(Generator, RxSink)> = sinks
                .into_iter()
                .enumerate()
                .map(|(stream, sink)| (Generator::stream_marker(stream as u32), sink))
                .collect();
            let mut block: Vec<Complex<f32>> = Vec::new();
            let mut next = Instant::now();
            while running.load(Ordering::Acquire) {
                let params = shared.load_full();
                let n = ((params.sample_rate * BLOCK_SECS).round() as usize).max(1);
                block.resize(n, Complex::new(0.0, 0.0));
                // One block round fills every lane from the same clock, so the streams stay
                // sample-aligned — what "coherent array" promises.
                for (stream, (generator, sink)) in lanes.iter_mut().enumerate() {
                    // Both vecs are sized from `rx_streams`, so `get` always hits; the
                    // unshifted default keeps a lane audible rather than a panic on the
                    // capture thread if that invariant ever breaks.
                    let offset = params
                        .marker_offsets
                        .get(stream)
                        .copied()
                        .unwrap_or_else(|| stream_marker_offset_hz(stream as u32));
                    generator.set_marker_offset_hz(offset);
                    generator.fill(&mut block, params.sample_rate);
                    sink.push(&block);
                }

                // Pace to ~real time so spectrum fps and CPU stay realistic (as the siggen).
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

/// A modulated test carrier at a fixed Hz offset from center (unlike the fraction-of-fs
/// tones, so channels can address it identically at any sample rate).
struct ModCarrier {
    offset_hz: f64,
    amp: f64,
    /// `amp` while the occupied band fits under Nyquist at the block's sample rate, else 0.0.
    /// Scratch recomputed per [`Generator::fill`], so a variable carrier count needs no
    /// per-block allocation on the capture thread.
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

    /// Carson-rule half-width for FM; carrier ± one sideband for AM.
    fn occupied_half_width_hz(&self) -> f64 {
        match self.kind {
            ModKind::Fm { deviation_hz } => deviation_hz + MOD_TONE_HZ,
            ModKind::Am { .. } => MOD_TONE_HZ,
        }
    }
}

/// Baseband IQ synthesis state: continuous-phase tone oscillators, the modulated test
/// carriers, and a noise source. Phases are `f64` so they stay accurate across long runs;
/// output is cast to `f32` at the edge.
struct Generator {
    /// (offset as fraction of fs, amplitude, phase) for the static tones.
    tones: Vec<(f64, f64, f64)>,
    /// Drift tone amplitude; 0.0 in generators that carry no drift tone.
    drift_amp: f64,
    /// Drifting tone phase and LFO phase for its slow frequency sweep.
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

    /// The baseband one rx stream of a multi-stream test radio produces: the shared noise
    /// floor plus one NFM marker at [`stream_marker_offset_hz`]. NFM because a bare tone
    /// demodulates to silence — the marker exists so a test can demodulate stream k's 1 kHz
    /// tone, not merely see energy at the offset. The seed varies per stream, so no two
    /// streams are ever sample-identical.
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

    /// Move the marker carrier of a [`Generator::stream_marker`] (its only carrier) to a new
    /// baseband offset. Phase accumulates across the change, as a real synthesizer's would —
    /// a reset would put an undemodulatable click at every retune.
    fn set_marker_offset_hz(&mut self, offset_hz: f64) {
        for carrier in &mut self.carriers {
            carrier.offset_hz = offset_hz;
        }
    }

    /// Fill `block` with one block of IQ at `sample_rate`. Every oscillator accumulates
    /// phase per sample and carries it across calls — FM with phase resets at block edges
    /// would be undemodulatable — and nothing here allocates, keeping the capture thread
    /// real-time.
    fn fill(&mut self, block: &mut [Complex<f32>], sample_rate: f64) {
        use std::f64::consts::TAU;

        let hz_to_w = TAU / sample_rate;
        // Drift tone: offset sweeps as a slow LFO across ±0.35·fs at ~0.08 Hz.
        let lfo_w = 0.08 * hz_to_w;
        let noise_amp = 0.012;
        let mod_w = MOD_TONE_HZ * hz_to_w;

        // A carrier whose occupied band would cross Nyquist is muted rather than allowed to
        // alias bogus energy into the spectrum at low sample rates.
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

        // Keep phases bounded.
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

/// Small deterministic PRNG (xorshift64*) — avoids a `rand` dependency and keeps the noise
/// floor reproducible for tests.
struct Xorshift(u64);

impl Xorshift {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    /// Uniform value in roughly `[-1.0, 1.0)`.
    fn next_bipolar(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        let bits = (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f64; // 24 bits
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

    /// 2.4 Msps keeps the fraction-of-fs tones (+360 kHz, +120 kHz, -720 kHz) and the drift
    /// sweep (below ~190 kHz for the first half second) clear of every carrier band under test.
    const SPECTRUM_FS: f64 = 2_400_000.0;
    const SPECTRUM_LEN: usize = 1 << 20; // ~437 ms

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
                // Hann window keeps tone leakage out of the quiet reference bands.
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

        // β = 75 WFM spreads energy across roughly ±(75 + 1) kHz (Carson), so a ±10 kHz
        // slice holds a minority of it; NFM (±3.5 kHz) and AM (±1 kHz) fit inside ±10 kHz.
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
        // 999 is deliberately misaligned with the 1 kHz modulator period so a phase reset
        // at block edges cannot hide.
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
        // 600 kHz + Carson half-width exceeds Nyquist (512 kHz); folding would land the WFM
        // carrier at -424 kHz.
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

    #[test]
    fn application_build_policy_matches_the_profile() {
        let d = VirtualDriver::for_build(None);
        let infos = d.probe();
        assert_eq!(
            infos.iter().any(|info| info.id() == "virtual:siggen"),
            cfg!(debug_assertions)
        );
        if !cfg!(debug_assertions) {
            assert!(matches!(
                d.open(&VirtualDriver::siggen_info()),
                Err(DeviceError::NotFound(id)) if id == "virtual:siggen"
            ));
        }
    }

    #[test]
    fn probe_lists_finalized_recordings() {
        let dir = tempfile::TempDir::new().unwrap();
        let stem = dir.path().join("capture");
        let mut writer =
            sdrmm_recorder::SigmfWriter::create(&stem, 250_000.0, 100_000_000.0, "test").unwrap();
        writer.write_block(&[Complex::new(0.5, -0.5)]).unwrap();
        writer.finalize().unwrap();
        // Crashed recording: only the .tmp breadcrumb exists; it must not be listed.
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
        // The probed info must be openable — the registry's open path re-probes and matches it.
        d.open(recording).unwrap();

        // Production keeps the recording path but neither advertises nor opens a synthetic id.
        let production = VirtualDriver::configured(Some(dir.path().to_path_buf()), false);
        let production_infos = production.probe();
        assert_eq!(production_infos.len(), 1);
        assert_eq!(production_infos[0].id(), recording.id());
        production.open(&production_infos[0]).unwrap();
        assert!(matches!(
            production.open(&VirtualDriver::siggen_info()),
            Err(DeviceError::NotFound(id)) if id == "virtual:siggen"
        ));

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
            // The profile must be there at probe time: the picker filters templates against
            // it without opening the radio, exactly as it does for the siggen.
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
            // Same tuning surface as the siggen, so templates that match one match the other.
            assert_eq!(caps.sample_rates, siggen_capabilities().sample_rates);
            assert_eq!(caps.freq_ranges, siggen_capabilities().freq_ranges);
        }
    }

    #[test]
    fn marker_rx_start_requires_one_sink_per_stream() {
        let mut dev = open_virtual("array4");
        for wrong in [0usize, 1, 3, 5] {
            let sinks = (0..wrong).map(|_| RxSink::new(|_| {})).collect();
            match dev.rx_start(sinks) {
                Err(DeviceError::Unsupported(message)) => {
                    assert!(message.contains('4'), "{message}");
                }
                other => panic!("{wrong} sinks must be Unsupported, got {other:?}"),
            }
        }
        dev.rx_start((0..4).map(|_| RxSink::new(|_| {})).collect())
            .unwrap();
        dev.rx_stop();
        // A refused start must not have consumed the worker slot.
        dev.rx_start((0..4).map(|_| RxSink::new(|_| {})).collect())
            .unwrap();
        dev.rx_stop();
    }

    #[test]
    fn siggen_refuses_more_than_one_sink() {
        let mut dev = SigGen::new();
        let sinks = vec![RxSink::new(|_| {}), RxSink::new(|_| {})];
        assert!(matches!(
            dev.rx_start(sinks),
            Err(DeviceError::Unsupported(_))
        ));
        // …and still starts with the one sink it has a stream for.
        dev.rx_start(vec![RxSink::new(|_| {})]).unwrap();
        dev.rx_stop();
    }

    #[test]
    fn each_stream_carries_its_own_marker() {
        const STREAMS: usize = 4;
        const N: usize = 1 << 17; // ~64 ms at the default 2.048 Msps

        let mut dev = open_virtual("array4");
        let mut receivers = Vec::new();
        let sinks = (0..STREAMS)
            .map(|_| {
                let (tx, rx) = mpsc::channel::<Vec<Complex<f32>>>();
                receivers.push(rx);
                RxSink::new(move |s| {
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
        // The refused entry must not leak into reported settings.
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
                // Tuning one lane of a coherent array apart is what the array must never do.
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

    /// Where tuning is per-stream, each lane's marker follows *that lane's* centre: the marker
    /// radiates at the radio-wide centre + its offset, so a lane tuned apart sees it displaced
    /// by the difference, and a lane left alone does not.
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
                RxSink::new(move |s| {
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

        // Lane 0 was not retuned: its marker stays at its own offset, not RETUNE_HZ above it.
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

        // Lane 1 tuned RETUNE_HZ below the radio: its marker appears RETUNE_HZ higher in its
        // baseband, and is gone from where it used to be.
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

    /// Edge case: a radio-wide retune stays the default for lanes without an override and must
    /// not wipe overrides that exist — the override keeps displacing that lane's marker.
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
        // Lane 0 rides the radio-wide dial (offset unchanged); lane 1 keeps its override, now
        // 75 kHz below the radio, so its marker sits that much higher.
        assert_eq!(marker_offsets(&settings, &caps), vec![50_000.0, 175_000.0]);

        // Shared tuning: the same override table moves nothing, whatever it says.
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
        dev.rx_start(vec![RxSink::new(move |s| {
            let _ = tx.send(s.to_vec());
        })])
        .unwrap();
        let mut streamed = Vec::new();
        while streamed.len() < n {
            streamed.extend(rx.recv_timeout(Duration::from_secs(2)).unwrap());
        }
        dev.rx_stop();

        // Same tolerance as the block-size test: phase renormalization at block edges keeps
        // chunked output equal only to ~1e-4, not bit-exact.
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
        // The rejected tunes must not have leaked into settings.
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
        // Defaults untouched by the delta must survive.
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
        dev.rx_start(vec![RxSink::new(move |s| {
            let _ = tx.send(s.len());
        })])
        .unwrap();

        // Collect a few blocks, then stop.
        let mut total = 0;
        for _ in 0..3 {
            if let Ok(n) = rx.recv_timeout(Duration::from_secs(2)) {
                total += n;
            }
        }
        dev.rx_stop();
        assert!(total > 0, "expected streamed samples");

        // Second start after stop must succeed (not stuck AlreadyStreaming).
        dev.rx_start(vec![RxSink::new(|_| {})]).unwrap();
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
