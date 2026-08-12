//! What the receiver knows about the channel, and how it keeps knowing it: the least-squares
//! estimate over known symbols, the interpolation that fills the bins the training did not
//! reach, the one-tap equaliser, and the pilot tracker that follows what drifts within a frame.
//!
//! The one-tap equaliser is the reason OFDM exists, and it is only correct because the prefix is
//! cyclic: a delay spread inside the prefix turns the channel into a per-bin complex gain, and
//! dividing by that gain is the whole equaliser. Everything in this module is therefore about
//! *how well the gain is known*, and every part of that is measured — the estimation cost as a
//! committed BER margin against a genie receiver, and the estimator's own error as a closed form
//! (`σ²/2` per bin from two averaged repeats, asserted rather than asserted-to).
//!
//! **Two estimators, because they fail differently.** The long training energises every occupied
//! bin, so it resolves any channel the prefix can carry. The short training energises a *comb* —
//! every stride-th bin — which buys 6.4 dB of estimator SNR at the reference geometry (its
//! energy is concentrated on a twelfth of the band and repeated ten times) and pays for it with
//! the delay-domain Nyquist limit a comb has: a stride-4 comb can only represent a channel
//! shorter than `fft/4` samples, and past that it aliases. Both are committed, and the pair is
//! what makes "channel-estimation error" a number rather than an adjective.

use num_complex::Complex;

use super::params::SubcarrierMap;

/// The smallest noise variance an estimate reports (see [`ChannelEstimate::finish`]).
pub const MIN_NOISE_VAR: f64 = 1e-12;

/// Which known symbols the channel estimate is formed from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelEstimator {
    /// The long training's repeats: every occupied bin estimated directly, nothing
    /// interpolated. Tier 1.
    LongTraining,
    /// The short training's comb, linearly interpolated across the band — the estimate a
    /// receiver already holds when the long training arrives. Tier 2, measured against tier 1.
    ShortComb,
}

/// A per-bin channel estimate and everything derived from it, sized once at construction so the
/// receive path never allocates.
#[derive(Clone, Debug)]
pub struct ChannelEstimate {
    inv: Vec<Complex<f32>>,
    gain: Vec<f32>,
    h: Vec<Complex<f32>>,
    known: Vec<bool>,
    noise_var: f64,
}

impl ChannelEstimate {
    #[must_use]
    pub fn new(fft: usize) -> Self {
        Self {
            inv: vec![Complex::new(0.0, 0.0); fft],
            gain: vec![0.0; fft],
            h: vec![Complex::new(0.0, 0.0); fft],
            known: vec![false; fft],
            noise_var: 0.0,
        }
    }

    /// Forgets everything: every bin unknown, every gain zero. Called at the start of an
    /// acquisition so a frame can never be equalised with the previous frame's channel.
    pub fn clear(&mut self) {
        self.h.fill(Complex::new(0.0, 0.0));
        self.inv.fill(Complex::new(0.0, 0.0));
        self.gain.fill(0.0);
        self.known.fill(false);
        self.noise_var = 0.0;
    }

    /// Records one bin's estimate.
    pub fn set(&mut self, bin: usize, h: Complex<f32>) {
        self.h[bin] = h;
        self.known[bin] = true;
    }

    /// Fills the occupied bins no [`set`](Self::set) reached, then derives the reciprocals and
    /// gains the equaliser and the demapper read. Every estimate must be in place first: this is
    /// the point the estimate becomes usable, and nothing after it changes.
    ///
    /// `ramp_cycles_per_bin` is a phase ramp the caller *knows* is in its estimates — a transform
    /// window deliberately placed early in the prefix puts one there — and it is removed before
    /// the interpolation and restored after. The distinction is not cosmetic: interpolation is a
    /// straight line between anchors, a ramp is a rotation between them, and at the reference
    /// geometry a four-sample backoff turns a quarter cycle between two comb anchors, where a
    /// chord is 29 % short of the arc. Interpolating the *channel* rather than the channel times
    /// a known rotation is what keeps the comb tier usable at all.
    pub fn finish(&mut self, map: &SubcarrierMap, noise_var: f64, ramp_cycles_per_bin: f64) {
        if ramp_cycles_per_bin != 0.0 {
            self.rotate(map, -ramp_cycles_per_bin);
        }
        interpolate(map, &self.known, &mut self.h);
        if ramp_cycles_per_bin != 0.0 {
            self.rotate(map, ramp_cycles_per_bin);
        }
        for c in map.occupied() {
            let h = self.h[c.bin];
            let gain = h.norm_sqr();
            self.gain[c.bin] = gain;
            // A bin the channel nulled has no information to recover and dividing by it would
            // manufacture confident noise; it equalises to zero, which the demapper reads as an
            // erasure rather than as a wrong symbol.
            self.inv[c.bin] = if gain > 0.0 {
                h.conj() / gain
            } else {
                Complex::new(0.0, 0.0)
            };
        }
        // Floored, because the demapper's contract is a *positive* variance and a synthetic
        // frame with no noise at all measures exactly zero. 1e-12 against a unit symbol energy is
        // 120 dB of SNR — past anything two training repeats could resolve, and short of the
        // arithmetic a literal zero would produce downstream.
        self.noise_var = noise_var.max(MIN_NOISE_VAR);
    }

