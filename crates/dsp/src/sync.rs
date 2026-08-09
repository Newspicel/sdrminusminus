//! Symbol timing recovery (PLAN §7): a Gardner detector driving a Farrow parabolic
//! interpolator for complex baseband, and a zero-crossing bit clock for real sliced baseband.

use std::f64::consts::{FRAC_1_SQRT_2, TAU};

use num_complex::Complex;

const ZERO: Complex<f32> = Complex::new(0.0, 0.0);

/// Curvature weight of the piecewise-parabolic Farrow interpolator (Erup/Gardner/Harris 1993);
/// 0.5 is their frequency-optimal choice at two or more samples per symbol.
const FARROW_CURVATURE: f32 = 0.5;

/// How far the integrator may pull the symbol period from nominal, as a fraction of it. Wide
/// enough for any transmitter crystal error, narrow enough that noise cannot drag the loop
/// onto a neighbouring multiple of the true clock.
const TRACKING_RANGE: f64 = 0.05;

/// Gardner timing-error detector with a parabolic (Farrow) interpolator, for complex baseband
/// at a nominal `sps` input samples per symbol (>= 2, may be fractional).
#[derive(Clone, Debug)]
pub struct SymbolSync {
    nominal_sps: f64,
    sps: f64,
    /// Interval to the next symbol instant, in input samples: `sps` plus this symbol's phase
    /// correction. Each half-symbol step uses exactly half of it, so the mid-point sample the
    /// detector needs sits halfway between two symbol instants even while the loop is pulling.
    step: f64,
    alpha: f64,
    beta: f64,
    /// Next interpolation instant, as an absolute input-sample index split into an integer
    /// part and a fraction in [0, 1). Keeping the fraction separate makes the accumulator
    /// depend only on the sequence of steps, never on where blocks happen to be cut.
    pos: usize,
    frac: f64,
    /// Absolute index of `buf[0]`.
    consumed: usize,
    buf: Vec<Complex<f32>>,
    /// The upcoming instant is a symbol instant rather than the mid-point between two.
    at_symbol: bool,
    prev_symbol: Complex<f32>,
    mid: Complex<f32>,
    /// A full (previous symbol, mid, symbol) triple is available.
    primed: bool,
}

impl SymbolSync {
    /// `loop_bw` is the normalised loop bandwidth in cycles per symbol (1e-3..0.05 is usual).
    #[must_use]
    pub fn new(sps: f64, loop_bw: f64) -> Self {
        assert!(
            sps.is_finite() && sps >= 2.0,
            "sps must be at least 2 samples per symbol"
        );
        assert!(
            loop_bw.is_finite() && loop_bw > 0.0 && loop_bw < 1.0,
            "loop_bw must be in (0, 1) cycles per symbol"
        );
        let denom = 1.0 + 2.0 * FRAC_1_SQRT_2 * loop_bw + loop_bw * loop_bw;
        let mut sync = Self {
            nominal_sps: sps,
            sps,
            step: sps,
            alpha: 4.0 * FRAC_1_SQRT_2 * loop_bw / denom,
            beta: 4.0 * loop_bw * loop_bw / denom,
            pos: 0,
            frac: 0.0,
            consumed: 0,
            buf: Vec::new(),
            at_symbol: true,
            prev_symbol: ZERO,
            mid: ZERO,
            primed: false,
        };
        sync.reset();
        sync
    }

