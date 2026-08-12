//! Carrier recovery for the linear engine (MODEM-PLAN §6: "coherent (Costas/FLL/DD/pilot)").
//! One second-order loop — `sdrmm_dsp::LoopFilter`, the crate's only loop filter (§3.2) — driven
//! at *symbol* rate by a pluggable phase-error detector, optionally aided by a frequency
//! detector for cold acquisition. The pilot/known-symbol arm of that list is
//! [`anchor`](super::anchor), which is a block estimator rather than a loop and therefore lives
//! next door.
//!
//! **Two detectors, and why both.**
//!
//! - [`PhaseDetector::DecisionDirected`] is the generalised Costas loop: slice the received
//!   point against the table, and the residual angle between the two *is* the phase error. It
//!   works for any constellation — that is what makes it the QAM and APSK answer, where no
//!   power of the signal strips the modulation — but it needs the amplitude scale to be roughly
//!   right first (a QAM slice against a mis-scaled table decides the wrong ring), and it can
//!   only pull in from within the table's own angular decision region.
//! - [`PhaseDetector::MthPower`] raises the symbol to the M-th power, which annihilates an
//!   M-PSK modulation outright: no decisions, no amplitude dependence, so it acquires from a
//!   cold start at an SNR where decisions are still mostly wrong. It applies only to a
//!   constant-modulus M-PSK table.
//!
//! **Ambiguity is not resolved here, deliberately.** Both detectors are blind to a rotation by
//! a table symmetry — π for BPSK, π/2 for QPSK, 2π/M in general — because the modulation they
//! strip is exactly what that rotation preserves. Removing the ambiguity is the job of either
//! differential coding (the DPSK rows carry the data in the *difference*, so the absolute phase
//! never matters) or the known-symbol anchor. An entry that uses neither has an
//! M-fold-ambiguous receiver, and that is a property of the entry, not a defect here.
//!
//! **The frequency aid.** The PI loop's integrator already tracks a static offset, but its
//! *pull-in* is bounded by the phase detector's unambiguous range: past ±π/M of accumulated
//! error per symbol the detector reports the wrong sign and the loop walks away. The FLL
//! measures the M-th-power signal's rotation between consecutive symbols instead, which is
//! signed correctly across that whole range, and feeds a separate frequency integrator. It is
//! offered only with [`PhaseDetector::MthPower`], for the same reason the detector is: nothing
//! strips a QAM constellation. A QAM entry acquires frequency from its pilots — which is what
//! fielded QAM links do.

use std::f64::consts::{PI, TAU};

use num_complex::Complex;
use sdrmm_dsp::LoopFilter;

use crate::constellation::Constellation;

/// Loop damping. 1/√2 is the critically-flat second-order response every loop in the workspace
/// uses; the linear entries gave no measured reason to differ.
pub const DAMPING: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// Integrator clamp, in cycles per symbol. A quarter of a cycle per symbol is far past any
/// offset a receiver front end leaves behind (the catalog's worst committed CFO row is under
/// 0.02 cycles/symbol) and still bounds a noise burst from integrating the loop off the signal.
pub const FREQ_LIMIT_CYCLES_PER_SYMBOL: f64 = 0.25;

/// How the loop measures phase error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhaseDetector {
    /// Generalised Costas: the angle from the nearest table point to the received one.
    DecisionDirected,
    /// M-th power modulation stripping, for a constant-modulus M-PSK table.
    MthPower { m: u32 },
}

impl PhaseDetector {
    /// The phase error in radians — positive meaning the received point sits *ahead* of where
    /// the modulation says it should, so the loop must advance its de-rotation by that much.
    ///
    /// Returns zero for a symbol carrying no phase at all (an exact origin sample, which OOK
    /// produces on every off symbol): no vote is the honest reading, and it leaves the loop
    /// coasting on its integrator rather than jerking it with an arbitrary angle.
    #[must_use]
    pub fn error(self, y: Complex<f32>, table: &Constellation) -> f64 {
        if y.re == 0.0 && y.im == 0.0 {
            return 0.0;
        }
        match self {
            Self::DecisionDirected => {
                let label = table.hard_slice(y);
                let index = table.labels().iter().position(|&l| l == label);
                let Some(x) = index.map(|i| table.points()[i]) else {
                    return 0.0;
                };
                if x.re == 0.0 && x.im == 0.0 {
                    return 0.0;
                }
                f64::from((y * x.conj()).arg())
            }
            Self::MthPower { m } => {
                let mut u = Complex::new(1.0f64, 0.0);
                let y = Complex::new(f64::from(y.re), f64::from(y.im));
                // Unit-normalised before the power so a large-amplitude symbol cannot dominate
                // the running estimate, and so m = 8 cannot overflow the f32 range.
                let unit = y / y.norm();
                for _ in 0..m {
                    u *= unit;
                }
                u.arg() / f64::from(m)
            }
        }
    }
}

