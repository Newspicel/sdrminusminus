//! The noncoherent envelope tier ( §6, the M-PAM/M-ASK/OOK row): magnitude detection
//! with an adaptive threshold, for a receiver that never recovers the carrier at all.
//!
//! **Why a separate chain rather than a mode of [`demod`](super::demod).** Everything downstream
//! of a magnitude detector is real, and everything upstream of it must not assume a phase. The
//! Gardner detector inside `SymbolSync` reads `Re{·}` and is therefore phase-sensitive, so the
//! order is forced: matched filter → magnitude → DC removal → timing → levels. A coherent chain
//! cannot be bent into that shape by a flag; it is a different chain, and pretending otherwise is
//! how a "mode" grows branches.
//!
//! **The DC removal is load-bearing, not hygiene.** Gardner's error term is
//! `mid · (late − early)`, which is a *zero-mean* signal's transition asymmetry — feed it a
//! unipolar stream and the mid-sample's pedestal multiplies every transition, so the detector
//! reports a bias that depends on the data rather than the timing, and the loop settles off the
//! Nyquist instant. Measured on noiseless 4-ASK: without it the recovered amplitudes scatter by
//! 0.12 RMS against a slicing margin of 0.267 and one symbol in a hundred is wrong on a signal
//! with no noise in it at all. The CPM engine subtracts its own centre estimate before
//! `SymbolSync` for exactly this reason (`cpm::demod`'s `centred` stage); this is the same fix in
//! the domain where the pedestal comes from the magnitude operator instead of the alphabet.
//!
//! **The adaptive threshold is two moments, not two extremes.** `sdrmm_dsp`'s
//! [`KeyingSlicer`](sdrmm_dsp::KeyingSlicer) — which morse and subghz run on — tracks a peak and
//! a floor, and that is right for what it does: asynchronous keying whose duty cycle is unknown
//! and lopsided, where averages say nothing about where the threshold belongs. This tier faces
//! the opposite situation. Its symbols are equiprobable draws from a known table at known
//! instants, so the envelope's mean and variance are known multiples of the amplitude scale and
//! the pedestal it sits on:
//!
//! ```text
//! e = g·a + p   ⇒   g = sd(e)/sd(a),   p = mean(e) − g·mean(a)
//! ```
//!
//! Both statistics are running means, so both are smooth and unbiased; an extremum tracker is
//! neither. Measured on noiseless 4-ASK, a peak/floor pair scatters the recovered amplitudes by
//! 10 % of the scale — because a min-chasing floor is an order statistic, and the attack/decay
//! asymmetry of a peak equilibrates below the true maximum (the ×1.09 the CPM engine measured on
//! its 8-level alphabet, `cpm::demod`'s `PEAK_SYMBOLS`). The two moments hold the same signal
//! inside 2 %.
//!
//! **The pedestal is not zero, and that is why it is fitted.** After magnitude detection an off
//! symbol carries `|noise|`, which is Rayleigh with a *positive* mean — so the levels ride on a
//! pedestal that moves with the noise floor, and a threshold at half the nominal amplitude is
//! what fails first when the front-end gain or the band noise changes.
//!
//! **The blind estimate's bias, stated once.** Noise adds its own variance to `Var(e)`, so the
//! fitted gain runs high by `√(1 + Var_noise/(g²·Var(a)))` — the same shape as the coherent
//! chain's `√(1 + 1/SNR)` power-normalisation bias, and answered the same way: the §3.4
//! known-symbol hook, whose fit compares against what was actually sent and has no such term.
//!
//! **The cost of noncoherence, stated once.** Against coherent detection of the same table an
//! envelope receiver gives away roughly 1 dB near BER 1e-3 (and less as SNR rises), because the
//! magnitude of a complex Gaussian around a point is not the projection onto it. That is the
//! measured tier gap the catalog's OOK row records, not a number this comment asserts.

use num_complex::Complex;
use sdrmm_dsp::{Decimator, SymbolSync, one_pole_coeff};