    /// Feed a block of input; append one interpolated sample per recovered symbol to `out`.
    /// Timing state carries across calls, so any block split gives the same symbol stream.
    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        self.buf.extend_from_slice(input);
        // The interpolator reads one sample below and two above the instant.
        while self.pos + 3 <= self.consumed + self.buf.len() {
            let base = self.pos - self.consumed;
            let y = farrow(&self.buf[base - 1..base + 3], self.frac as f32);
            if self.at_symbol {
                if self.primed {
                    self.retime(y);
                }
                self.prev_symbol = y;
                self.primed = true;
                out.push(y);
            } else {
                self.mid = y;
            }
            self.at_symbol = !self.at_symbol;
            self.advance();
        }
        let drain = (self.pos - 1)
            .saturating_sub(self.consumed)
            .min(self.buf.len());
        self.buf.drain(..drain);
        self.consumed += drain;
    }

    /// Current estimate of input samples per symbol (tracks the transmitter's clock).
    #[must_use]
    pub fn sps(&self) -> f64 {
        self.sps
    }

    pub fn reset(&mut self) {
        self.sps = self.nominal_sps;
        self.step = self.nominal_sps;
        // The first instant is the earliest one the interpolator has history for.
        self.pos = 1;
        self.frac = 0.0;
        self.consumed = 0;
        self.buf.clear();
        self.at_symbol = true;
        self.prev_symbol = ZERO;
        self.mid = ZERO;
        self.primed = false;
    }

    fn advance(&mut self) {
        self.frac += 0.5 * self.step;
        let whole = self.frac.floor();
        // Exact for a positive step: both operands share an exponent range where the
        // difference is representable, so the fraction never accumulates split-dependent drift.
        self.frac -= whole;
        self.pos += whole as usize;
    }

    fn retime(&mut self, symbol: Complex<f32>) {
        // Gardner: a ±A transition through a Nyquist pulse crosses zero with slope πA per
        // symbol, so Re{(x[k] − x[k−1])·conj(x_mid)} ≈ 2π·A²·τ. Dividing by 2π times the
        // symbol energy reads τ off directly in symbol periods, at any signal amplitude.
        let raw = ((symbol - self.prev_symbol) * self.mid.conj()).re;
        let energy = 0.5 * (self.prev_symbol.norm_sqr() + symbol.norm_sqr());
        let err = f64::from(raw) / (f64::from(energy) * TAU);
        // Silence (energy 0) and non-finite input must free-run, not poison the accumulator.
        if !err.is_finite() {
            self.step = self.sps;
            return;
        }
        let err = err.clamp(-0.5, 0.5);
        // A positive error means the instant landed late, so shorten this symbol's interval
        // and slow the estimated clock.
        self.sps = (self.sps - self.beta * err * self.nominal_sps).clamp(
            self.nominal_sps * (1.0 - TRACKING_RANGE),
            self.nominal_sps * (1.0 + TRACKING_RANGE),
        );
        self.step = self.sps - self.alpha * err * self.nominal_sps;
    }
}

/// Piecewise-parabolic interpolation at `mu` in [0, 1) between `w[1]` and `w[2]`.
fn farrow(w: &[Complex<f32>], mu: f32) -> Complex<f32> {
    let curvature = w[3] - w[2] - w[1] + w[0];
    w[1] + (w[2] - w[1]) * mu + curvature * (FARROW_CURVATURE * mu * (mu - 1.0))
}

/// A zero crossing pulls the bit phase this far toward the mid-bit position — the 1/8 of the
/// classic 1200 baud TNC clock recovery: fast enough to lock inside a preamble, slow enough
/// that one noisy crossing cannot steal the clock.
const CROSSING_NUDGE: f64 = 0.125;

/// Bit-clock recovery for a real-valued sliced baseband (FSK discriminator output, AFSK
/// correlator difference): tracks the symbol phase from zero crossings and samples at the
/// centre of each bit. This is the classic "PLL on transitions" used by 1200 baud TNCs.
#[derive(Clone, Debug)]
pub struct BitSync {
    sample_rate: f64,
    increment: f64,
    /// Bit phase in [0, 1), wrapping at the slicing instant. A zero crossing therefore belongs
    /// at 0.5 — half a bit ahead of the slice, i.e. the slice lands at the bit centre.
    phase: f64,
    positive: bool,
    primed: bool,
    since_symbol: usize,
}

impl BitSync {
    #[must_use]
    pub fn new(sample_rate: f64, baud: f64) -> Self {
        assert!(sample_rate > 0.0, "sample rate must be positive");
        let mut sync = Self {
            sample_rate,
            increment: 0.0,
            phase: 0.0,
            positive: false,
            primed: false,
            since_symbol: 0,
        };
        sync.set_baud(baud);
        sync.reset();
        sync
    }

    pub fn set_baud(&mut self, baud: f64) {
        assert!(
            baud.is_finite() && baud > 0.0 && baud * 2.0 <= self.sample_rate,
            "baud must be positive and at most half the sample rate"
        );
        self.increment = baud / self.sample_rate;
    }

    /// Feed one sample; returns the sliced bit at each symbol instant.
    pub fn push(&mut self, sample: f32) -> Option<bool> {
        let positive = sample >= 0.0;
        if self.primed && positive != self.positive {
            self.phase += CROSSING_NUDGE * (0.5 - self.phase);
        }
        self.positive = positive;
        self.primed = true;

        self.phase += self.increment;
        self.since_symbol += 1;
        if self.phase < 1.0 {
            return None;
        }
        self.phase -= 1.0;
        self.since_symbol = 0;
        Some(positive)
    }

