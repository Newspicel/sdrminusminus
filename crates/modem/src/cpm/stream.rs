use std::f64::consts::{FRAC_PI_2, TAU};

use num_complex::Complex;
use sdrmm_dsp::{Decimator, farrow};

use super::{
    costas::LaurentCostas,
    laurent::{LaurentError, laurent_main_pulse, polarity, whole_sps},
    lines::SquaredLines,
    lock::{FrequencyLock, LockDetector},
    params::CpmParams,
};

const POWER_SYMBOLS: f32 = 64.0;

const HANDOFF_PER_SYMBOL: f64 = 1.0 / 64.0;

const SQUARE_SYMBOLS: f32 = 8.0;

const FINE_ENTER: f32 = 0.3;

const FINE_LEAVE: f32 = 0.15;

const FINE_RANGE_RAD: f64 = 0.3;

const FINE_GAIN: f64 = 1.0 / 32.0;

const ACQUIRE_BW_RATIO: f64 = 4.0;

#[derive(Clone, Debug)]
struct SymbolClock {
    recent: [Complex<f32>; 4],
    newest: i64,
    next: f64,
    sps: f64,
    gain: f64,
}

impl SymbolClock {
    fn new(sps: f64, gain: f64) -> Self {
        Self {
            recent: [Complex::new(0.0, 0.0); 4],
            newest: -1,
            next: sps,
            sps,
            gain,
        }
    }

    fn push(&mut self, y: Complex<f32>) -> Option<Complex<f32>> {
        self.recent.rotate_left(1);
        self.recent[3] = y;
        self.newest += 1;
        let base = self.next.floor();
        if (base as i64) + 2 > self.newest {
            return None;
        }
        Some(farrow(&self.recent, (self.next - base) as f32))
    }

    fn steer(&mut self, epoch: Option<f64>) {
        let error = epoch.map_or(0.0, |target| {
            let gap = (target - self.next).rem_euclid(self.sps);
            if gap > self.sps / 2.0 {
                gap - self.sps
            } else {
                gap
            }
        });
        self.next += self.sps + self.gain * error;
    }

    fn reset(&mut self) {
        *self = Self::new(self.sps, self.gain);
    }
}

#[derive(Clone, Debug, Default)]
struct SquareLock {
    average: Complex<f32>,
    fine: bool,
}

impl SquareLock {
    fn push(&mut self, z: Complex<f32>, allowed: bool) -> Option<f64> {
        let before = self.average;
        self.average += (z * z - self.average) / SQUARE_SYMBOLS;
        let coherence = self.average.norm();
        self.fine = allowed
            && if self.fine {
                coherence > FINE_LEAVE
            } else {
                coherence > FINE_ENTER
            };
        self.fine.then(|| {
            (0.5 * f64::from((self.average * before.conj()).arg()))
                .clamp(-FINE_RANGE_RAD, FINE_RANGE_RAD)
        })
    }

    fn fine(&self) -> bool {
        self.fine
    }
}

fn neighbour_ratio(pulse: &[f32], sps: usize) -> f32 {
    let lag = |shift: usize| -> f32 {
        pulse
            .iter()
            .zip(pulse.iter().skip(shift))
            .map(|(a, b)| a * b)
            .sum()
    };
    let centre = lag(0);
    if centre <= f32::MIN_POSITIVE {
        return 0.0;
    }
    lag(sps) / centre
}

pub struct CoherentCpmStream {
    lock: FrequencyLock,
    matched: Decimator,
    lines: SquaredLines,
    clock: SymbolClock,
    square: SquareLock,
    carrier: LaurentCostas,
    detector: LockDetector,
    polarity: f32,
    sps: usize,
    power: f32,
    turn: u8,
    before: Complex<f32>,
    previous: Complex<f32>,
    chunk: Vec<Complex<f32>>,
    filtered: Vec<Complex<f32>>,
    locked: Vec<bool>,
}