use super::params::LinearParams;
use crate::constellation::Constellation;

/// Time constants of the level trackers, in **symbols** (see the module docs on why the units
/// are symbols and not seconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvelopeTiming {
    /// One-pole time constant of both envelope moments — the scale and pedestal reference.
    pub mean_symbols: f64,
    /// Symbols the moments are seeded from before any level is reported: the mean of the
    /// window and its smallest sample. Seeding from zero instead would report a full-scale
    /// first symbol whatever arrived.
    pub warmup_symbols: usize,
    /// One-pole time constant, in symbols, of the DC estimate removed before the timing
    /// detector. Shorter than the mean tracker's on purpose: this one only has to be right
    /// enough that Gardner sees a zero-mean signal, and it must settle before the timing loop
    /// does or the loop acquires against a moving pedestal.
    pub dc_symbols: f64,
}

impl EnvelopeTiming {
    /// The catalog's continuous-stream operating point. 200 symbols of averaging holds the fitted
    /// scale near 2 % on a 4-ASK payload — the per-symbol amplitude's coefficient of variation is
    /// 0.75 there, and a one-pole at α = 1/200 divides it by √(2/α) — against a slicing margin of
    /// 33 % of the mean, while still following a fading transmitter inside a burst.
    pub const CONTINUOUS: Self = Self {
        mean_symbols: 200.0,
        warmup_symbols: 32,
        dc_symbols: 50.0,
    };
}

/// The pedestal-and-scale estimator described in the module docs: two running moments of the
/// envelope, and the affine map they define onto the table's own amplitude axis.
#[derive(Clone, Debug)]
struct LevelTracker {
    mean: f32,
    mean_square: f32,
    alpha: f32,
    warmup: usize,
    seen: usize,
    seed_sum: f32,
    seed_square: f32,
    /// Mean and standard deviation of |point| over the table — what the two tracked moments
    /// correspond to.
    table_mean: f32,
    table_sd: f32,
}

impl LevelTracker {
    fn new(timing: EnvelopeTiming, table_mean: f32, table_sd: f32) -> Self {
        Self {
            mean: 0.0,
            mean_square: 0.0,
            // Time constant in symbols: a "sample rate" of 1.0 means one sample per symbol.
            alpha: one_pole_coeff(1.0, timing.mean_symbols),
            warmup: timing.warmup_symbols.max(2),
            seen: 0,
            seed_sum: 0.0,
            seed_square: 0.0,
            table_mean,
            table_sd,
        }
    }

    /// Feed one symbol's envelope; returns its amplitude on the table's axis, or `None` while
    /// the moments are still being seeded.
    fn push(&mut self, envelope: f32) -> Option<f32> {
        if !envelope.is_finite() {
            return None;
        }
        if self.seen < self.warmup {
            self.seed_sum += envelope;
            self.seed_square += envelope * envelope;
            self.seen += 1;
            if self.seen == self.warmup {
                let n = self.warmup as f32;
                self.mean = self.seed_sum / n;
                self.mean_square = self.seed_square / n;
            }
            return None;
        }
        self.mean += self.alpha * (envelope - self.mean);
        self.mean_square += self.alpha * (envelope * envelope - self.mean_square);
        // A negative variance is arithmetic noise on a constant stream, not a signal; the
        // clamp keeps the gain finite instead of producing a NaN scale.
        let variance = (self.mean_square - self.mean * self.mean).max(0.0);
        let gain = variance.sqrt() / self.table_sd;
        if gain <= f32::MIN_POSITIVE {
            return Some(self.table_mean);
        }
        let pedestal = self.mean - gain * self.table_mean;
        Some((envelope - pedestal) / gain)
    }

    /// Ratio of the tracked mean to the fitted pedestal — a carrier-present test. On a dead
    /// channel the levels collapse onto the pedestal and the ratio sits at 1.
    fn snr(&self) -> f32 {
        let variance = (self.mean_square - self.mean * self.mean).max(0.0);
        let gain = variance.sqrt() / self.table_sd;
        let pedestal = self.mean - gain * self.table_mean;
        self.mean / pedestal.max(f32::MIN_POSITIVE)
    }

