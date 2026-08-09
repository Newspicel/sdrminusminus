//! Tone and envelope detection (PLAN §7): the single-bin detectors behind AFSK/CTCSS/Selcall,
//! plus the envelope follower and adaptive keying slicer a CW decoder runs on. All state is
//! per-sample and allocation-free — safe on the DSP thread.

use std::f64::consts::TAU;

use num_complex::Complex;

use crate::iir::one_pole_coeff;

/// Goertzel single-bin power over a fixed block — the cheapest "is this tone present".
/// Resolution is `sample_rate / block`; `freq_hz` need not sit on a bin center, but off-center
/// frequencies pay the usual rectangular-window scalloping loss (up to −3.9 dB at half a bin).
#[derive(Clone, Debug)]
pub struct Goertzel {
    coeff: f32,
    /// Scales `|X|²` to squared amplitude: an on-bin unit sine gives `|X| = block/2`.
    norm: f32,
    block: usize,
    n: usize,
    s1: f32,
    s2: f32,
}

impl Goertzel {
    #[must_use]
    pub fn new(sample_rate: f64, freq_hz: f64, block: usize) -> Self {
        assert!(sample_rate > 0.0, "sample rate must be positive");
        assert!(
            freq_hz > 0.0 && freq_hz < sample_rate / 2.0,
            "freq must lie inside the Nyquist band"
        );
        assert!(block >= 2, "block must hold at least two samples");
        Self {
            coeff: 2.0 * (TAU * freq_hz / sample_rate).cos() as f32,
            norm: (2.0 / block as f64).powi(2) as f32,
            block,
            n: 0,
            s1: 0.0,
            s2: 0.0,
        }
    }

    /// Feed one sample; returns the block's power when the block completes.
    pub fn push(&mut self, sample: f32) -> Option<f32> {
        let s0 = sample + self.coeff * self.s1 - self.s2;
        self.s2 = self.s1;
        self.s1 = s0;
        self.n += 1;
        if self.n < self.block {
            return None;
        }
        let power = self.s1 * self.s1 + self.s2 * self.s2 - self.coeff * self.s1 * self.s2;
        // The recurrence's poles sit on the unit circle, so it would latch a non-finite sample
        // forever; clearing at every block boundary bounds the damage to one block.
        self.reset();
        Some(power * self.norm)
    }

    pub fn reset(&mut self) {
        self.n = 0;
        self.s1 = 0.0;
        self.s2 = 0.0;
    }
}

/// Sliding single-frequency correlator: the magnitude of a `window`-long DFT bin, updated
/// every sample. Two of these (mark and space) are the classic AFSK1200 detector. Scaled so a
/// unit-amplitude sine on the analysed frequency reads ≈ 1.0.
#[derive(Clone, Debug)]
pub struct ToneCorrelator {
    /// `e^(−jω)` — one sample of the analysed tone's phase.
    rot: Complex<f64>,
    /// `e^(−jωN)` — the phase the departing sample accumulated while inside the window.
    exit: Complex<f64>,
    norm: f32,
    buf: Vec<f32>,
    /// Index of the oldest sample, i.e. where the next one is written.
    pos: usize,
    acc: Complex<f64>,
    since_rebuild: usize,
}

impl ToneCorrelator {
    #[must_use]
    pub fn new(sample_rate: f64, freq_hz: f64, window: usize) -> Self {
        assert!(sample_rate > 0.0, "sample rate must be positive");
        assert!(
            freq_hz > 0.0 && freq_hz < sample_rate / 2.0,
            "freq must lie inside the Nyquist band"
        );
        assert!(window >= 2, "window must hold at least two samples");
        let omega = TAU * freq_hz / sample_rate;
        Self {
            rot: Complex::from_polar(1.0, -omega),
            exit: Complex::from_polar(1.0, -omega * window as f64),
            norm: (2.0 / window as f64) as f32,
            buf: vec![0.0; window],
            pos: 0,
            acc: Complex::new(0.0, 0.0),
            since_rebuild: 0,
        }
    }