    /// Turns every occupied bin's estimate by `cycles · offset`.
    fn rotate(&mut self, map: &SubcarrierMap, cycles_per_bin: f64) {
        for c in map.occupied() {
            let phase = std::f64::consts::TAU * cycles_per_bin * f64::from(c.offset);
            let (sin, cos) = phase.sin_cos();
            let h = self.h[c.bin];
            self.h[c.bin] = Complex::new(
                (f64::from(h.re) * cos - f64::from(h.im) * sin) as f32,
                (f64::from(h.re) * sin + f64::from(h.im) * cos) as f32,
            );
        }
    }

    #[must_use]
    pub fn h(&self, bin: usize) -> Complex<f32> {
        self.h[bin]
    }

    /// `|H|²` — the pre-equalisation SNR weighting, which is what the pilot fit weights by and
    /// what the post-equalisation noise variance divides by.
    #[must_use]
    pub fn gain(&self, bin: usize) -> f32 {
        self.gain[bin]
    }

    /// The one tap: `Y/H`.
    #[must_use]
    pub fn equalize(&self, bin: usize, y: Complex<f32>) -> Complex<f32> {
        y * self.inv[bin]
    }

    /// Noise variance per FFT bin, before equalisation — measured, not assumed (see
    /// [`noise_var_from_repeats`]).
    #[must_use]
    pub fn noise_var(&self) -> f64 {
        self.noise_var
    }

    /// Noise variance *after* the one tap: `N0/|H|²`. This is what makes an OFDM LLR a true LLR
    /// under a frequency-selective channel — a deeply faded bin's symbol is not merely noisier,
    /// it must be *believed less*, and the shared demapper takes exactly this number.
    #[must_use]
    pub fn bin_noise_var(&self, bin: usize) -> f64 {
        let gain = f64::from(self.gain[bin]);
        if gain > 0.0 {
            self.noise_var / gain
        } else {
            f64::INFINITY
        }
    }
}

/// Fills every occupied bin left unknown by linear interpolation in the *offset* axis between
/// its known neighbours, extending flat past the outermost known bin.
///
/// Linear in the complex plane rather than in magnitude and phase separately: the two agree
/// wherever the interpolation is worth doing (neighbouring known bins that differ by little) and
/// the polar form has a branch cut where they do not. A ramp the caller *knows* about is removed
/// first — see [`ChannelEstimate::finish`].
///
/// Walks the map rather than collecting its anchors, so the receive path this sits on stays
/// allocation-free (§4.2).
pub fn interpolate(map: &SubcarrierMap, known: &[bool], h: &mut [Complex<f32>]) {
    let occupied = map.occupied();
    let Some(first) = occupied.iter().position(|c| known[c.bin]) else {
        return;
    };
    let Some(last) = occupied.iter().rposition(|c| known[c.bin]) else {
        return;
    };
    for i in 0..first {
        h[occupied[i].bin] = h[occupied[first].bin];
    }
    for i in last + 1..occupied.len() {
        h[occupied[i].bin] = h[occupied[last].bin];
    }
    let mut lo = first;
    while lo < last {
        let hi = (lo + 1..=last)
            .find(|&j| known[occupied[j].bin])
            .unwrap_or(last);
        let (a, b) = (occupied[lo], occupied[hi]);
        let span = f64::from(b.offset - a.offset);
        for c in &occupied[lo + 1..hi] {
            let t = (f64::from(c.offset - a.offset) / span) as f32;
            h[c.bin] = h[a.bin] * (1.0 - t) + h[b.bin] * t;
        }
        lo = hi;
    }
}

