//! `sdrmm-device-virtual` — always-on backend (PLAN §6) providing a signal generator. This is
//! how CI, demo mode, and decoder golden tests run without hardware. The siggen synthesizes a
//! baseband IQ stream — a few fixed tones, one slowly drifting tone, and a white-noise floor —
//! so the full device → ring → spectrum → waterfall path has something to show at M0.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;
use num_complex::Complex;
use sdrmm_device::{DeviceDriver, DeviceError, RxSink, SdrDevice};
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings, Range};

const DRIVER_ID: &str = "virtual";
const SIGGEN_KEY: &str = "siggen";
/// Target block duration; the capture thread paces itself to roughly real time.
const BLOCK_SECS: f64 = 0.025;

/// Driver that exposes the virtual devices.
#[derive(Default)]
pub struct VirtualDriver;

impl VirtualDriver {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    fn siggen_info() -> DeviceInfo {
        DeviceInfo {
            driver: DRIVER_ID.to_string(),
            key: SIGGEN_KEY.to_string(),
            label: "Signal Generator (virtual)".to_string(),
            serial: None,
        }
    }
}

impl DeviceDriver for VirtualDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![Self::siggen_info()]
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        match info.key.as_str() {
            SIGGEN_KEY => Ok(Box::new(SigGen::new())),
            other => Err(DeviceError::NotFound(format!("{DRIVER_ID}:{other}"))),
        }
    }
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
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
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
            running: Arc::new(AtomicBool::new(false)),
            worker: None,
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
        if self.running.load(Ordering::Acquire) {
            return Err(DeviceError::AlreadyStreaming);
        }
        self.running.store(true, Ordering::Release);
        let running = self.running.clone();
        let shared = self.shared.clone();

        self.worker = Some(std::thread::spawn(move || {
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
        }));
        Ok(())
    }

    fn rx_stop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for SigGen {
    fn drop(&mut self) {
        self.rx_stop();
    }
}

/// Baseband IQ synthesis state: continuous-phase tone oscillators + a noise source. Phases are
/// `f64` so they stay accurate across long runs; output is cast to `f32` at the edge.
struct Generator {
    /// (offset as fraction of fs, amplitude, phase) for the static tones.
    tones: Vec<(f64, f64, f64)>,
    /// Drifting tone phase and LFO phase for its slow frequency sweep.
    drift_phase: f64,
    lfo_phase: f64,
    noise: Xorshift,
    elapsed: f64,
}

impl Generator {
    fn new() -> Self {
        Self {
            tones: vec![(0.15, 0.30, 0.0), (-0.30, 0.16, 0.0), (0.05, 0.10, 0.0)],
            drift_phase: 0.0,
            lfo_phase: 0.0,
            noise: Xorshift::new(0x5DEE_CE66_D00D_1234),
            elapsed: 0.0,
        }
    }

    /// Fill `block` with one block of IQ at `sample_rate`. Uses incremental phasor rotation
    /// (no per-sample transcendentals) and a cheap white-noise floor.
    fn fill(&mut self, block: &mut [Complex<f32>], sample_rate: f64) {
        use std::f64::consts::TAU;
        let n = block.len();

        // Drift tone: offset sweeps as a slow LFO across ±0.35·fs at ~0.08 Hz.
        let lfo_hz = 0.08;
        let drift_frac = 0.35 * (self.lfo_phase).sin();
        let drift_w = TAU * drift_frac; // rad/sample (offset already in fs fractions)
        let drift_amp = 0.20;

        let noise_amp = 0.012;

        for slot in block.iter_mut() {
            let mut re = 0.0f64;
            let mut im = 0.0f64;
            for (frac, amp, phase) in &mut self.tones {
                let w = TAU * *frac;
                *phase += w;
                re += *amp * phase.cos();
                im += *amp * phase.sin();
            }
            self.drift_phase += drift_w;
            re += drift_amp * self.drift_phase.cos();
            im += drift_amp * self.drift_phase.sin();

            re += noise_amp * self.noise.next_bipolar();
            im += noise_amp * self.noise.next_bipolar();

            *slot = Complex::new(re as f32, im as f32);
            self.lfo_phase += TAU * lfo_hz / sample_rate;
        }

        // Keep phases bounded.
        for (_, _, phase) in &mut self.tones {
            *phase = phase.rem_euclid(TAU);
        }
        self.drift_phase = self.drift_phase.rem_euclid(TAU);
        self.lfo_phase = self.lfo_phase.rem_euclid(TAU);
        self.elapsed += n as f64 / sample_rate;
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
    use std::{sync::mpsc, time::Duration};

    use super::*;

    #[test]
    fn probe_lists_siggen() {
        let d = VirtualDriver::new();
        let infos = d.probe();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].id(), "virtual:siggen");
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