    /// Samples since the last symbol instant — lets a decoder detect a gap in the carrier.
    #[must_use]
    pub fn samples_since_symbol(&self) -> usize {
        self.since_symbol
    }

    pub fn reset(&mut self) {
        // Half a bit of phase: with no crossings yet, a stream that starts on a bit boundary
        // still gets sliced at the centre of its first bit.
        self.phase = 0.5;
        self.positive = false;
        self.primed = false;
        self.since_symbol = 0;
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use super::*;
    use crate::testutil::XorShift32;

    const ROLLOFF: f64 = 0.35;
    /// Pulse truncation, in symbol periods either side.
    const SPAN: f64 = 6.0;

    fn sinc(t: f64) -> f64 {
        if t.abs() < 1e-12 {
            1.0
        } else {
            (PI * t).sin() / (PI * t)
        }
    }

    fn raised_cosine(t: f64) -> f64 {
        let denom = 1.0 - (2.0 * ROLLOFF * t).powi(2);
        if denom.abs() < 1e-8 {
            return std::f64::consts::FRAC_PI_4 * sinc(t);
        }
        sinc(t) * (PI * ROLLOFF * t).cos() / denom
    }

    fn symbols(count: usize, seed: u32) -> Vec<f32> {
        let mut rng = XorShift32(seed);
        (0..count)
            .map(|_| if rng.next_f32() >= 0.0 { 1.0 } else { -1.0 })
            .collect()
    }

    /// Pulse-shaped BPSK at `sps` samples per symbol, symbol 0 peaking `offset` symbols in.
    fn bpsk(syms: &[f32], sps: f64, offset: f64) -> Vec<Complex<f32>> {
        let len = ((syms.len() as f64 - SPAN) * sps) as usize;
        (0..len)
            .map(|n| {
                let t = n as f64 / sps - offset;
                let lo = (t - SPAN).ceil().max(0.0) as usize;
                let hi = ((t + SPAN) as usize).min(syms.len() - 1);
                let v: f64 = (lo..=hi)
                    .map(|k| f64::from(syms[k]) * raised_cosine(t - k as f64))
                    .sum();
                Complex::new(v as f32, 0.0)
            })
            .collect()
    }

    /// Recovered signs equal the transmitted signs shifted by `d`, ignoring settling and the
    /// truncated tail of the pulse train.
    fn matches_at(syms: &[f32], out: &[Complex<f32>], settle: usize, d: usize) -> bool {
        let end = out.len().saturating_sub(4);
        settle < end
            && (settle..end).all(|j| j + d < syms.len() && (out[j].re > 0.0) == (syms[j + d] > 0.0))
    }

    fn alignment(syms: &[f32], out: &[Complex<f32>], settle: usize) -> Option<usize> {
        (0..8).find(|&d| matches_at(syms, out, settle, d))
    }

    #[test]
    fn recovers_bpsk_symbols_through_a_fractional_timing_offset() {
        let syms = symbols(600, 0x1234_5678);
        let signal = bpsk(&syms, 4.0, 0.37);
        let mut sync = SymbolSync::new(4.0, 0.02);
        let mut out = Vec::new();
        sync.process(&signal, &mut out);
        assert!(
            alignment(&syms, &out, 60).is_some(),
            "no offset aligns {} recovered symbols",
            out.len()
        );
    }

    #[test]
    fn ragged_blocks_match_one_shot_exactly() {
        let syms = symbols(400, 0x2bad_c0de);
        let signal = bpsk(&syms, 4.0, 0.21);

        let mut whole = SymbolSync::new(4.0, 0.02);
        let mut expected = Vec::new();
        whole.process(&signal, &mut expected);

        let mut ragged = SymbolSync::new(4.0, 0.02);
        let mut got = Vec::new();
        let mut pos = 0;
        for len in [1usize, 7, 64, 3, 129, 1024, 17].iter().cycle() {
            if pos >= signal.len() {
                break;
            }
            let end = (pos + len).min(signal.len());
            ragged.process(&signal[pos..end], &mut got);
            pos = end;
        }
        assert_eq!(expected, got);
    }

    #[test]
    fn tracks_a_half_percent_clock_error_without_slipping() {
        const TRUE_SPS: f64 = 4.02;
        let syms = symbols(3_000, 0x0f0f_1234);
        let signal = bpsk(&syms, TRUE_SPS, 0.1);
        let mut sync = SymbolSync::new(4.0, 0.02);
        let mut out = Vec::new();
        sync.process(&signal, &mut out);

        assert!(
            (sync.sps() - TRUE_SPS).abs() < 5e-3,
            "sps estimate {} did not converge on {TRUE_SPS}",
            sync.sps()
        );
        // First instant at sample 1, last one needs three samples of lookahead.
        let ideal = ((signal.len() - 4) as f64 / TRUE_SPS).floor() as i64;
        assert!(
            (out.len() as i64 - ideal).abs() <= 2,
            "recovered {} symbols, ideal {ideal}",
            out.len()
        );
        assert!(
            alignment(&syms, &out, 300).is_some(),
            "symbol stream slipped while tracking"
        );
    }

    #[test]
    fn silence_free_runs_at_the_nominal_rate() {
        let mut sync = SymbolSync::new(4.0, 0.02);
        let mut out = Vec::new();
        sync.process(&vec![ZERO; 4_000], &mut out);
        assert_eq!(sync.sps(), 4.0, "silence must not pull the clock");
        assert!((out.len() as i64 - 999).abs() <= 2, "{} symbols", out.len());
    }

    const RATE: f64 = 48_000.0;
    const BAUD: f64 = 1_200.0;
    const SAMPLES_PER_BIT: usize = 40;

    fn nrz(bits: &[bool]) -> Vec<f32> {
        bits.iter()
            .flat_map(|&b| std::iter::repeat_n(if b { 1.0 } else { -1.0 }, SAMPLES_PER_BIT))
            .collect()
    }

    fn bit_pattern(count: usize, seed: u32) -> Vec<bool> {
        let mut rng = XorShift32(seed);
        (0..count).map(|_| rng.next_f32() >= 0.0).collect()
    }

    fn recover(samples: &[f32]) -> Vec<bool> {
        let mut sync = BitSync::new(RATE, BAUD);
        samples.iter().filter_map(|&s| sync.push(s)).collect()
    }

    fn bits_align(tx: &[bool], rx: &[bool], settle: usize) -> bool {
        (0..3).any(|d| {
            let end = rx.len().saturating_sub(1);
            settle < end && (settle..end).all(|j| j + d < tx.len() && rx[j] == tx[j + d])
        })
    }

    #[test]
    fn recovers_a_bit_pattern_locked_to_the_waveform() {
        let bits = bit_pattern(500, 0x5eed_1234);
        let rx = recover(&nrz(&bits));
        assert_eq!(rx.len(), 500, "one slice per bit expected");
        assert_eq!(rx, bits);
    }

    #[test]
    fn locks_from_a_half_bit_phase_offset_and_stays_locked() {
        let bits = bit_pattern(500, 0xfeed_beef);
        let waveform = nrz(&bits);
        let rx = recover(&waveform[SAMPLES_PER_BIT / 2..]);
        assert!(
            (rx.len() as i64 - 500).abs() <= 2,
            "recovered {} bits from 500",
            rx.len()
        );
        assert!(bits_align(&bits, &rx, 64), "never locked to the bit clock");
    }

    #[test]
    fn free_runs_on_a_crossing_free_input() {
        let mut sync = BitSync::new(RATE, BAUD);
        let (mut count, mut since) = (0, 0);
        for _ in 0..400 {
            since += 1;
            if sync.push(1.0).is_some() {
                count += 1;
                since = 0;
            }
            assert_eq!(sync.samples_since_symbol(), since);
        }
        assert_eq!(count, 10, "constant input must keep the clock running");
    }

    #[test]
    fn set_baud_keeps_the_current_phase() {
        let mut sync = BitSync::new(RATE, BAUD);
        while sync.push(1.0).is_none() {}
        sync.set_baud(2.0 * BAUD);
        // Phase is 0 right after a slice, so the doubled rate must take exactly half a bit
        // period — a reset phase would show up as a quarter.
        let mut gap = 0;
        while sync.push(1.0).is_none() {
            gap += 1;
        }
        assert_eq!(gap + 1, SAMPLES_PER_BIT / 2);
    }
}