/// Noise variance per bin from two repeats of the same known symbol: their difference is noise
/// alone, of twice the per-bin variance, whatever the channel did to the signal in between.
///
/// This is the §3.4 hook paying for itself twice — the same repeats that give the channel
/// estimate give the noise variance the LLRs need, and neither number is a parameter anyone has
/// to supply.
#[must_use]
pub fn noise_var_from_repeats(first: &[Complex<f32>], second: &[Complex<f32>]) -> f64 {
    if first.is_empty() {
        return 0.0;
    }
    let sum: f64 = first
        .iter()
        .zip(second)
        .map(|(a, b)| f64::from((a - b).norm_sqr()))
        .sum();
    sum / (2.0 * first.len() as f64)
}

/// One symbol's residual phase, as a line across the band: a constant term (what a residual
/// carrier offset leaves per symbol) and a slope in subcarrier offset (what a sampling-instant
/// error leaves — a cyclic shift of τ samples is a phase ramp of 2πτ/N per bin).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PilotFit {
    pub common_rad: f64,
    pub slope_rad_per_bin: f64,
}

impl PilotFit {
    #[must_use]
    pub fn phase_at(&self, offset: i32) -> f64 {
        self.common_rad + self.slope_rad_per_bin * f64::from(offset)
    }
}

/// Position gain of the α-β filter below. See [`PilotTracker`] for why the tracker is filtered
/// at all and what the value was chosen against.
pub const TRACK_ALPHA: f64 = 0.35;

/// Rate gain, at the Benedict–Bordner optimum for this α (`β = α²/(2−α)`): the pairing that
/// minimises the tracker's own output noise for a given transient response, and the one that
/// keeps it critically damped rather than ringing along a ramp.
pub const TRACK_BETA: f64 = TRACK_ALPHA * TRACK_ALPHA / (2.0 - TRACK_ALPHA);

/// Tracks the residual phase across a frame's symbols: a line per symbol, filtered across
/// symbols.
///
/// **The state is what makes the fit unwrappable.** A weighted line through four pilot phases can
/// only resolve them modulo 2π, and a sampling-clock error walks the slope *past* that ambiguity
/// within a frame — at the reference geometry a 3000 ppm clock has moved the sampling instant by
/// 16 samples by the end of a 64-symbol frame, five times the ±2.3 samples one symbol's pilots
/// could unwrap. Fitting each symbol about the *predicted* line rather than about zero turns that
/// into a per-symbol increment of a fraction of a radian.
///
/// **The filtering is what makes it cheap.** A raw per-symbol fit is four noisy phases turned into
/// two parameters, and the parameters are then extrapolated to the band edge, which amplifies:
/// measured on the reference chain, an unfiltered tracker's own noise was worth ~0.47σ² at
/// subcarrier ±26 — comparable to the channel noise it was correcting for, and about 1.2 dB of
/// the acquiring rows' distance from theory. So the line is not taken per symbol but *tracked*:
/// an α-β filter on (common phase, slope), each with its own per-symbol rate, which is unbiased
/// along a ramp — the shape a clock error actually produces — where plain exponential smoothing
/// would lag it.
#[derive(Clone, Debug, Default)]
pub struct PilotTracker {
    state: PilotFit,
    rate: PilotFit,
    symbols: usize,
}