    /// Feed one sample; returns the current correlation magnitude.
    pub fn push(&mut self, sample: f32) -> f32 {
        let leaving = self.buf[self.pos];
        self.buf[self.pos] = sample;
        self.pos = (self.pos + 1) % self.buf.len();
        // S[n] = e^(−jω)·S[n−1] + x[n] − x[n−N]·e^(−jωN): the sliding DFT of the window with
        // the newest sample as phase reference, valid for any ω (not just bin centers).
        self.acc = self.acc * self.rot + f64::from(sample) - self.exit * f64::from(leaving);
        self.since_rebuild += 1;
        if self.since_rebuild >= self.buf.len() {
            self.rebuild();
        }
        self.acc.norm() as f32 * self.norm
    }

    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
        self.acc = Complex::new(0.0, 0.0);
        self.since_rebuild = 0;
    }

    /// The recurrence's pole sits exactly on the unit circle, so its rounding error never
    /// decays. Recomputing the sum straight from the ring once per window bounds the error to
    /// one window's worth of rounding at O(1) amortized cost, and heals a non-finite sample
    /// within one window of it leaving the ring.
    fn rebuild(&mut self) {
        let n = self.buf.len();
        let mut acc = Complex::new(0.0, 0.0);
        let mut phasor = Complex::new(1.0, 0.0);
        for m in 0..n {
            acc += phasor * f64::from(self.buf[(self.pos + n - 1 - m) % n]);
            phasor *= self.rot;
        }
        self.acc = acc;
        self.since_rebuild = 0;
    }
}

/// One-pole envelope follower with independent attack and release time constants: attack
/// applies while the input sits above the current value, release while it sits below.
#[derive(Clone, Debug)]
pub struct Envelope {
    value: f32,
    attack: f32,
    release: f32,
}

impl Envelope {
    #[must_use]
    pub fn new(sample_rate: f64, attack_s: f64, release_s: f64) -> Self {
        assert!(sample_rate > 0.0, "sample rate must be positive");
        assert!(
            attack_s > 0.0 && release_s > 0.0,
            "time constants must be positive"
        );
        Self {
            value: 0.0,
            attack: one_pole_coeff(sample_rate, attack_s),
            release: one_pole_coeff(sample_rate, release_s),
        }
    }

    pub fn push(&mut self, magnitude: f32) -> f32 {
        // One non-finite input would latch the recursion forever; hold the last good value so
        // a driver glitch costs a sample rather than the channel.
        if magnitude.is_finite() {
            let coeff = if magnitude > self.value {
                self.attack
            } else {
                self.release
            };
            self.value += coeff * (magnitude - self.value);
        }
        self.value
    }

    #[must_use]
    pub fn value(&self) -> f32 {
        self.value
    }

    pub fn reset(&mut self) {
        self.value = 0.0;
    }
}

/// Peak rises with the envelope almost immediately but decays over several Morse elements, so
/// a dot's amplitude still sets the reference during the space that follows it.
const PEAK_RISE_S: f64 = 5e-3;
const PEAK_FALL_S: f64 = 0.5;
/// The floor mirrors it: it drops into a key-up gap within one gap, and only creeps back up
/// over a band-noise timescale.
const FLOOR_FALL_S: f64 = 20e-3;
const FLOOR_RISE_S: f64 = 1.0;
/// Hysteresis as a fraction of the peak-to-floor span, applied either side of the midpoint.
const HYSTERESIS: f32 = 0.1;
/// Below this peak-to-floor ratio the "signal" is indistinguishable from a noise envelope's own
/// crest factor (~3 for a lightly smoothed one), so the slicer refuses to key at all.
const MIN_SNR: f32 = 6.0;
/// Seeding both trackers from zero would let the floor climb on its own (slow) constant while
/// the peak snaps to the input, faking a full-scale signal for the first second. Instead they
/// start together at the loudest sample of a warm-up window — long enough for an upstream
/// envelope follower to settle, far shorter than a Morse element.
const WARMUP_S: f64 = FLOOR_FALL_S;

/// Adaptive on/off threshold for a keyed envelope (CW): tracks slow noise-floor and peak
/// estimates and slices halfway between them, with hysteresis.
#[derive(Clone, Debug)]
pub struct KeyingSlicer {
    peak: f32,
    floor: f32,
    warmup_samples: u32,
    peak_rise: f32,
    peak_fall: f32,
    floor_rise: f32,
    floor_fall: f32,
    warmup: u32,
    key: bool,
}

impl KeyingSlicer {
    #[must_use]
    pub fn new(sample_rate: f64) -> Self {
        assert!(sample_rate > 0.0, "sample rate must be positive");
        let warmup_samples = (WARMUP_S * sample_rate).round().max(1.0) as u32;
        Self {
            peak: 0.0,
            floor: 0.0,
            warmup_samples,
            peak_rise: one_pole_coeff(sample_rate, PEAK_RISE_S),
            peak_fall: one_pole_coeff(sample_rate, PEAK_FALL_S),
            floor_rise: one_pole_coeff(sample_rate, FLOOR_RISE_S),
            floor_fall: one_pole_coeff(sample_rate, FLOOR_FALL_S),
            warmup: warmup_samples,
            key: false,
        }
    }

