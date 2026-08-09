//! `sdrmm-device-virtual` — always-on backend (PLAN §6) providing a signal generator and
//! SigMF file playback (PLAN §3: playback lives here, SigMF IO in `sdrmm-recorder`). This is
//! how CI, demo mode, and decoder golden tests run without hardware. The siggen synthesizes a
//! baseband IQ stream: a few fixed tones, one slowly drifting tone, and a white-noise floor
//! (the M0 spectrum path), plus NFM/AM/WFM carriers modulated by a 1 kHz tone that the M2
//! engine e2e tests demodulate. The plain tones sit at fixed fractions of the sample rate;
//! the modulated carriers sit at fixed Hz offsets from center — both ride along when the
//! device retunes, but Hz offsets let channels address the carriers at any sample rate.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;
use num_complex::Complex;
use sdrmm_device::{DeviceDriver, DeviceError, RxSink, SdrDevice, Worker};
use sdrmm_recorder::scan_stems;
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings, Range};

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

/// Comparable to the 0.10–0.30 static tones so the carriers get a similar SNR.
const MOD_CARRIER_AMP: f64 = 0.20;

/// Driver that exposes the virtual devices: the signal generator always, plus one playback
/// device per finalized SigMF recording when constructed with a recordings dir.
#[derive(Default)]
pub struct VirtualDriver {
    recordings_dir: Option<PathBuf>,
}

impl VirtualDriver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_recordings(dir: PathBuf) -> Self {
        Self {
            recordings_dir: Some(dir),
        }
    }

    fn siggen_info() -> DeviceInfo {
        DeviceInfo {
            driver: DRIVER_ID.to_string(),
            key: SIGGEN_KEY.to_string(),
            label: "Signal Generator (virtual)".to_string(),
            serial: None,
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
        })
    }
}