impl PilotTracker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets the previous frame — the first symbol of a new frame has no predecessor and must
    /// be fitted about zero.
    pub fn reset(&mut self) {
        self.state = PilotFit::default();
        self.rate = PilotFit::default();
        self.symbols = 0;
    }

    /// Fits one symbol's pilots and returns the line to remove from that symbol's data bins.
    ///
    /// `offsets`, `errors` and `weights` are parallel per pilot: `errors[i]` is the equalised
    /// pilot divided by the value it should have carried, so its angle *is* the residual phase,
    /// and `weights[i]` is that pilot's `|H|²` — a deeply faded pilot's phase is nearly noise and
    /// must not drag the line.
    pub fn fit(&mut self, offsets: &[i32], errors: &[Complex<f32>], weights: &[f32]) -> PilotFit {
        let predicted = PilotFit {
            common_rad: self.state.common_rad + self.rate.common_rad,
            slope_rad_per_bin: self.state.slope_rad_per_bin + self.rate.slope_rad_per_bin,
        };
        let Some(measured) = weighted_line(offsets, errors, weights, predicted) else {
            // No usable pilot this symbol: the prediction stands rather than the correction
            // collapsing to zero, which would rotate the symbol by whatever had accumulated.
            self.state = predicted;
            self.symbols += 1;
            return predicted;
        };
        // The first symbol initialises the state instead of being filtered into it: with no
        // history there is nothing to weigh a measurement against, and a state started at zero
        // would drag the frame's early symbols toward it.
        //
        // The *rate* starts at zero and is only ever built by the β term, which is the correction
        // to a first draft that seeded it from the difference of the first two symbols. That
        // draft's rate was one noise sample: measured on the reference chain at 6 dB, it sent the
        // prediction running at ~0.3 rad per symbol, the per-symbol unwrap then locked onto the
        // wrong branch, and 8 % of frames came back inverted end to end (BER ≈ 0.5 with a
        // *perfect* acquisition behind it). A rate that can only be learnt from repeated evidence
        // cannot do that.
        let fit = if self.symbols == 0 {
            self.rate = PilotFit::default();
            measured
        } else {
            let residual = PilotFit {
                common_rad: measured.common_rad - predicted.common_rad,
                slope_rad_per_bin: measured.slope_rad_per_bin - predicted.slope_rad_per_bin,
            };
            self.rate = PilotFit {
                common_rad: self.rate.common_rad + TRACK_BETA * residual.common_rad,
                slope_rad_per_bin: self.rate.slope_rad_per_bin
                    + TRACK_BETA * residual.slope_rad_per_bin,
            };
            PilotFit {
                common_rad: predicted.common_rad + TRACK_ALPHA * residual.common_rad,
                slope_rad_per_bin: predicted.slope_rad_per_bin
                    + TRACK_ALPHA * residual.slope_rad_per_bin,
            }
        };
        self.state = fit;
        self.symbols += 1;
        fit
    }

    /// The line the last [`fit`](Self::fit) settled on.
    #[must_use]
    pub fn last(&self) -> PilotFit {
        self.state
    }

    /// Symbols tracked since the last [`reset`](Self::reset).
    #[must_use]
    pub fn symbols(&self) -> usize {
        self.symbols
    }
}

/// The weighted least-squares line through one symbol's pilot phases, each phase unwrapped about
/// `predicted` — the measured angle is taken as the prediction plus whatever residual lies in
/// (−π, π], never as a bare `arg`. `None` when no pilot carries any weight.
fn weighted_line(
    offsets: &[i32],
    errors: &[Complex<f32>],
    weights: &[f32],
    predicted: PilotFit,
) -> Option<PilotFit> {
    let (mut sw, mut sx, mut sy, mut sxx, mut sxy) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
    for ((&offset, &error), &weight) in offsets.iter().zip(errors).zip(weights) {
        let w = f64::from(weight);
        if w <= 0.0 {
            continue;
        }
        let at = predicted.phase_at(offset);
        let residual =
            Complex::new(f64::from(error.re), f64::from(error.im)) * Complex::from_polar(1.0, -at);
        let phase = at + residual.arg();
        let x = f64::from(offset);
        sw += w;
        sx += w * x;
        sy += w * phase;
        sxx += w * x * x;
        sxy += w * x * phase;
    }
    if sw <= 0.0 {
        return None;
    }
    let det = sw * sxx - sx * sx;
    if det.abs() <= 1e-12 {
        // Pilots all at one offset (or a single pilot): a slope is not identifiable, so the
        // honest fit is a common phase alone.
        return Some(PilotFit {
            common_rad: sy / sw,
            slope_rad_per_bin: 0.0,
        });
    }
    Some(PilotFit {
        common_rad: (sxx * sy - sx * sxy) / det,
        slope_rad_per_bin: (sw * sxy - sx * sy) / det,
    })
}

#[cfg(test)]
mod tests {
    use std::f64::consts::{PI, TAU};

    use super::*;
    use crate::ofdm::params::OfdmParams;

    fn map() -> SubcarrierMap {
        OfdmParams::wifi_like().map().clone()
    }

