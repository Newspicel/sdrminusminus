//! The reference linear transmitter: table lookup, per-symbol rotation, optional quadrature
//! stagger, pulse shaping, complex baseband out. This is *the* modulator behind every linear
//! catalog entry — `testgen` builds the demodulator's test signals from it and `tx.rs` drives it
//! as a signal generator, so modulator and demodulator can never drift apart ( §1.2).
//!
//! Energy accounting, stated once: the table is mean-Es = 1 and the pulse is unit energy, so a
//! block of n symbols carries n units of energy up to the pulse cascade's cross-terms — which is
//! exactly what makes an Eb/N0 set from measured waveform energy the textbook one. The stagger
//! does not disturb it (delaying one rail moves no energy) and neither does the rotation.
//!
//! Design is cold path: constructors allocate freely. The §4.2 zero-allocation gate binds the
//! demodulator's `process()`, not a test-signal generator.

use num_complex::Complex;

use super::params::LinearParams;

/// Linear modulator. Streaming: [`modulate`](Self::modulate) carries pulse and rotation state
/// across calls, so a transmission may be produced in blocks and any split gives the same
/// samples; [`flush`](Self::flush) drains the final symbols' pulse tail.
#[derive(Clone, Debug)]
pub struct LinearMod {
    params: LinearParams,
    /// Symbols accepted so far — the rotation schedule's index, kept as an integer so a long
    /// transmission's phase is exact rather than accumulated.
    symbols_in: u64,
    /// Shaped output not yet emitted: the pulse tail of the symbols already accepted, plus the
    /// stagger's half-symbol of quadrature. Held as samples because both are sample-domain.
    tail: Vec<Complex<f32>>,
}

impl LinearMod {
    #[must_use]
    pub fn new(params: LinearParams) -> Self {
        Self {
            params,
            symbols_in: 0,
            tail: Vec::new(),
        }
    }

    #[must_use]
    pub fn params(&self) -> &LinearParams {
        &self.params
    }

    /// The complex point symbol `k` is transmitted at: the table point for `label`, rotated by
    /// the entry's per-symbol schedule.
    #[must_use]
    pub fn point(&self, k: u64, label: u32) -> Complex<f32> {
        let table = self.params.constellation();
        // Table order is not label order in general, so the label is looked up rather than
        // indexed — the exotic tables' labels are a descent's output, not an enumeration.
        let index = table
            .labels()
            .iter()
            .position(|&l| l == label)
            .unwrap_or_default();
        let p = table.points()[index];
        if self.params.rotation_rad() == 0.0 {
            return p;
        }
        // The rotation is taken modulo a turn before it becomes an f32 phasor: `k` reaches
        // millions of symbols in a sweep, and k·π/4 in f32 would lose its low bits long before.
        let theta = (k as f64 * self.params.rotation_rad()) % std::f64::consts::TAU;
        p * Complex::new(theta.cos() as f32, theta.sin() as f32)
    }

    /// Modulate a block of constellation labels, appending complex baseband to `out`. State
    /// carries across calls; the last pulse span stays inside until more symbols — or
    /// [`flush`](Self::flush) — push it through.
    pub fn modulate(&mut self, labels: &[u32], out: &mut Vec<Complex<f32>>) {
        let sps = self.params.sps();
        let pulse = self.params.pulse();
        let stagger = self.params.stagger_samples();
        // Room for every new symbol's whole pulse plus the stagger, added onto what is already
        // held back from previous calls.
        let span = labels.len() * sps + pulse.len() + stagger;
        if self.tail.len() < span {
            self.tail.resize(span, Complex::new(0.0, 0.0));
        }
        for (j, &label) in labels.iter().enumerate() {
            let s = self.point(self.symbols_in + j as u64, label);
            let base = j * sps;
            for (m, &h) in pulse.iter().enumerate() {
                // The rails are shaped by the same pulse; the stagger is a pure delay on Q, so
                // it is applied as an index shift rather than a second filter.
                self.tail[base + m].re += s.re * h;
                self.tail[base + m + stagger].im += s.im * h;
            }
        }
        self.symbols_in += labels.len() as u64;
        // Everything before the newest symbol's pulse start is complete and may be emitted.
        let complete = labels.len() * sps;
        out.extend_from_slice(&self.tail[..complete]);
        self.tail.drain(..complete);
    }