    fn reset(&mut self) {
        self.mean = 0.0;
        self.mean_square = 0.0;
        self.seen = 0;
        self.seed_sum = 0.0;
        self.seed_square = 0.0;
    }
}

/// Noncoherent envelope demodulator for unipolar amplitude alphabets. Emits one real soft
/// amplitude per symbol period, scaled so the table's own amplitudes are the reference: 0 at the
/// tracked floor, and the table's largest amplitude at the tracked peak.
pub struct EnvelopeDemod {
    matched: Decimator,
    sync: SymbolSync,
    /// Running DC of the magnitude stream, and its per-sample one-pole coefficient.
    dc: f32,
    dc_alpha: f32,
    dc_primed: bool,
    levels: LevelTracker,
    /// The table's smallest amplitude — what a not-yet-seeded tracker reports, and what an
    /// unkeyed channel is.
    quietest: f32,
    filtered: Vec<Complex<f32>>,
    magnitude: Vec<Complex<f32>>,
    retimed: Vec<Complex<f32>>,
}

impl EnvelopeDemod {
    /// `receive_filter` is the entry's matched filter as unit-energy taps, as in
    /// [`LinearDemod::new`](super::LinearDemod::new).
    ///
    /// # Panics
    /// If `receive_filter` is empty or not unit-energy, or the table has no energy.
    #[must_use]
    pub fn new(
        params: &LinearParams,
        receive_filter: &[f32],
        timing_bw: f64,
        timing: EnvelopeTiming,
    ) -> Self {
        assert!(!receive_filter.is_empty(), "receive filter must have taps");
        let energy: f64 = receive_filter
            .iter()
            .map(|&h| f64::from(h) * f64::from(h))
            .sum();
        assert!(
            (energy - 1.0).abs() < 1e-3,
            "receive filter must be unit-energy (pulse::Norm::Energy), got Σh² = {energy}"
        );
        let amplitudes: Vec<f32> = params
            .constellation()
            .points()
            .iter()
            .map(|p| p.norm())
            .collect();
        let largest = amplitudes.iter().copied().fold(0.0f32, f32::max);
        assert!(
            largest > 0.0,
            "an all-origin table has no amplitude to scale to"
        );
        let n = amplitudes.len() as f32;
        let table_mean = amplitudes.iter().sum::<f32>() / n;
        let table_sd = (amplitudes
            .iter()
            .map(|a| (a - table_mean) * (a - table_mean))
            .sum::<f32>()
            / n)
            .sqrt();
        assert!(
            table_sd > 0.0,
            "a constant-modulus table carries no amplitude for an envelope tier to detect"
        );
        let quietest = amplitudes.iter().copied().fold(f32::MAX, f32::min);
        Self {
            matched: Decimator::new(receive_filter, 1),
            sync: SymbolSync::new(params.sps() as f64, timing_bw),
            dc: 0.0,
            dc_alpha: one_pole_coeff(params.sps() as f64, timing.dc_symbols),
            dc_primed: false,
            levels: LevelTracker::new(timing, table_mean, table_sd),
            quietest,
            filtered: Vec::new(),
            magnitude: Vec::new(),
            retimed: Vec::new(),
        }
    }