    /// A linear channel sampled on a comb must come back linear everywhere — the property the
    /// comb estimator's whole tier rests on.
    #[test]
    fn interpolation_reproduces_a_linear_channel_exactly() {
        let map = map();
        let truth = |offset: i32| Complex::new(1.0 + 0.01 * offset as f32, 0.02 * offset as f32);
        let mut h = vec![Complex::new(0.0, 0.0); map.fft()];
        let mut known = vec![false; map.fft()];
        for c in map.occupied().iter().filter(|c| c.offset % 4 == 0) {
            h[c.bin] = truth(c.offset);
            known[c.bin] = true;
        }
        interpolate(&map, &known, &mut h);
        // Inside the comb's span only: past the outermost anchor there is nothing to interpolate
        // between, which is the next test's subject.
        for c in map.occupied().iter().filter(|c| c.offset.abs() <= 24) {
            let want = truth(c.offset);
            assert!(
                (h[c.bin] - want).norm() < 1e-5,
                "bin {} ({}): {:?} vs {want:?}",
                c.bin,
                c.offset,
                h[c.bin]
            );
        }
    }

    /// Outside the comb's own span there is nothing to interpolate between, so the estimate
    /// extends flat rather than running off along the last slope it saw.
    #[test]
    fn interpolation_extends_flat_past_the_outermost_anchor() {
        let map = map();
        let mut h = vec![Complex::new(0.0, 0.0); map.fft()];
        let mut known = vec![false; map.fft()];
        for c in map.occupied().iter().filter(|c| c.offset.abs() <= 4) {
            h[c.bin] = Complex::new(c.offset as f32, 0.0);
            known[c.bin] = true;
        }
        interpolate(&map, &known, &mut h);
        let edge = |offset: i32| {
            h[map
                .occupied()
                .iter()
                .find(|c| c.offset == offset)
                .unwrap()
                .bin]
        };
        assert!((edge(26) - edge(4)).norm() < 1e-6);
        assert!((edge(-26) - edge(-4)).norm() < 1e-6);
        // Nothing known at all leaves the estimate untouched rather than inventing one.
        let mut empty = vec![Complex::new(0.0, 0.0); map.fft()];
        interpolate(&map, &vec![false; map.fft()], &mut empty);
        assert!(empty.iter().all(|v| *v == Complex::new(0.0, 0.0)));
    }

    /// The estimator's own error, against its closed form: two averaged repeats of a known
    /// symbol leave `σ²/2` per bin. This is the channel-estimation error curve as an identity —
    /// the BER cost it produces is measured separately, as a margin against a genie receiver.
    #[test]
    fn averaged_repeats_leave_half_the_noise_variance() {
        use crate::ber::rng::Rng;
        let mut rng = Rng::new(0x1e5);
        for &sigma2 in &[0.01f64, 0.1, 1.0] {
            let sigma = (sigma2 / 2.0).sqrt();
            let draw = |rng: &mut Rng| {
                Complex::new((rng.normal() * sigma) as f32, (rng.normal() * sigma) as f32)
            };
            let n = 20_000;
            let (mut first, mut second) = (Vec::with_capacity(n), Vec::with_capacity(n));
            let mut error = 0.0f64;
            for _ in 0..n {
                // The truth is 1 on every bin; the two repeats see independent noise.
                let (a, b) = (
                    Complex::new(1.0, 0.0) + draw(&mut rng),
                    Complex::new(1.0, 0.0) + draw(&mut rng),
                );
                let estimate = (a + b) * 0.5;
                error += f64::from((estimate - Complex::new(1.0, 0.0)).norm_sqr());
                first.push(a);
                second.push(b);
            }
            let measured_var = noise_var_from_repeats(&first, &second);
            assert!(
                (measured_var / sigma2 - 1.0).abs() < 0.05,
                "σ² {sigma2}: measured {measured_var}"
            );
            let mse = error / n as f64;
            assert!(
                (mse / (sigma2 / 2.0) - 1.0).abs() < 0.05,
                "σ² {sigma2}: estimator MSE {mse}, closed form {}",
                sigma2 / 2.0
            );
        }
    }