    /// Feed an envelope sample; returns the key state (true = tone present).
    pub fn push(&mut self, envelope: f32) -> bool {
        // Same healing rule as `Envelope`: a non-finite sample would latch both trackers.
        if !envelope.is_finite() {
            return self.key;
        }
        if self.warmup > 0 {
            self.warmup -= 1;
            self.peak = self.peak.max(envelope);
            self.floor = self.peak;
            return false;
        }
        let peak_coeff = if envelope > self.peak {
            self.peak_rise
        } else {
            self.peak_fall
        };
        self.peak += peak_coeff * (envelope - self.peak);
        // A key-down run carries no information about the noise floor; letting the floor creep
        // toward the tone through a long dash would erode the reported SNR until the slicer
        // dropped the key mid-element. Falling is never gated, so the estimate cannot latch
        // high — a noise floor that rises while keyed still unkeys on the first dip and
        // resumes tracking.
        let floor_coeff = if envelope < self.floor {
            self.floor_fall
        } else if self.key {
            0.0
        } else {
            self.floor_rise
        };
        self.floor += floor_coeff * (envelope - self.floor);

        let span = (self.peak - self.floor).max(0.0);
        let mid = self.floor + span / 2.0;
        let guard = span * HYSTERESIS;
        if envelope > mid + guard {
            self.key = true;
        } else if envelope < mid - guard {
            self.key = false;
        }
        if self.snr() < MIN_SNR {
            self.key = false;
        }
        self.key
    }

    /// Current signal-to-floor ratio — a decoder uses it to ignore pure noise. Linear power
    /// ratio of the tracked peak to the tracked floor, not dB.
    #[must_use]
    pub fn snr(&self) -> f32 {
        self.peak / self.floor.max(f32::MIN_POSITIVE)
    }