    /// Demodulate a block, appending one soft amplitude per symbol period to `out`.
    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<f32>) {
        self.matched.process(iq, &mut self.filtered);
        self.magnitude.clear();
        self.magnitude.reserve(self.filtered.len());
        for s in &self.filtered {
            let envelope = s.norm();
            if self.dc_primed {
                self.dc += self.dc_alpha * (envelope - self.dc);
            } else {
                // Seeded from the first sample rather than from zero: a cold estimate climbing
                // out of zero is a pedestal step, and the timing loop would chase it.
                self.dc = envelope;
                self.dc_primed = true;
            }
            self.magnitude.push(Complex::new(envelope - self.dc, 0.0));
        }
        // `SymbolSync::process` appends; the scratch buffer is cleared per block (see
        // `LinearDemod::process`).
        self.retimed.clear();
        self.sync.process(&self.magnitude, &mut self.retimed);
        for y in &self.retimed {
            // A tracker still being seeded has no scale to report; the table's quietest
            // amplitude is the honest reading, and it is what an unkeyed channel is.
            out.push(self.levels.push(y.re).unwrap_or(self.quietest));
        }
    }

    /// Ratio of the tracked mean envelope to the fitted pedestal — a caller's carrier-present
    /// test. On a dead channel the levels collapse onto the pedestal and the ratio sits at 1.
    #[must_use]
    pub fn level_snr(&self) -> f32 {
        self.levels.snr()
    }

    pub fn reset(&mut self) {
        self.sync.reset();
        self.levels.reset();
        self.dc = 0.0;
        self.dc_primed = false;
    }
}