    /// The fit recovers a line it was given, and the weights are honoured: a pilot at zero gain
    /// contributes nothing, so one broken pilot cannot rotate a symbol.
    #[test]
    fn the_pilot_fit_recovers_a_line_and_ignores_dead_pilots() {
        let offsets = [-21i32, -7, 7, 21];
        let truth = PilotFit {
            common_rad: 0.3,
            slope_rad_per_bin: 0.01,
        };
        let errors: Vec<Complex<f32>> = offsets
            .iter()
            .map(|&k| {
                let p = truth.phase_at(k);
                Complex::new(p.cos() as f32, p.sin() as f32)
            })
            .collect();
        let mut tracker = PilotTracker::new();
        let fit = tracker.fit(&offsets, &errors, &[1.0; 4]);
        assert!((fit.common_rad - truth.common_rad).abs() < 1e-6, "{fit:?}");
        assert!((fit.slope_rad_per_bin - truth.slope_rad_per_bin).abs() < 1e-9);

        let mut spoiled = errors.clone();
        spoiled[0] = Complex::new(-1.0, 0.0);
        tracker.reset();
        let fit = tracker.fit(&offsets, &spoiled, &[0.0, 1.0, 1.0, 1.0]);
        assert!((fit.common_rad - truth.common_rad).abs() < 1e-6, "{fit:?}");
    }

    /// The tracking claim: a slope that walks past the pilots' own ±π ambiguity is followed when
    /// each symbol is fitted about the previous prediction, and lost when it is not. The step here
    /// is the one a 3000 ppm clock produces at the reference geometry, and the residual is read at
    /// the *data* subcarriers — a correction that happens to be right where it was fitted and
    /// wrong at the band edge is still a wrong correction.
    ///
    /// Two numbers, because the filter has two regimes: a transient while the β term learns the
    /// ramp's rate from repeated evidence — bounded well inside half a turn, which is what keeps
    /// the unwrap on the right branch — and a steady state, where an α-β filter's lag along a
    /// ramp is zero by construction.
    #[test]
    fn tracking_follows_a_slope_past_the_unwrapping_ambiguity() {
        let pilots = [-21i32, -7, 7, 21];
        let step = TAU * 0.24 / 64.0; // 0.24 samples of walk per symbol
        let wrap = |e: f64| (e + PI).rem_euclid(TAU) - PI;
        let mut tracked = PilotTracker::new();
        let mut untracked = PilotTracker::new();
        let (mut transient, mut steady, mut worst_untracked) = (0.0f64, 0.0f64, 0.0f64);
        for symbol in 0..64 {
            let truth = PilotFit {
                common_rad: 0.0,
                slope_rad_per_bin: step * f64::from(symbol),
            };
            let errors: Vec<Complex<f32>> = pilots
                .iter()
                .map(|&k| {
                    let p = truth.phase_at(k);
                    Complex::new(p.cos() as f32, p.sin() as f32)
                })
                .collect();
            let a = tracked.fit(&pilots, &errors, &[1.0; 4]);
            untracked.reset();
            let b = untracked.fit(&pilots, &errors, &[1.0; 4]);
            let residual = |fit: PilotFit| {
                (-26..=26)
                    .map(|k| wrap(fit.phase_at(k) - truth.phase_at(k)).abs())
                    .fold(0.0f64, f64::max)
            };
            transient = transient.max(residual(a));
            if symbol >= 32 {
                steady = steady.max(residual(a));
            }
            worst_untracked = worst_untracked.max(residual(b));
        }
        assert!(transient < PI / 2.0, "transient residual {transient} rad");
        assert!(steady < 0.02, "steady-state residual {steady} rad");
        assert_eq!(tracked.symbols(), 64);
        assert!(
            worst_untracked > 1.0,
            "untracked residual {worst_untracked} rad — the ambiguity was never reached, so \
             this test is not measuring what it claims"
        );
    }

    /// A nulled bin equalises to zero and reports infinite noise, rather than dividing by
    /// almost-nothing and handing the demapper a confident wrong symbol.
    #[test]
    fn a_nulled_bin_erases_instead_of_amplifying() {
        let map = map();
        let mut estimate = ChannelEstimate::new(map.fft());
        for c in map.occupied() {
            estimate.set(c.bin, Complex::new(1.0, 0.0));
        }
        let dead = map.data()[3].bin;
        estimate.set(dead, Complex::new(0.0, 0.0));
        estimate.finish(&map, 0.01, 0.0);
        assert_eq!(
            estimate.equalize(dead, Complex::new(0.4, -0.2)),
            Complex::new(0.0, 0.0)
        );
        assert!(estimate.bin_noise_var(dead).is_infinite());
        let live = map.data()[4].bin;
        assert!((estimate.bin_noise_var(live) - 0.01).abs() < 1e-9);
    }
}