    pub fn reset(&mut self) {
        self.peak = 0.0;
        self.floor = 0.0;
        self.warmup = self.warmup_samples;
        self.key = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{XorShift32, real_tone};

    const RATE: f64 = 48_000.0;
    const MARK_HZ: f64 = 1_200.0;
    const SPACE_HZ: f64 = 2_200.0;

    fn db(ratio: f32) -> f32 {
        20.0 * ratio.log10()
    }

    #[test]
    fn goertzel_separates_its_bin_from_the_next_one() {
        // 40-sample blocks at 48 kHz put the bins 1.2 kHz apart, so 2.4 kHz lands on a null of
        // the 1.2 kHz analysis window.
        let block = 40;
        let power = |freq: f64| {
            let mut g = Goertzel::new(RATE, MARK_HZ, block);
            let mut last = 0.0;
            for &s in &real_tone(freq / RATE, block * 10) {
                if let Some(p) = g.push(s) {
                    last = p;
                }
            }
            last
        };
        let on_bin = power(MARK_HZ);
        let off_bin = power(2_400.0);
        assert!((on_bin - 1.0).abs() < 0.05, "on-bin power {on_bin}");
        assert!(
            off_bin < on_bin / 1e4,
            "off-bin power {off_bin} vs {on_bin}"
        );
    }

    #[test]
    fn goertzel_emits_once_per_block_and_resets() {
        let mut g = Goertzel::new(RATE, MARK_HZ, 40);
        let emitted = real_tone(MARK_HZ / RATE, 200)
            .iter()
            .filter_map(|&s| g.push(s))
            .count();
        assert_eq!(emitted, 5);
    }

    /// Steady-state correlator magnitude, measured after the window has filled.
    fn correlate(tone_hz: f64, detector_hz: f64, window: usize) -> f32 {
        let mut c = ToneCorrelator::new(RATE, detector_hz, window);
        let x = real_tone(tone_hz / RATE, window * 20);
        let mut peak = 0.0f32;
        for (n, &s) in x.iter().enumerate() {
            let m = c.push(s);
            if n >= window * 4 {
                peak = peak.max(m);
            }
        }
        peak
    }

    #[test]
    fn bell202_tones_separate_by_more_than_20_db() {
        // A 48-sample window at 48 kHz spaces the bins 1 kHz apart — exactly the mark/space
        // split — so each detector sits on the other tone's null.
        let window = 48;
        for (tone, other) in [(MARK_HZ, SPACE_HZ), (SPACE_HZ, MARK_HZ)] {
            let matched = correlate(tone, tone, window);
            let rejected = correlate(tone, other, window);
            let separation = db(matched / rejected);
            assert!(
                separation > 20.0,
                "{tone} Hz: matched {matched}, rejected {rejected} ({separation} dB)"
            );
        }
    }

    #[test]
    fn sliding_recurrence_stays_accurate_over_200k_samples() {
        let window = 48;
        let x = real_tone(1_700.0 / RATE, 200_000 + window);
        let mut aged = ToneCorrelator::new(RATE, MARK_HZ, window);
        for &s in &x[..200_000] {
            aged.push(s);
        }
        // A fresh correlator holds the identical window once it has seen `window` samples.
        let mut fresh = ToneCorrelator::new(RATE, MARK_HZ, window);
        let (mut a, mut b) = (0.0, 0.0);
        for &s in &x[200_000..] {
            a = aged.push(s);
            b = fresh.push(s);
        }
        assert!((a - b).abs() < 1e-5, "drifted: aged {a} vs fresh {b}");
    }

    #[test]
    fn envelope_attack_and_release_hit_63_percent_at_their_time_constants() {
        let (attack, release) = (0.010, 0.100);
        let mut env = Envelope::new(RATE, attack, release);
        for _ in 0..(attack * RATE) as usize {
            env.push(1.0);
        }
        let risen = env.value();
        assert!((0.569..0.696).contains(&risen), "attack reached {risen}");

        let mut env = Envelope::new(RATE, attack, release);
        for _ in 0..(4.0 * attack * RATE) as usize {
            env.push(1.0);
        }
        let start = env.value();
        for _ in 0..(release * RATE) as usize {
            env.push(0.0);
        }
        let fallen = (start - env.value()) / start;
        assert!((0.569..0.696).contains(&fallen), "release fell {fallen}");
    }

    #[test]
    fn envelope_holds_through_a_non_finite_sample() {
        let mut env = Envelope::new(RATE, 0.001, 0.001);
        for _ in 0..480 {
            env.push(1.0);
        }
        let before = env.value();
        assert_eq!(env.push(f32::NAN), before);
        assert!(env.push(1.0).is_finite(), "state poisoned by NaN");
    }

    const CW_RATE: f64 = 8_000.0;
    /// 60 ms dots — 20 wpm.
    const DOT: usize = 480;

    /// Build a keyed envelope for `pattern` (true = key down, in dot units) with additive noise.
    fn keyed_envelope(pattern: &[(bool, usize)], noise: f32) -> Vec<f32> {
        let mut rng = XorShift32(0x5eed_1234);
        let mut env = Envelope::new(CW_RATE, 2e-3, 2e-3);
        let mut out = Vec::new();
        for &(on, dots) in pattern {
            for _ in 0..dots * DOT {
                let level = f32::from(u8::from(on)) + rng.next_f32() * noise;
                out.push(env.push(level.max(0.0)));
            }
        }
        out
    }

    #[test]
    fn keying_slicer_recovers_a_noisy_pattern() {
        // PARIS-ish: dot, dash, dot dot — with the standard 1-dot intra-character spacing.
        let pattern = [
            (false, 4),
            (true, 1),
            (false, 1),
            (true, 3),
            (false, 1),
            (true, 1),
            (false, 1),
            (true, 1),
            (false, 3),
        ];
        let signal = keyed_envelope(&pattern, 0.1);
        let mut slicer = KeyingSlicer::new(CW_RATE);
        let keys: Vec<bool> = signal.iter().map(|&v| slicer.push(v)).collect();

        let mut start = 0;
        for (i, &(on, dots)) in pattern.iter().enumerate() {
            let len = dots * DOT;
            // Skip the leading element: the trackers are still seeding on the noise floor.
            if i > 0 {
                // Judge the middle half of the element, clear of both edges.
                let steady = &keys[start + len / 4..start + 3 * len / 4];
                let wrong = steady.iter().filter(|&&k| k != on).count();
                assert_eq!(
                    wrong,
                    0,
                    "element {i} (key {on}): {wrong} of {} samples wrong",
                    steady.len()
                );
            }
            start += len;
        }
    }

    #[test]
    fn keying_slicer_ignores_pure_noise() {
        let signal = keyed_envelope(&[(false, 40)], 0.5);
        let mut slicer = KeyingSlicer::new(CW_RATE);
        for (n, &v) in signal.iter().enumerate() {
            assert!(!slicer.push(v), "keyed on noise at sample {n}");
        }
        let snr = slicer.snr();
        assert!(snr < MIN_SNR, "noise reported snr {snr}");
    }

    #[test]
    fn keying_slicer_holds_through_a_non_finite_sample() {
        let signal = keyed_envelope(&[(false, 2), (true, 4)], 0.05);
        let mut slicer = KeyingSlicer::new(CW_RATE);
        let mut key = false;
        for &v in &signal {
            key = slicer.push(v);
        }
        assert!(key, "should be keyed at the end of a key-down run");
        assert!(slicer.push(f32::NAN), "state dropped on NaN");
        assert!(slicer.snr().is_finite());
    }
}