/// The label of the table point nearest a real soft amplitude — the hard decision of this tier.
/// Distance is taken on |point|, because that is all a magnitude detector measured; for the
/// unipolar tables this row covers, the amplitude ordering *is* the table.
#[must_use]
pub fn slice_amplitude(table: &Constellation, amplitude: f32) -> u32 {
    let mut best = 0usize;
    let mut best_d = f64::INFINITY;
    for (i, p) in table.points().iter().enumerate() {
        let d = (f64::from(amplitude) - f64::from(p.norm())).abs();
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    table.labels()[best]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ber::{
            impair::{Awgn, Impairment},
            perf::assert_no_alloc,
            rng::Rng,
        },
        constellation::tables,
        linear::LinearMod,
        pulse::{self, Norm},
    };

    const SPS: usize = 8;

    fn rrc() -> Vec<f32> {
        pulse::root_raised_cosine(SPS as f64, 0.35, 8, Norm::Energy)
    }

    fn labels(n: usize, m: u32, seed: u64) -> Vec<u32> {
        let mut rng = Rng::new(seed);
        (0..n)
            .map(|_| (rng.next_u64() % u64::from(m)) as u32)
            .collect()
    }

    fn demodulate(p: &LinearParams, wave: &[Complex<f32>]) -> Vec<f32> {
        let mut demod = EnvelopeDemod::new(p, &rrc(), 0.003, EnvelopeTiming::CONTINUOUS);
        let mut out = Vec::new();
        demod.process(wave, &mut out);
        out
    }

    /// Symbol offset between the demodulator's output and the transmitted stream: the pulse
    /// cascade's group delay, searched rather than derived so a filter-length change does not
    /// silently turn every test below into a comparison against the wrong symbols. The search
    /// is over a range far shorter than any real misalignment, and every caller then asserts an
    /// exact count, so a wrong answer here cannot hide a defect.
    fn alignment(out: &[f32], sent: &[u32], table: &Constellation) -> usize {
        let window = 400..1_200;
        (0..40)
            .filter(|off| off + window.end <= out.len())
            .min_by_key(|off| {
                window
                    .clone()
                    .filter(|&k| slice_amplitude(table, out[k + off]) != sent[k])
                    .count()
            })
            .expect("the demodulator returned too few symbols to align")
    }

    /// Mis-sliced symbols over the settled span, at the recovered alignment.
    fn errors(out: &[f32], sent: &[u32], table: &Constellation, from: usize) -> (usize, usize) {
        let off = alignment(out, sent, table);
        let last = sent.len().min(out.len() - off);
        let wrong = (from..last)
            .filter(|&k| slice_amplitude(table, out[k + off]) != sent[k])
            .count();
        (wrong, last - from)
    }

    /// Noiseless OOK through the whole tier: every settled symbol slices back to what was sent,
    /// with no carrier recovery anywhere in the chain.
    #[test]
    fn noiseless_ook_recovers_every_symbol() {
        let table = tables::ook().unwrap();
        let sent = labels(2_000, 2, 0x0009);
        let p = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
        let out = demodulate(&p, &LinearMod::transmission(&p, &sent));
        let (wrong, total) = errors(&out, &sent, &table, 400);
        assert_eq!(wrong, 0, "{wrong} of {total} mis-sliced");
    }

    /// Four unipolar levels, which is what makes this a tier and not an OOK special case: the
    /// fitted scale has to place three interior thresholds, not one.
    ///
    /// It does not reach zero errors on a noiseless signal, and the reason is the tier's own,
    /// not the tracker's: `|·|` folds the negative lobes of the shaped waveform, which for a
    /// multilevel unipolar stream leaves an additive self-noise of ~0.085 RMS against a 0.267
    /// slicing margin — measured here, level by level, and *flat across the levels*, which is
    /// what says it is additive rather than a gain error. Half a percent of symbols is what that
    /// costs. This is why the catalog measures the M-ASK row on the coherent tier and keeps the
    /// envelope tier for OOK, where the margin is half the full scale and the fold costs nothing.
    #[test]
    fn noiseless_4ask_carries_the_tiers_own_self_noise() {
        let table = tables::ask(4).unwrap();
        let sent = labels(2_000, 4, 0x4a54);
        let p = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
        let out = demodulate(&p, &LinearMod::transmission(&p, &sent));
        let (wrong, total) = errors(&out, &sent, &table, 400);
        assert!(
            wrong * 100 < total,
            "{wrong} of {total} mis-sliced — past the measured self-noise floor"
        );
        assert!(
            wrong > 0,
            "the self-noise floor vanished; re-measure the bound"
        );
    }

    /// The tier's defining claim, in its exact form: an unknown *phase* costs literally nothing,
    /// because the magnitude operator does not see one. Bit-identical output, not merely close.
    #[test]
    fn an_unknown_carrier_phase_costs_the_envelope_tier_nothing() {
        let table = tables::ook().unwrap();
        let sent = labels(1_500, 2, 0x0009);
        let p = LinearParams::new(table, rrc(), SPS).unwrap();
        let clean = LinearMod::transmission(&p, &sent);
        let rot = Complex::new(0.9f64.cos() as f32, 0.9f64.sin() as f32);
        let rotated: Vec<Complex<f32>> = clean.iter().map(|&s| s * rot).collect();
        let a = demodulate(&p, &clean);
        let b = demodulate(&p, &rotated);
        let worst = a
            .iter()
            .zip(&b)
            .map(|(x, y)| f64::from((x - y).abs()))
            .fold(0.0, f64::max);
        assert!(worst < 1e-5, "a static phase moved the envelope by {worst}");
    }

    /// A *frequency* offset is not free, and the honest statement is why: the matched filter is
    /// a real filter, so sliding the signal across its skirt reshapes the pulse and with it the
    /// ISI the magnitude folds. At 0.024 cycles/symbol — an offset that leaves a coherent
    /// receiver's loop with real work to do — the cost is a few percent of the slicing margin
    /// and no symbol errors at all.
    #[test]
    fn a_frequency_offset_costs_the_envelope_tier_only_the_filter_skirt() {
        let table = tables::ook().unwrap();
        let sent = labels(1_500, 2, 0x0009);
        let p = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
        let clean = LinearMod::transmission(&p, &sent);
        let mut shifted = clean.clone();
        for (n, s) in shifted.iter_mut().enumerate() {
            let theta = std::f64::consts::TAU * 3e-3 * n as f64;
            *s *= Complex::new(theta.cos() as f32, theta.sin() as f32);
        }
        let a = demodulate(&p, &clean);
        let b = demodulate(&p, &shifted);
        let worst = a
            .iter()
            .zip(&b)
            .map(|(x, y)| f64::from((x - y).abs()))
            .fold(0.0, f64::max);
        let margin = f64::from(table.points()[1].norm()) / 2.0;
        assert!(worst < 0.1 * margin, "offset moved the envelope by {worst}");
        let (wrong, total) = errors(&b, &sent, &table, 400);
        assert_eq!(wrong, 0, "{wrong} of {total} mis-sliced under the offset");
    }

    /// The adaptive part: a level step mid-transmission — the front-end gain change every real
    /// receiver sees — is followed, where a fixed threshold at half the original amplitude would
    /// slice the whole second half as off.
    #[test]
    fn the_threshold_follows_a_level_step() {
        let table = tables::ook().unwrap();
        let sent = labels(4_000, 2, 0x57e9);
        let p = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
        let mut wave = LinearMod::transmission(&p, &sent);
        let step_at = wave.len() / 2;
        for s in &mut wave[step_at..] {
            *s *= 0.25;
        }
        let out = demodulate(&p, &wave);
        // Judge past the step plus the tracker's own 200-symbol settling.
        let (wrong, total) = errors(&out, &sent, &table, sent.len() / 2 + 400);
        assert_eq!(wrong, 0, "{wrong} of {total} mis-sliced after the step");
    }

    /// The fitted pedestal is what makes the threshold right under noise: an OOK stream at a
    /// modest SNR still slices essentially perfectly, where a threshold pinned at half the peak
    /// would sit below the Rayleigh mean of the off symbols and key on noise.
    #[test]
    fn the_fitted_pedestal_survives_noise() {
        let table = tables::ook().unwrap();
        let sent = labels(4_000, 2, 0x0f100);
        let p = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
        let mut wave = LinearMod::transmission(&p, &sent);
        // Es/N0 = 13 dB on a unit-mean-energy table: total noise variance 2σ² = 10^-1.3.
        Awgn::with_sigma((0.5 * 10f64.powf(-1.3)).sqrt()).apply(&mut wave, &mut Rng::new(0x9e));
        let out = demodulate(&p, &wave);
        let (wrong, total) = errors(&out, &sent, &table, 400);
        assert!(
            wrong * 200 < total,
            "{wrong} of {total} mis-sliced at 13 dB"
        );
    }

    /// A constant-modulus table has no amplitude to detect, and the tier says so at construction
    /// rather than dividing by a zero scale later.
    #[test]
    fn a_constant_modulus_table_is_refused() {
        let p = LinearParams::new(tables::psk(4).unwrap(), rrc(), SPS).unwrap();
        let built = std::panic::catch_unwind(|| {
            EnvelopeDemod::new(&p, &rrc(), 0.003, EnvelopeTiming::CONTINUOUS)
        });
        assert!(built.is_err());
    }

    #[test]
    fn slicing_reads_the_table_by_amplitude() {
        let table = tables::ask(4).unwrap();
        let mut amplitudes: Vec<f32> = table.points().iter().map(|p| p.norm()).collect();
        amplitudes.sort_by(f32::total_cmp);
        for (i, &a) in amplitudes.iter().enumerate() {
            assert_eq!(slice_amplitude(&table, a), tables::gray(i as u32));
        }
        // Between two levels, the nearer wins; past the top, the top does.
        let mid = 0.5 * (amplitudes[0] + amplitudes[1]);
        assert_eq!(slice_amplitude(&table, mid - 0.01), tables::gray(0));
        assert_eq!(slice_amplitude(&table, mid + 0.01), tables::gray(1));
        assert_eq!(slice_amplitude(&table, 99.0), tables::gray(3));
    }

    /// §4.2: the steady-state path allocates nothing.
    #[test]
    fn steady_state_allocates_nothing() {
        let p = LinearParams::new(tables::ask(4).unwrap(), rrc(), SPS).unwrap();
        let wave = LinearMod::transmission(&p, &labels(2_048, 4, 0x0a12));
        let mut demod = EnvelopeDemod::new(&p, &rrc(), 0.003, EnvelopeTiming::CONTINUOUS);
        let mut out = Vec::with_capacity(wave.len());
        demod.process(&wave, &mut out);
        out.clear();
        demod.process(&wave, &mut out);
        out.clear();
        assert_no_alloc("EnvelopeDemod::process", || {
            demod.process(&wave, &mut out);
        });
        assert!(!out.is_empty());
    }
}