    /// Push the held pulse tail out — a transmission that ends mid-tail hands the receiver's
    /// matched filter a truncated pulse it was not built for.
    pub fn flush(&mut self, out: &mut Vec<Complex<f32>>) {
        out.append(&mut self.tail);
    }

    /// Forget the streaming state. The parameters are immutable, so this is a full reset.
    pub fn reset(&mut self) {
        self.symbols_in = 0;
        self.tail.clear();
    }

    /// One complete transmission in one call — the shape the harness's links and `testgen` use.
    #[must_use]
    pub fn transmission(params: &LinearParams, labels: &[u32]) -> Vec<Complex<f32>> {
        let mut m = Self::new(params.clone());
        let mut out = Vec::with_capacity(labels.len() * params.sps() + params.pulse().len());
        m.modulate(labels, &mut out);
        m.flush(&mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ber::{impair::signal_energy, rng::Rng},
        constellation::tables,
        pulse::{self, Norm},
    };

    const SPS: usize = 8;

    fn rrc() -> Vec<f32> {
        pulse::root_raised_cosine(SPS as f64, 0.35, 8, Norm::Energy)
    }

    fn params(m: u32) -> LinearParams {
        LinearParams::new(tables::qam_square(m).unwrap(), rrc(), SPS).unwrap()
    }

    fn random_labels(n: usize, m: u32, seed: u64) -> Vec<u32> {
        let mut rng = Rng::new(seed);
        (0..n)
            .map(|_| (rng.next_u64() % u64::from(m)) as u32)
            .collect()
    }

    /// The accounting the whole harness rests on: unit-energy pulse times unit-mean-energy table
    /// means one unit of energy per symbol, hence per k bits.
    #[test]
    fn block_energy_is_one_per_symbol() {
        for m in [4u32, 16, 64] {
            let labels = random_labels(2048, m, 0xe9e);
            let wave = LinearMod::transmission(&params(m), &labels);
            let es = signal_energy(&wave) / labels.len() as f64;
            assert!((es - 1.0).abs() < 0.02, "M={m}: Es = {es}");
        }
    }

    /// Streaming must be a pure refactor of the one-shot call: any block split, same samples.
    #[test]
    fn any_block_split_gives_the_same_waveform() {
        let p = params(16);
        let labels = random_labels(300, 16, 0x5137);
        let whole = LinearMod::transmission(&p, &labels);
        let mut m = LinearMod::new(p);
        let mut split = Vec::new();
        for chunk in labels.chunks(37) {
            m.modulate(chunk, &mut split);
        }
        m.flush(&mut split);
        assert_eq!(split, whole);
    }

    /// The rotation is a schedule, not a state: symbol k's point is `exp(jkθ)` times the
    /// table's, read directly off the modulator.
    #[test]
    fn the_rotation_schedule_advances_one_step_per_symbol() {
        let p = params(4).with_rotation(tables::PI_4_ROTATION).unwrap();
        let m = LinearMod::new(p);
        let base = m.point(0, 0);
        for k in [1u64, 2, 7, 1_000_003] {
            let want = (k as f64 * std::f64::consts::FRAC_PI_4) % std::f64::consts::TAU;
            let got = f64::from((m.point(k, 0) / base).arg());
            let diff = (got - want + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
                - std::f64::consts::PI;
            assert!(diff.abs() < 1e-5, "k={k}: {got} vs {want}");
        }
    }

    /// The stagger is exactly half a symbol on Q and nothing on I — checked on a single symbol,
    /// where the two rails' pulse peaks must land `sps/2` samples apart.
    #[test]
    fn the_stagger_delays_the_quadrature_rail_by_half_a_symbol() {
        let p = params(4).with_offset(true).unwrap();
        // Label 0b11 is the (+, +) corner of Gray 4-QAM: both rails carry a positive pulse.
        let wave = LinearMod::transmission(&p, &[0b11]);
        let peak = |f: fn(&Complex<f32>) -> f32| {
            wave.iter()
                .enumerate()
                .max_by(|a, b| f(a.1).abs().total_cmp(&f(b.1).abs()))
                .map(|(i, _)| i)
                .unwrap()
        };
        assert_eq!(peak(|s| s.im) - peak(|s| s.re), SPS / 2);
        // Unstaggered, the two peaks coincide.
        let aligned = LinearMod::transmission(&params(4), &[0b11]);
        let peak_a = |f: fn(&Complex<f32>) -> f32| {
            aligned
                .iter()
                .enumerate()
                .max_by(|a, b| f(a.1).abs().total_cmp(&f(b.1).abs()))
                .map(|(i, _)| i)
                .unwrap()
        };
        assert_eq!(peak_a(|s| s.im), peak_a(|s| s.re));
    }

    /// Staggering costs no energy and keeps the trajectory away from the origin, which is its
    /// whole purpose. Stated as *how often* the envelope collapses rather than as a hard floor:
    /// with root-raised-cosine shaping the bound is statistical, not structural — but a QPSK
    /// diagonal transition passes through zero by construction, and a staggered one never turns
    /// more than 90° at a time, so the fraction of the trajectory spent near the origin differs
    /// by more than an order.
    #[test]
    fn the_stagger_keeps_the_trajectory_off_the_origin() {
        let labels = random_labels(512, 4, 0x0f5e7);
        let plain = LinearMod::transmission(&params(4), &labels);
        let staggered = LinearMod::transmission(&params(4).with_offset(true).unwrap(), &labels);
        let near_origin = |w: &[Complex<f32>], frac: f64| {
            // Skip a whole pulse span at each end, where the envelope is legitimately small.
            let inner = &w[8 * SPS..w.len() - 8 * SPS];
            let rms = (signal_energy(inner) / inner.len() as f64).sqrt();
            inner
                .iter()
                .filter(|s| f64::from(s.norm()) < frac * rms)
                .count() as f64
                / inner.len() as f64
        };
        // At a fifth of the RMS envelope: QPSK spends 2.2 % of its trajectory there, OQPSK
        // 0.05 %. At a tenth, QPSK still spends 0.7 % and OQPSK never arrives at all.
        let (a, b) = (near_origin(&plain, 0.2), near_origin(&staggered, 0.2));
        assert!(a > 0.02, "QPSK spends only {a} of its time near the origin");
        assert!(b * 20.0 < a, "OQPSK {b} vs QPSK {a}");
        assert!(near_origin(&plain, 0.1) > 0.005);
        assert_eq!(near_origin(&staggered, 0.1), 0.0);
        let (ea, eb) = (signal_energy(&plain), signal_energy(&staggered));
        assert!((ea / eb - 1.0).abs() < 0.02, "{ea} vs {eb}");
    }

    /// Labels are looked up, not indexed: the descent-labelled tables put label ℓ at a table
    /// position that is not ℓ, and a modulator that indexed would transmit the wrong point.
    #[test]
    fn labels_index_the_table_by_value() {
        let table = tables::qam_cross(32).unwrap();
        assert!(
            table
                .labels()
                .iter()
                .enumerate()
                .any(|(i, &l)| l != i as u32),
            "the test needs a table whose labels are not their own indices"
        );
        let p = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
        let m = LinearMod::new(p);
        for (i, &label) in table.labels().iter().enumerate() {
            assert_eq!(m.point(0, label), table.points()[i]);
        }
    }
}
