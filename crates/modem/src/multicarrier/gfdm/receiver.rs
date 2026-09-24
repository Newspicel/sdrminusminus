use std::f64::consts::TAU;

use num_complex::Complex;

use super::{
    GfdmDemod, GfdmDetector, GfdmParams,
    preamble::{GfdmAcquisition, GfdmSync},
};
use crate::{
    constellation::Constellation,
    framesync::{derotate, derotate_from},
    multicarrier::transform::Dft,
    ofdm::MIN_NOISE_VAR,
};

pub const LEAK_TAPS: usize = 2;
pub const TAP_SIGNIFICANCE: f32 = 9.0;
pub const SEGMENTS: usize = 8;
pub const PULL_IN_PASSES: usize = 1;
pub const FREQ_GAIN: f64 = 1.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CarrierTracker {
    phase: f64,
    cfo: f64,
}

impl CarrierTracker {
    #[must_use]
    pub fn new(cfo: f64, first_sample: usize) -> Self {
        Self {
            phase: (TAU * cfo * first_sample as f64).rem_euclid(TAU),
            cfo,
        }
    }

    #[must_use]
    pub fn phase(&self) -> f64 {
        self.phase
    }

    #[must_use]
    pub fn cfo(&self) -> f64 {
        self.cfo
    }