impl DeviceDriver for VirtualDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        let mut infos = vec![Self::siggen_info()];
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
        match info.key.as_str() {
            SIGGEN_KEY => Ok(Box::new(SigGen::new())),
            other => Err(DeviceError::NotFound(format!("{DRIVER_ID}:{other}"))),
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

impl SigGen {
    #[must_use]
    pub fn new() -> Self {
        let capabilities = Capabilities {
            freq_ranges: vec![Range {
                min: 0.0,
                max: 6_000_000_000.0,
                step: None,
            }],
            sample_rates: vec![
                250_000.0,
                1_024_000.0,
                2_048_000.0,
                2_400_000.0,
                3_200_000.0,
            ],
            sample_rate_range: None,
            gains: Vec::new(),
            antennas: vec!["RX".to_string()],
            bandwidths: Vec::new(),
            extra: Vec::new(),
            tx_capable: false,
        };
        let settings = DeviceSettings {
            center_hz: Some(100_000_000.0),
            sample_rate: Some(2_048_000.0),
            ..DeviceSettings::default()
        };
        let shared = Arc::new(ArcSwap::from_pointee(SigParams {
            sample_rate: settings.sample_rate.unwrap_or(2_048_000.0),
        }));
        Self {
            capabilities,
            settings,
            shared,
            worker: Worker::new(),
        }
    }

    fn sample_rate(&self) -> f64 {
        self.settings.sample_rate.unwrap_or(2_048_000.0)
    }
}

impl SdrDevice for SigGen {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        if let Some(rate) = settings.sample_rate
            && (rate <= 0.0 || !self.capabilities.sample_rates.contains(&rate))
        {
            return Err(DeviceError::Unsupported(format!("sample_rate {rate}")));
        }
        // The advertised freq_ranges are a contract: accepting a tune outside them would
        // make this backend an unfaithful double for hardware that rejects it (device-soapy
        // pre-flights the same check), hiding the reject path from every engine/server test.
        if let Some(f) = settings.center_hz
            && !self
                .capabilities
                .freq_ranges
                .iter()
                .any(|r| r.min <= f && f <= r.max)
        {
            return Err(DeviceError::Unsupported(format!(
                "center_hz {f} outside tuner range"
            )));
        }
        // Store every field via the one shared merge (`wire`), so PATCHed values round-trip
        // into state even for knobs the siggen has no behavior for (ppm, gains, bandwidth…).
        self.settings.merge_from(settings);
        // Publish the derived params so a live capture thread picks them up next block.
        self.shared.store(Arc::new(SigParams {
            sample_rate: self.sample_rate(),
        }));
        Ok(())
    }

    fn rx_start(&mut self, mut sink: RxSink) -> Result<(), DeviceError> {
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

/// A modulated test carrier at a fixed Hz offset from center (unlike the fraction-of-fs
/// tones, so channels can address it identically at any sample rate).
struct ModCarrier {
    offset_hz: f64,
    amp: f64,
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
    /// Drifting tone phase and LFO phase for its slow frequency sweep.
    drift_phase: f64,
    lfo_phase: f64,
    carriers: [ModCarrier; 3],
    noise: Xorshift,
}

impl Generator {
    fn new() -> Self {
        Self {
            tones: vec![(0.15, 0.30, 0.0), (-0.30, 0.16, 0.0), (0.05, 0.10, 0.0)],
            drift_phase: 0.0,
            lfo_phase: 0.0,
            carriers: [
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
            noise: Xorshift::new(0x5DEE_CE66_D00D_1234),
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
        let drift_amp = 0.20;
        let noise_amp = 0.012;
        let mod_w = MOD_TONE_HZ * hz_to_w;

        // A carrier whose occupied band would cross Nyquist is muted rather than allowed to
        // alias bogus energy into the spectrum at low sample rates.
        let nyquist = 0.5 * sample_rate;
        let mut carrier_amps = [0.0f64; 3];
        for (amp, carrier) in carrier_amps.iter_mut().zip(&self.carriers) {
            if carrier.offset_hz.abs() + carrier.occupied_half_width_hz() <= nyquist {
                *amp = carrier.amp;
            }
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

            self.drift_phase += TAU * 0.35 * self.lfo_phase.sin();
            self.lfo_phase += lfo_w;
            re += drift_amp * self.drift_phase.cos();
            im += drift_amp * self.drift_phase.sin();

            for (carrier, &amp) in self.carriers.iter_mut().zip(&carrier_amps) {
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
                re += amp * envelope * carrier.carrier_phase.cos();
                im += amp * envelope * carrier.carrier_phase.sin();
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

    use super::*;

    /// 2.4 Msps keeps the fraction-of-fs tones (+360 kHz, +120 kHz, -720 kHz) and the drift
    /// sweep (below ~190 kHz for the first half second) clear of every carrier band under test.
    const SPECTRUM_FS: f64 = 2_400_000.0;
    const SPECTRUM_LEN: usize = 1 << 20; // ~437 ms

    fn power_spectrum(n: usize, sample_rate: f64) -> Vec<f64> {
        let mut generator = Generator::new();
        let mut block = vec![Complex::new(0.0f32, 0.0f32); n];
        generator.fill(&mut block, sample_rate);

        let mut buf: Vec<Complex<f64>> = block
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
    fn probe_lists_siggen() {
        let d = VirtualDriver::new();
        let infos = d.probe();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].id(), "virtual:siggen");
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
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].id(), "virtual:siggen");
        assert_eq!(infos[1].id(), format!("virtual:file:{}", stem.display()));
        assert_eq!(infos[1].label, "capture (recording)");
        assert!(infos[1].serial.is_none());
        // The probed info must be openable — the registry's open path re-probes and matches it.
        d.open(&infos[1]).unwrap();

        assert!(matches!(
            d.open(&DeviceInfo {
                driver: "virtual".to_string(),
                key: format!("file:{}", dir.path().join("crashed").display()),
                label: String::new(),
                serial: None,
            }),
            Err(DeviceError::NotFound(_))
        ));
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
        dev.rx_start(RxSink::new(move |s| {
            let _ = tx.send(s.to_vec());
        }))
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
            gains: vec![sdrmm_wire::GainValue {
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
        dev.rx_start(RxSink::new(move |s| {
            let _ = tx.send(s.len());
        }))
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
        dev.rx_start(RxSink::new(|_| {})).unwrap();
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