impl CoherentCpmStream {
    pub fn new(params: &CpmParams, loop_bw: f64, timing_bw: f64) -> Result<Self, LaurentError> {
        let sps = whole_sps(params)?;
        let pulse = laurent_main_pulse(params)?;
        Ok(Self {
            lock: FrequencyLock::new(sps),
            matched: Decimator::new(&pulse, 1),
            lines: SquaredLines::new(sps),
            clock: SymbolClock::new(sps as f64, timing_bw),
            square: SquareLock::default(),
            carrier: LaurentCostas::new(
                ACQUIRE_BW_RATIO * loop_bw,
                loop_bw,
                neighbour_ratio(&pulse, sps),
            ),
            detector: LockDetector::default(),
            polarity: polarity(params),
            sps,
            power: 1.0,
            turn: 0,
            before: Complex::new(0.0, 0.0),
            previous: Complex::new(0.0, 0.0),
            chunk: Vec::new(),
            filtered: Vec::new(),
            locked: Vec::new(),
        })
    }

    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<f32>) {
        self.locked.clear();
        for piece in iq.chunks(self.sps) {
            self.lock.derotate(piece, &mut self.chunk);
            self.matched.process(&self.chunk, &mut self.filtered);
            self.lock.observe(&self.chunk, self.square.fine());
            for k in 0..self.filtered.len() {
                let y = self.filtered[k];
                self.lines.push(y);
                if let Some(symbol) = self.clock.push(y) {
                    self.clock.steer(self.lines.epoch_samples());
                    out.push(self.polarity * self.decide(symbol));
                    self.locked.push(self.detector.locked());
                }
            }
        }
    }

    fn decide(&mut self, symbol: Complex<f32>) -> f32 {
        let prepared = self.prepare(symbol);
        if let Some(rotation) = self.square.push(prepared, !self.lock.far()) {
            self.lock.shift(FINE_GAIN * rotation / self.sps as f64);
        }
        let z = self.carrier.advance(prepared, self.detector.locked());
        let centre = self.carrier.clean(self.previous, self.before, z);
        self.before = self.previous;
        if self.detector.push(centre) {
            let shed = self.carrier.shed_frequency(HANDOFF_PER_SYMBOL);
            self.lock.shift(TAU * shed / self.sps as f64);
        }
        let soft = z.re * self.previous.re;
        self.previous = z;
        soft
    }

    fn prepare(&mut self, symbol: Complex<f32>) -> Complex<f32> {
        self.power += (symbol.norm_sqr() - self.power) / POWER_SYMBOLS;
        let scale = self.power.max(f32::MIN_POSITIVE).sqrt().recip();
        let theta = -FRAC_PI_2 * f64::from(self.turn);
        self.turn = (self.turn + 1) % 4;
        symbol * scale * Complex::from_polar(1.0, theta as f32)
    }

    #[must_use]
    pub fn locked(&self) -> &[bool] {
        &self.locked
    }

    #[must_use]
    pub fn is_locked(&self) -> bool {
        self.detector.locked()
    }

    #[must_use]
    pub fn lock_quality(&self) -> f32 {
        self.detector.quality()
    }

    #[must_use]
    pub fn frequency_error_cycles_per_sample(&self) -> f64 {
        self.lock.freq_cycles_per_sample() + self.carrier.freq_cycles_per_symbol() / self.sps as f64
    }

    pub fn reset(&mut self) {
        self.lock.reset();
        self.matched.reset();
        self.lines.reset();
        self.clock.reset();
        self.square = SquareLock::default();
        self.carrier.reset();
        self.detector.reset();
        self.power = 1.0;
        self.turn = 0;
        self.before = Complex::new(0.0, 0.0);
        self.previous = Complex::new(0.0, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use sdrmm_dsp::design_lowpass;
    use sdrmm_modem_test_support::ber::{perf::assert_no_alloc, rng::Rng};

    use super::*;
    use crate::{
        cpm::{CpmDemod, CpmMod, Mapping, TIMING_BW_BURST},
        pulse::{self, Norm},
    };

    const SPS: f64 = 10.0;
    const RATE: f64 = 48_000.0;
    const LOOP_BW: f64 = 0.01;
    const TIMING_BW: f64 = 0.02;
    const SETTLE: usize = 500;

    fn gmsk() -> CpmParams {
        CpmParams::from_h(
            Mapping::natural(2),
            0.5,
            pulse::gaussian_freq(SPS, 0.5, 3, Norm::Area),
            SPS,
        )
    }

    fn symbols(n: usize, seed: u64) -> Vec<u8> {
        let mut rng = Rng::new(seed);
        (0..n).map(|_| (rng.next_u64() & 1) as u8).collect()
    }

    fn wave(sent: &[u8]) -> Vec<Complex<f32>> {
        let mut m = CpmMod::new(gmsk());
        let mut out = Vec::new();
        m.modulate(sent, &mut out);
        m.flush(&mut out);
        out
    }

    fn shift(w: &mut [Complex<f32>], hz: f64, hz_per_second: f64) {
        let mut phase = 0.0f64;
        for (k, s) in w.iter_mut().enumerate() {
            let f = (hz + hz_per_second * k as f64 / RATE) / RATE;
            phase = (phase + TAU * f) % TAU;
            *s *= Complex::new(phase.cos() as f32, phase.sin() as f32);
        }
    }

    fn noise(w: &mut [Complex<f32>], ebn0_db: f64, seed: u64) {
        let sigma = (SPS / 10f64.powf(ebn0_db / 10.0) / 2.0).sqrt();
        let mut rng = Rng::new(seed);
        for s in w {
            let (a, b) = rng.normal_pair();
            *s += Complex::new((a * sigma) as f32, (b * sigma) as f32);
        }
    }

    fn front(w: &[Complex<f32>]) -> Vec<Complex<f32>> {
        let mut out = Vec::new();
        Decimator::new(&design_lowpass(127, 6_000.0 / RATE), 1).process(w, &mut out);
        out
    }

    fn burst(sent: &[u8], hz: f64, ebn0_db: f64, seed: u64) -> Vec<Complex<f32>> {
        let mut w = vec![Complex::new(0.0, 0.0); 2_000];
        w.extend(wave(sent));
        shift(&mut w, hz, 0.0);
        noise(&mut w, ebn0_db, seed);
        front(&w)
    }

    fn errors(soft: &[f32], sent: &[u8], from: usize, to: usize) -> usize {
        (0..soft.len().saturating_sub(to))
            .map(|lag| {
                (from..to)
                    .filter(|&i| {
                        soft.get(lag + i)
                            .is_none_or(|&s| (s > 0.0) != (sent[i] == 1))
                    })
                    .count()
            })
            .min()
            .unwrap_or(usize::MAX)
    }

    fn stream(w: &[Complex<f32>]) -> (Vec<f32>, CoherentCpmStream) {
        let mut demod = CoherentCpmStream::new(&gmsk(), LOOP_BW, TIMING_BW).unwrap();
        let mut soft = Vec::new();
        for (k, piece) in w.chunks(997).enumerate() {
            let (a, b) = piece.split_at(piece.len().min(k % 7));
            demod.process(a, &mut soft);
            demod.process(b, &mut soft);
        }
        (soft, demod)
    }

    fn discriminator(w: &[Complex<f32>]) -> Vec<f32> {
        let mut demod = CpmDemod::new(
            &gmsk(),
            &pulse::gaussian(SPS, 0.5, 3, Norm::Area),
            TIMING_BW_BURST,
        );
        let mut soft = Vec::new();
        demod.process(w, &mut soft);
        soft
    }

    fn ber(demod: fn(&[Complex<f32>]) -> Vec<f32>, ebn0_db: f64) -> f64 {
        let (mut wrong, mut total) = (0, 0);
        for seed in 0..8 {
            let sent = symbols(4_000, seed);
            let soft = demod(&burst(&sent, 0.0, ebn0_db, seed + 50));
            wrong += errors(&soft, &sent, SETTLE, sent.len() - 16);
            total += sent.len() - 16 - SETTLE;
        }
        wrong as f64 / total as f64
    }

    #[test]
    fn only_binary_half_index_cpm_streams() {
        let four = CpmParams::from_h(Mapping::natural(4), 0.5, pulse::rect(SPS, Norm::Area), SPS);
        assert!(CoherentCpmStream::new(&four, LOOP_BW, TIMING_BW).is_err());
    }

    #[test]
    fn a_noiseless_stream_decodes_clean_in_ragged_chunks() {
        let sent = symbols(3_000, 1);
        let (soft, demod) = stream(&burst(&sent, 0.0, 90.0, 2));
        assert_eq!(errors(&soft, &sent, SETTLE, sent.len() - 16), 0);
        assert!(demod.is_locked());
        assert!(
            demod.lock_quality() > 0.9,
            "quality {}",
            demod.lock_quality()
        );
    }

    #[test]
    fn pulls_in_offsets_across_the_d_star_tolerance() {
        for hz in [-2_500.0, -1_200.0, 400.0, 1_800.0, 2_500.0] {
            let sent = symbols(4_000, 3);
            let (soft, demod) = stream(&burst(&sent, hz, 12.0, 4));
            let wrong = errors(&soft, &sent, 1_000, sent.len() - 16);
            assert_eq!(wrong, 0, "{hz} Hz");
            let read = demod.frequency_error_cycles_per_sample() * RATE;
            assert!((read - hz).abs() < 10.0, "{hz} Hz read as {read}");
        }
    }

    #[test]
    fn tracks_a_drifting_carrier() {
        let sent = symbols(9_600, 5);
        let mut w = vec![Complex::new(0.0, 0.0); 2_000];
        w.extend(wave(&sent));
        shift(&mut w, -600.0, 1_200.0);
        noise(&mut w, 10.0, 6);
        let (soft, _) = stream(&front(&w));
        let wrong = errors(&soft, &sent, SETTLE, sent.len() - 16);
        assert!(wrong <= 2, "{wrong} errors over a 1.2 kHz/s drift");
    }

    #[test]
    fn reacquires_a_new_transmission_after_silence() {
        let first = symbols(2_000, 7);
        let second = symbols(3_000, 8);
        let mut w = wave(&first);
        shift(&mut w, 900.0, 0.0);
        w.extend(vec![Complex::new(0.0, 0.0); 24_003]);
        let mut tail = wave(&second);
        shift(&mut tail, -1_500.0, 0.0);
        w.extend(tail);
        noise(&mut w, 11.0, 9);
        let (soft, _) = stream(&front(&w));
        let later = &soft[soft.len() - second.len() - 16..];
        assert_eq!(errors(later, &second, 1_000, second.len() - 16), 0);
    }

    #[test]
    fn beats_the_discriminator_by_more_than_three_db() {
        let coherent = ber(|w| stream(w).0, 7.0);
        let discriminator = ber(discriminator, 10.0);
        println!("coherent 7 dB {coherent:.2e}, discriminator 10 dB {discriminator:.2e}");
        assert!(coherent < 5e-3, "coherent BER {coherent} at 7 dB");
        assert!(coherent * 5.0 < discriminator);
    }

    #[test]
    fn a_warm_stream_does_not_allocate() {
        let sent = symbols(2_000, 11);
        let w = burst(&sent, 300.0, 12.0, 12);
        let mut demod = CoherentCpmStream::new(&gmsk(), LOOP_BW, TIMING_BW).unwrap();
        let mut soft = Vec::with_capacity(4 * sent.len());
        demod.process(&w, &mut soft);
        soft.clear();
        assert_no_alloc("CoherentCpmStream::process", || {
            demod.process(&w, &mut soft);
        });
        assert!(!soft.is_empty());
    }
}