    pub fn correct(
        &mut self,
        points: &mut [Complex<f32>],
        table: &Constellation,
        window: usize,
        span: usize,
    ) -> f64 {
        let error = decision_phase(points, table);
        rotate(points, -error);
        let centre = window as f64 / 2.0;
        let span = span as f64;
        let before = self.cfo;
        self.cfo += FREQ_GAIN * error / (TAU * span);
        let advance = TAU * (before * centre + self.cfo * (span - centre));
        self.phase = (self.phase + error + advance).rem_euclid(TAU);
        error
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct PhaseLine {
    last: Option<f64>,
    unwrapped: f64,
    sw: f64,
    sx: f64,
    sy: f64,
    sxx: f64,
    sxy: f64,
}

impl PhaseLine {
    fn push(&mut self, x: f64, value: Complex<f64>) {
        let angle = value.arg();
        self.unwrapped = match self.last {
            Some(last) => self.unwrapped + wrap(angle - last),
            None => angle,
        };
        self.last = Some(angle);
        let w = value.norm();
        self.sw += w;
        self.sx += w * x;
        self.sy += w * self.unwrapped;
        self.sxx += w * x * x;
        self.sxy += w * x * self.unwrapped;
    }

    fn slope(&self) -> f64 {
        let det = self.sw * self.sxx - self.sx * self.sx;
        if det.abs() <= f64::EPSILON {
            0.0
        } else {
            (self.sw * self.sxy - self.sx * self.sy) / det
        }
    }
}

fn wrap(rad: f64) -> f64 {
    (rad + std::f64::consts::PI).rem_euclid(TAU) - std::f64::consts::PI
}

fn rotate(points: &mut [Complex<f32>], rad: f64) {
    let turn = Complex::from_polar(1.0f32, rad as f32);
    for p in points {
        *p *= turn;
    }
}

fn decision_phase(points: &[Complex<f32>], table: &Constellation) -> f64 {
    let mut acc = Complex::new(0.0f64, 0.0);
    for &p in points {
        let d = table.nearest(p);
        let v = p * d.conj();
        acc += Complex::new(f64::from(v.re), f64::from(v.im));
    }
    acc.arg()
}

#[derive(Clone)]
pub struct GfdmReceiver {
    params: GfdmParams,
    sync: GfdmSync,
    demod: GfdmDemod,
    table: Constellation,
    full: Dft,
    half: Dft,
    channel: Vec<Complex<f32>>,
    weights: Vec<Complex<f32>>,
    taps: Vec<Complex<f32>>,
    window: Vec<Complex<f32>>,
    points: Vec<Complex<f32>>,
    noise_var: f64,
    acquisition: Option<GfdmAcquisition>,
    tracker: CarrierTracker,
}

impl GfdmReceiver {
    #[must_use]
    pub fn new(params: GfdmParams, detector: GfdmDetector, table: Constellation) -> Self {
        let n = params.block();
        let sync = GfdmSync::new(&params);
        let half = sync.preamble().half();
        Self {
            sync,
            demod: GfdmDemod::new(params, detector),
            table,
            full: Dft::new(n),
            half: Dft::new(half),
            channel: vec![Complex::new(1.0, 0.0); n],
            weights: vec![Complex::new(1.0, 0.0); n],
            taps: vec![Complex::new(0.0, 0.0); half],
            window: vec![Complex::new(0.0, 0.0); n],
            points: vec![Complex::new(0.0, 0.0); n],
            noise_var: MIN_NOISE_VAR,
            acquisition: None,
            tracker: CarrierTracker::default(),
            params,
        }
    }

    #[must_use]
    pub fn with_sync(mut self, sync: GfdmSync) -> Self {
        self.sync = sync;
        self
    }

    #[must_use]
    pub fn params(&self) -> &GfdmParams {
        &self.params
    }

    #[must_use]
    pub fn acquisition(&self) -> Option<GfdmAcquisition> {
        self.acquisition
    }

    #[must_use]
    pub fn channel(&self) -> &[Complex<f32>] {
        &self.channel
    }

    #[must_use]
    pub fn noise_var(&self) -> f64 {
        self.noise_var
    }

    #[must_use]
    pub fn cfo(&self) -> f64 {
        self.tracker.cfo()
    }

    pub fn acquire(&mut self, x: &[Complex<f32>], search: usize) -> Option<GfdmAcquisition> {
        self.acquisition = self.sync.acquire(x, search);
        let mut acquisition = self.acquisition?;
        self.estimate(x, acquisition.preamble_start, acquisition.cfo);
        acquisition.cfo += self.residual_cfo(x, acquisition.preamble_start, acquisition.cfo);
        self.estimate(x, acquisition.preamble_start, acquisition.cfo);
        self.mmse_weights();
        self.tracker =
            CarrierTracker::new(acquisition.cfo, acquisition.data_start + self.params.cp);
        self.acquisition = Some(acquisition);
        Some(acquisition)
    }

    pub fn demodulate(
        &mut self,
        x: &[Complex<f32>],
        blocks: usize,
        out: &mut Vec<Complex<f32>>,
    ) -> usize {
        let Some(acquisition) = self.acquisition else {
            return 0;
        };
        let readable = self.readable_blocks(x, acquisition, blocks);
        self.pull_in(x, acquisition, readable);
        for block in 0..readable {
            self.read_block(x, self.block_start(acquisition, block));
            out.extend_from_slice(&self.points);
        }
        readable
    }

    fn readable_blocks(
        &self,
        x: &[Complex<f32>],
        acquisition: GfdmAcquisition,
        blocks: usize,
    ) -> usize {
        (0..blocks)
            .take_while(|&block| {
                self.block_start(acquisition, block) + self.params.block() <= x.len()
            })
            .count()
    }

    fn block_start(&self, acquisition: GfdmAcquisition, block: usize) -> usize {
        acquisition.data_start + block * self.params.samples() + self.params.cp
    }

    fn pull_in(&mut self, x: &[Complex<f32>], acquisition: GfdmAcquisition, blocks: usize) {
        if blocks < 2 {
            return;
        }
        for _ in 0..PULL_IN_PASSES {
            for block in 0..blocks {
                self.read_block(x, self.block_start(acquisition, block));
            }
            let cfo = self.tracker.cfo();
            self.estimate(x, acquisition.preamble_start, cfo);
            self.mmse_weights();
            self.tracker = CarrierTracker::new(cfo, self.block_start(acquisition, 0));
        }
    }

    fn read_block(&mut self, x: &[Complex<f32>], start: usize) {
        derotate_from(
            x,
            start,
            self.tracker.cfo(),
            self.tracker.phase(),
            &mut self.window,
        );
        self.full.forward(&mut self.window);
        for (y, &w) in self.window.iter_mut().zip(&self.weights) {
            *y *= w;
        }
        self.full.inverse(&mut self.window);
        self.demod.detect_block(&self.window, &mut self.points);
        self.tracker.correct(
            &mut self.points,
            &self.table,
            self.params.block(),
            self.params.samples(),
        );
    }

    fn estimate(&mut self, x: &[Complex<f32>], start: usize, cfo: f64) {
        derotate(x, start, cfo, &mut self.window);
        self.full.forward(&mut self.window);
        self.noise_var = self.odd_bin_power().max(MIN_NOISE_VAR);
        self.sample_even_bins();
        self.half.inverse(&mut self.taps);
        self.spread_taps();
        self.full.forward(&mut self.channel);
        let scale = std::f32::consts::SQRT_2;
        for h in &mut self.channel {
            *h *= scale;
        }
    }

    fn residual_cfo(&mut self, x: &[Complex<f32>], start: usize, cfo: f64) -> f64 {
        self.expected_preamble();
        derotate(x, start, cfo, &mut self.window);
        let segment = self.window.len() / SEGMENTS;
        if segment == 0 {
            return 0.0;
        }
        let mut fit = PhaseLine::default();
        for (index, (got, want)) in self
            .window
            .chunks_exact(segment)
            .zip(self.points.chunks_exact(segment))
            .enumerate()
        {
            let sum: Complex<f32> = got.iter().zip(want).map(|(y, r)| y * r.conj()).sum();
            fit.push(
                index as f64,
                Complex::new(f64::from(sum.re), f64::from(sum.im)),
            );
        }
        fit.slope() / (TAU * segment as f64)
    }

    fn expected_preamble(&mut self) {
        let spectrum = self.sync.preamble().spectrum();
        let scale = std::f32::consts::SQRT_2;
        self.points.fill(Complex::new(0.0, 0.0));
        for (k, &p) in spectrum.iter().enumerate() {
            self.points[2 * k] = self.channel[2 * k] * p * scale;
        }
        self.full.inverse(&mut self.points);
    }

    fn odd_bin_power(&self) -> f64 {
        let odd = self.window.iter().skip(1).step_by(2);
        let count = self.window.len() / 2;
        odd.map(|y| f64::from(y.norm_sqr())).sum::<f64>() / count as f64
    }

    fn sample_even_bins(&mut self) {
        let spectrum = self.sync.preamble().spectrum();
        let scale = std::f32::consts::FRAC_1_SQRT_2;
        for (k, (slot, &p)) in self.taps.iter_mut().zip(spectrum).enumerate() {
            *slot = self.window[2 * k] * p.conj() * scale;
        }
    }

    fn spread_taps(&mut self) {
        let (n, half, cp) = (self.channel.len(), self.taps.len(), self.params.cp);
        self.channel.fill(Complex::new(0.0, 0.0));
        let floor = TAP_SIGNIFICANCE * self.noise_var as f32 / 2.0;
        let significant = |g: Complex<f32>| {
            if g.norm_sqr() > floor {
                g
            } else {
                Complex::new(0.0, 0.0)
            }
        };
        let keep = (cp + 1).min(half);
        for l in 0..keep {
            self.channel[l] = significant(self.taps[l]);
        }
        for j in 1..=LEAK_TAPS.min(half - keep) {
            self.channel[n - j] = significant(self.taps[half - j]);
        }
    }

    fn mmse_weights(&mut self) {
        let nv = self.noise_var as f32;
        let mut bias = 0.0f64;
        for (w, h) in self.weights.iter_mut().zip(&self.channel) {
            let gain = h.norm_sqr();
            *w = h.conj() / (gain + nv);
            bias += f64::from(gain / (gain + nv));
        }
        let bias = (bias / self.weights.len() as f64).max(f64::MIN_POSITIVE) as f32;
        for w in &mut self.weights {
            *w /= bias;
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_modem_test_support::ber::{
        impair::{Awgn, Cfo, Impairment, Multipath, MultipathProfile},
        perf::assert_no_alloc,
        rng::Rng,
    };

    use super::*;
    use crate::{constellation::tables, multicarrier::GfdmMod};

    const BLOCKS: usize = 8;

    fn params() -> GfdmParams {
        let mut params = GfdmParams::new(16, 5, 0.5);
        params.cp = 8;
        params
    }

    fn qpsk() -> Constellation {
        tables::qam_square(4).unwrap()
    }

    fn payload(seed: u32) -> Vec<Complex<f32>> {
        let table = qpsk();
        let mut state = seed | 1;
        (0..BLOCKS * params().block())
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                table.points()[(state % 4) as usize]
            })
            .collect()
    }

    fn burst(lead: usize, seed: u32) -> (Vec<Complex<f32>>, Vec<Complex<f32>>) {
        let sent = payload(seed);
        let mut wave = vec![Complex::new(0.0, 0.0); lead];
        GfdmMod::new(params()).frame(&sent, &mut wave);
        wave.resize(wave.len() + 64, Complex::new(0.0, 0.0));
        (sent, wave)
    }

    fn receiver() -> GfdmReceiver {
        GfdmReceiver::new(params(), GfdmDetector::ZeroForcing, qpsk())
    }

    fn errors(sent: &[Complex<f32>], got: &[Complex<f32>]) -> usize {
        let table = qpsk();
        let wrong = sent
            .iter()
            .zip(got)
            .filter(|(a, b)| table.hard_slice(**a) != table.hard_slice(**b))
            .count();
        wrong + sent.len().saturating_sub(got.len())
    }

    fn decode(rx: &mut GfdmReceiver, wave: &[Complex<f32>]) -> Vec<Complex<f32>> {
        assert!(rx.acquire(wave, 200).is_some(), "no preamble found");
        let mut out = Vec::new();
        assert_eq!(rx.demodulate(wave, BLOCKS, &mut out), BLOCKS);
        out
    }

    #[test]
    fn a_clean_frame_is_found_and_decoded_at_any_lead() {
        for lead in [0usize, 1, 17, 90, 173] {
            let (sent, wave) = burst(lead, 0x61);
            let mut rx = receiver();
            let got = decode(&mut rx, &wave);
            assert_eq!(errors(&sent, &got), 0, "lead {lead}");
            let a = rx.acquisition().unwrap();
            let true_start = lead + params().cp;
            assert!(
                a.preamble_start <= true_start && true_start - a.preamble_start <= 2,
                "lead {lead}: window at {} for a block at {true_start}",
                a.preamble_start
            );
            assert!(a.cfo.abs() < 1e-6 && a.metric > 0.95);
        }
    }

    #[test]
    fn integer_and_fractional_carrier_offsets_are_removed() {
        let range = receiver().sync.cfo_range();
        assert!((range - 2.5 / 40.0).abs() < 1e-12, "range {range}");
        for cfo in [-0.06, -0.031, -0.004, 0.0021, 0.0124, 0.026, 0.059] {
            let (sent, mut wave) = burst(41, 0x62);
            Cfo::from_cycles_per_sample(cfo).apply(&mut wave, &mut Rng::new(0));
            let mut rx = receiver();
            let got = decode(&mut rx, &wave);
            assert_eq!(errors(&sent, &got), 0, "cfo {cfo}");
            let read = rx.acquisition().unwrap().cfo;
            assert!((read - cfo).abs() < 1e-6, "cfo {cfo} read {read}");
        }
    }

    #[test]
    fn echoes_inside_the_prefix_are_equalised() {
        for (delay, db, phase) in [(1usize, -3.0, 0.7), (4, -1.0, 2.2), (6, -6.0, -1.9)] {
            let (sent, mut wave) = burst(29, 0x63);
            Multipath::new(MultipathProfile::TwoRay {
                delay_samples: delay,
                relative_db: db,
                phase_rad: phase,
            })
            .apply(&mut wave, &mut Rng::new(0));
            let mut rx = receiver();
            let got = decode(&mut rx, &wave);
            assert_eq!(errors(&sent, &got), 0, "echo at {delay}");
        }
    }

    #[test]
    fn a_stronger_late_echo_does_not_pull_the_window_past_the_first_path() {
        let (sent, mut wave) = burst(33, 0x64);
        Multipath::new(MultipathProfile::TwoRay {
            delay_samples: 5,
            relative_db: 3.0,
            phase_rad: 0.4,
        })
        .apply(&mut wave, &mut Rng::new(0));
        let mut rx = receiver();
        let got = decode(&mut rx, &wave);
        assert_eq!(errors(&sent, &got), 0);
        assert!(rx.acquisition().unwrap().preamble_start <= 33 + params().cp);
    }

    #[test]
    fn a_carrier_offset_missed_by_acquisition_is_pulled_in() {
        for missed in [-4e-4, 2e-4, 4e-4] {
            let (sent, mut wave) = burst(12, 0x65);
            let mut rx = receiver();
            assert!(rx.acquire(&wave, 200).is_some());
            Cfo::from_cycles_per_sample(missed).apply(&mut wave, &mut Rng::new(0));
            let mut got = Vec::new();
            rx.demodulate(&wave, BLOCKS, &mut got);
            assert_eq!(errors(&sent, &got), 0, "missed {missed}");
            assert!((rx.cfo() - missed).abs() < 2e-6, "cfo {}", rx.cfo());
        }
    }

    #[test]
    fn the_noise_estimate_reads_the_channel_noise() {
        let (_, mut wave) = burst(20, 0x66);
        Awgn::with_sigma(0.3).apply(&mut wave, &mut Rng::new(0x66));
        let mut rx = receiver();
        assert!(rx.acquire(&wave, 200).is_some());
        let want = 2.0 * 0.3 * 0.3;
        assert!(
            (rx.noise_var() / want - 1.0).abs() < 0.35,
            "noise {} want {want}",
            rx.noise_var()
        );
    }

    #[test]
    fn frames_are_found_at_low_snr_and_noise_alone_is_rejected() {
        let mut found = 0;
        for trial in 0..50u64 {
            let (_, mut wave) = burst(37, 0x67);
            Awgn::with_sigma(0.7).apply(&mut wave, &mut Rng::new(0x670 + trial));
            let mut rx = receiver();
            if rx
                .acquire(&wave, 200)
                .is_some_and(|a| a.preamble_start.abs_diff(37 + params().cp) <= 2)
            {
                found += 1;
            }
        }
        assert_eq!(found, 50, "{found} of 50 frames found at 0 dB SNR");
        let mut false_alarms = 0;
        for trial in 0..50u64 {
            let mut noise = vec![Complex::new(0.0, 0.0); 800];
            Awgn::with_sigma(0.7).apply(&mut noise, &mut Rng::new(0x680 + trial));
            if receiver().acquire(&noise, 200).is_some() {
                false_alarms += 1;
            }
        }
        assert_eq!(false_alarms, 0, "noise alone raised {false_alarms} frames");
    }

    #[test]
    fn demodulation_does_not_allocate() {
        let (_, wave) = burst(5, 0x68);
        let mut rx = receiver();
        assert!(rx.acquire(&wave, 200).is_some());
        let mut out = Vec::with_capacity(BLOCKS * params().block());
        assert_no_alloc("gfdm demodulate", || {
            rx.demodulate(&wave, BLOCKS, &mut out);
        });
    }
}