/// The tracking loop. Fed one symbol at a time; returns the de-rotated symbol.
#[derive(Clone, Debug)]
pub struct CarrierLoop {
    detector: PhaseDetector,
    filter: LoopFilter,
    /// De-rotation phase in radians, wrapped every symbol so an arbitrarily long transmission
    /// keeps f64 precision.
    phase: f64,
    /// Frequency-aid gain (0 disables), and the previous stripped symbol the differential
    /// detector needs.
    fll_gain: f64,
    fll_freq: f64,
    last_stripped: Option<Complex<f64>>,
}

impl CarrierLoop {
    /// `loop_bw` is the loop's natural frequency in cycles per symbol — the entry's data. The
    /// catalog's coherent rows are measured at 0.01 (fast enough to acquire inside a preamble,
    /// narrow enough that the loop's own phase jitter costs nothing at sensitivity); a
    /// slow-fading, high-order entry wants less.
    ///
    /// # Panics
    /// If `loop_bw` is not positive, or `MthPower` is asked for an order below 2.
    #[must_use]
    pub fn new(detector: PhaseDetector, loop_bw: f64) -> Self {
        if let PhaseDetector::MthPower { m } = detector {
            assert!(m >= 2, "M-th power stripping needs an order of at least 2");
        }
        Self {
            detector,
            filter: LoopFilter::new(loop_bw, DAMPING, FREQ_LIMIT_CYCLES_PER_SYMBOL),
            phase: 0.0,
            fll_gain: 0.0,
            fll_freq: 0.0,
            last_stripped: None,
        }
    }

    /// Adds the frequency aid at the given gain (cycles per symbol of correction per radian of
    /// measured per-symbol rotation). 0.01 is the catalog's operating point.
    ///
    /// # Panics
    /// If the detector is not [`PhaseDetector::MthPower`] — nothing strips a QAM constellation,
    /// so an FLL over one would be measuring the modulation.
    #[must_use]
    pub fn with_frequency_aid(mut self, gain: f64) -> Self {
        assert!(
            matches!(self.detector, PhaseDetector::MthPower { .. }),
            "the frequency aid needs a modulation-stripping detector"
        );
        assert!(
            gain >= 0.0 && gain.is_finite(),
            "FLL gain {gain} is not one"
        );
        self.fll_gain = gain;
        self
    }

    /// De-rotate one symbol and advance the loop. Zero allocation — this is the hot path of
    /// every coherent tier.
    #[must_use]
    pub fn advance(&mut self, y: Complex<f32>, table: &Constellation) -> Complex<f32> {
        let rot = Complex::new((-self.phase).cos() as f32, (-self.phase).sin() as f32);
        let out = y * rot;
        let error = self.detector.error(out, table);
        let mut inc = self.filter.advance(error);
        if self.fll_gain > 0.0 {
            inc += self.advance_frequency_aid(out);
        }
        self.phase = wrap(self.phase + inc);
        out
    }

    /// The frequency detector: how far the stripped symbol rotated since the last one, divided
    /// back down by M. Its own integrator, added to the phase loop's increment, so the two
    /// bands stay separable — the PI filter still owns steady-state phase.
    fn advance_frequency_aid(&mut self, y: Complex<f32>) -> f64 {
        let PhaseDetector::MthPower { m } = self.detector else {
            return 0.0;
        };
        if y.re == 0.0 && y.im == 0.0 {
            return self.fll_freq;
        }
        let y = Complex::new(f64::from(y.re), f64::from(y.im));
        let unit = y / y.norm();
        let mut stripped = Complex::new(1.0f64, 0.0);
        for _ in 0..m {
            stripped *= unit;
        }
        if let Some(previous) = self.last_stripped {
            let rotation = (stripped * previous.conj()).arg() / f64::from(m);
            self.fll_freq = (self.fll_freq + self.fll_gain * rotation).clamp(
                -TAU * FREQ_LIMIT_CYCLES_PER_SYMBOL,
                TAU * FREQ_LIMIT_CYCLES_PER_SYMBOL,
            );
        }
        self.last_stripped = Some(stripped);
        self.fll_freq
    }

    /// The loop's current frequency estimate in cycles per symbol — the phase filter's
    /// integrator plus the frequency aid's. Reading it is how a limits row states what a CFO
    /// search actually pulled in.
    #[must_use]
    pub fn freq_cycles_per_symbol(&self) -> f64 {
        self.filter.freq_norm() + self.fll_freq / TAU
    }

    pub fn reset(&mut self) {
        self.filter.reset(0.0);
        self.phase = 0.0;
        self.fll_freq = 0.0;
        self.last_stripped = None;
    }
}

/// Reduce a phase into `[-π, π)`. Exact for a phasor — `e^(jθ)` is 2π-periodic — so a single
/// wrap per symbol keeps the accumulator bounded with no drift of its own.
fn wrap(theta: f64) -> f64 {
    let t = (theta + PI).rem_euclid(TAU);
    t - PI
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ber::{perf::assert_no_alloc, rng::Rng},
        constellation::tables,
    };

    fn stream(m: u32, n: usize, seed: u64, table: &Constellation) -> Vec<Complex<f32>> {
        let mut rng = Rng::new(seed);
        (0..n)
            .map(|_| table.points()[(rng.next_u64() % u64::from(m)) as usize])
            .collect()
    }

    fn rotate(w: &mut [Complex<f32>], cycles_per_symbol: f64, phase0: f64) {
        for (k, s) in w.iter_mut().enumerate() {
            let theta = phase0 + TAU * cycles_per_symbol * k as f64;
            *s *= Complex::new(theta.cos() as f32, theta.sin() as f32);
        }
    }

    /// Worst per-symbol phase error over the settled tail, in turns, modulo the table symmetry
    /// no blind detector can resolve (`hard_slice` picks the nearest rotated hypothesis, so what
    /// is left is the error inside one ambiguity cell).
    ///
    /// Deliberately a *worst*, not a mean. A loop that never acquires and cycle-slips forever
    /// spreads its error uniformly around the circle, and its mean direction is therefore ~0 —
    /// the same number a perfectly locked loop reports. That coincidence is exactly the failure
    /// an acquisition test must not miss.
    fn worst_residual_turns(out: &[Complex<f32>], table: &Constellation) -> f64 {
        out[out.len() * 3 / 4..]
            .iter()
            .map(|&y| {
                let label = table.hard_slice(y);
                let i = table.labels().iter().position(|&l| l == label).unwrap();
                f64::from((y * table.points()[i].conj()).arg()).abs() / TAU
            })
            .fold(0.0, f64::max)
    }

    /// Both detectors must pull a static phase offset out of a clean stream. The M-th-power
    /// loop does it without ever slicing; the decision-directed one does it for a table no
    /// power strips — 16-QAM, where its ability to work at all is the whole point.
    #[test]
    fn both_detectors_acquire_a_static_phase_offset() {
        for (name, table, detector, m) in [
            (
                "qpsk mth-power",
                tables::psk(4).unwrap(),
                PhaseDetector::MthPower { m: 4 },
                4u32,
            ),
            (
                "qpsk decision-directed",
                tables::psk(4).unwrap(),
                PhaseDetector::DecisionDirected,
                4,
            ),
            (
                "16-qam decision-directed",
                tables::qam_square(16).unwrap(),
                PhaseDetector::DecisionDirected,
                4,
            ),
        ] {
            let mut wave = stream(table.len() as u32, 4_000, 0xca47, &table);
            rotate(&mut wave, 0.0, 0.7);
            let mut loop_ = CarrierLoop::new(detector, 0.01);
            let out: Vec<Complex<f32>> = wave.iter().map(|&y| loop_.advance(y, &table)).collect();
            let _ = m;
            let residual = worst_residual_turns(&out, &table);
            assert!(residual < 0.01, "{name}: worst residual {residual} turns");
        }
    }

    /// A static frequency offset is what the integrator is for: after settling, the loop's own
    /// frequency estimate must read the injected offset back.
    #[test]
    fn the_integrator_reads_back_a_static_frequency_offset() {
        let table = tables::psk(4).unwrap();
        let offset = 1.5e-3;
        let mut wave = stream(4, 6_000, 0xf5e9, &table);
        rotate(&mut wave, offset, 0.0);
        let mut loop_ = CarrierLoop::new(PhaseDetector::MthPower { m: 4 }, 0.01);
        let out: Vec<Complex<f32>> = wave.iter().map(|&y| loop_.advance(y, &table)).collect();
        assert!(worst_residual_turns(&out, &table) < 0.02);
        let measured = loop_.freq_cycles_per_symbol();
        assert!(
            (measured - offset).abs() < 0.1 * offset,
            "loop reads {measured}, injected {offset}"
        );
    }

    /// Symbols the loop takes to settle: the first index after which every de-rotated symbol
    /// stays within `tol` turns of a table point for a whole window, so one lucky symbol cannot
    /// declare lock.
    fn symbols_to_lock(out: &[Complex<f32>], table: &Constellation, tol: f64) -> Option<usize> {
        const WINDOW: usize = 200;
        let error = |y: Complex<f32>| {
            let label = table.hard_slice(y);
            let i = table.labels().iter().position(|&l| l == label).unwrap();
            f64::from((y * table.points()[i].conj()).arg()).abs() / TAU
        };
        let mut run = 0usize;
        for (k, &y) in out.iter().enumerate() {
            run = if error(y) < tol { run + 1 } else { 0 };
            if run == WINDOW {
                return Some(k + 1 - WINDOW);
            }
        }
        None
    }

    /// What the frequency aid buys, measured: acquisition of an offset the phase loop alone
    /// cannot reach. Past the detector's unambiguous range the phase error wraps faster than the
    /// filter can integrate, and the loop slips indefinitely; the FLL measures the stripped
    /// signal's rotation between symbols instead, which stays correctly signed across that whole
    /// range, so the loop starts from the right frequency rather than walking to it.
    ///
    /// Measured at 0.1 cycles/symbol against a 0.002 loop bandwidth: the plain loop cycle-slips
    /// for the whole 20 000-symbol run and never holds a lock, while the aided one settles inside
    /// a few thousand symbols. (A *mean* residual would have called the slipping loop locked —
    /// see [`worst_residual_turns`].)
    #[test]
    fn the_frequency_aid_acquires_far_sooner_than_the_phase_loop_alone() {
        let table = tables::psk(4).unwrap();
        let lock_after = |aided: bool| {
            let mut wave = stream(4, 20_000, 0x7011, &table);
            rotate(&mut wave, 0.1, 0.0);
            let base = CarrierLoop::new(PhaseDetector::MthPower { m: 4 }, 0.002);
            let mut loop_ = if aided {
                base.with_frequency_aid(0.01)
            } else {
                base
            };
            let out: Vec<Complex<f32>> = wave.iter().map(|&y| loop_.advance(y, &table)).collect();
            symbols_to_lock(&out, &table, 0.02)
        };
        let aided = lock_after(true).expect("the aided loop never locked");
        assert!(aided < 4_000, "the aided loop took {aided} symbols");
        assert_eq!(
            lock_after(false),
            None,
            "the plain loop acquired 0.1 cycles/symbol at a 0.002 loop bandwidth; \
             re-measure where the aid earns its keep"
        );
    }

    /// An origin sample is no vote, not an arbitrary angle: an OOK stream's off symbols must
    /// leave the loop where it was rather than driving it with the noise-free zero's phase.
    #[test]
    fn an_origin_symbol_casts_no_vote() {
        let table = tables::ook().unwrap();
        let mut loop_ = CarrierLoop::new(PhaseDetector::DecisionDirected, 0.05);
        let before = loop_.freq_cycles_per_symbol();
        for _ in 0..100 {
            let _ = loop_.advance(Complex::new(0.0, 0.0), &table);
        }
        assert_eq!(loop_.freq_cycles_per_symbol(), before);
        assert!(
            (PhaseDetector::DecisionDirected.error(Complex::new(0.0, 0.0), &table)).abs() < 1e-18
        );
    }

    #[test]
    fn reset_returns_the_loop_to_a_cold_start() {
        let table = tables::psk(4).unwrap();
        let mut wave = stream(4, 2_000, 0x1e5, &table);
        rotate(&mut wave, 1e-3, 0.4);
        let mut loop_ = CarrierLoop::new(PhaseDetector::MthPower { m: 4 }, 0.01);
        for &y in &wave {
            let _ = loop_.advance(y, &table);
        }
        assert!(loop_.freq_cycles_per_symbol().abs() > 1e-5);
        loop_.reset();
        assert_eq!(loop_.freq_cycles_per_symbol(), 0.0);
    }

    /// §4.2: the symbol-rate path of every coherent tier allocates nothing.
    #[test]
    fn the_loop_allocates_nothing() {
        let table = tables::qam_square(16).unwrap();
        let mut dd = CarrierLoop::new(PhaseDetector::DecisionDirected, 0.01);
        let mut mth =
            CarrierLoop::new(PhaseDetector::MthPower { m: 4 }, 0.01).with_frequency_aid(0.01);
        let y = Complex::new(0.7f32, 0.3);
        let psk = tables::psk(4).unwrap();
        let _ = dd.advance(y, &table);
        let _ = mth.advance(y, &psk);
        assert_no_alloc("CarrierLoop::advance decision-directed", || {
            std::hint::black_box(dd.advance(y, &table));
        });
        assert_no_alloc("CarrierLoop::advance mth-power + FLL", || {
            std::hint::black_box(mth.advance(y, &psk));
        });
    }
}
