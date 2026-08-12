//! The reference CPM transmitter: a symbol-impulse train through the frequency pulse, a phase
//! integrator, complex baseband out. This is *the* modulator behind every CPM catalog entry —
//! `testgen`'s per-mode C4FM/GMSK recipes migrate onto it (MODEM-PLAN §1.2: testgen builds
//! demodulator test signals from the library's own modulators, so the two can never drift
//! apart), and `tx.rs` drives it as a signal generator.
//!
//! Phase arithmetic: with the unit-area frequency pulse `g` (asserted by [`CpmParams`]) and a
//! symbol at level L, the per-sample phase step is `π·h·(impulses ⊛ g)[n]`, so one symbol
//! advances the carrier by exactly `π·h·L` — the Aulin/Sundberg q(∞) = ½ convention
//! (see `pulse::phase_pulse`). The accumulator is f64 and wrapped each sample, so a
//! transmission of any length casts to f32 without the phase's magnitude eating its precision.
//!
//! Design is cold path: constructors and the keyed builder allocate freely. The §4.2
//! zero-allocation gate binds the demodulator's `process()`, not a test-signal generator.

use std::f64::consts::{PI, TAU};

use num_complex::Complex;
use sdrmm_dsp::RealDecimator;

use super::params::CpmParams;

/// Phase-continuous CPM modulator. Streaming: [`modulate`](Self::modulate) carries filter and
/// phase state across calls, so a transmission may be produced in blocks;
/// [`flush`](Self::flush) drains the last symbols' pulse tail. [`keyed`](Self::keyed) is the
/// one-shot TDMA builder.
pub struct CpmMod {
    params: CpmParams,
    shaper: RealDecimator,
    /// π·h — the per-sample step is this times the shaped impulse train.
    step_scale: f64,
    phase: f64,
    /// Symbols accepted so far; symbol k's impulse lands at sample `round(k·sps)`, which is
    /// exact for integer sps and within half a sample for fractional (0.5 % of a symbol at
    /// POCSAG's 93.75 — far under any receiver's timing tolerance).
    symbols_in: u64,
    /// Samples emitted so far by `modulate` (the shaper's delay means sample n of the stream
    /// is not symbol n's peak, but the count still names impulse positions exactly).
    samples_out: u64,
    impulses: Vec<f32>,
    shaped: Vec<f32>,
}

impl CpmMod {
    #[must_use]
    pub fn new(params: CpmParams) -> Self {
        let shaper = RealDecimator::new(params.freq_pulse(), 1);
        let step_scale = PI * params.h();
        Self {
            params,
            shaper,
            step_scale,
            phase: 0.0,
            symbols_in: 0,
            samples_out: 0,
            impulses: Vec::new(),
            shaped: Vec::new(),
        }
    }

    #[must_use]
    pub fn params(&self) -> &CpmParams {
        &self.params
    }

    /// Modulate a block of symbol indices, appending complex baseband to `out`. State carries
    /// across calls: the same symbols in any block split produce the same samples. The last
    /// pulse span stays in the shaper until more symbols — or [`flush`](Self::flush) — push it
    /// through.
    pub fn modulate(&mut self, symbols: &[u8], out: &mut Vec<Complex<f32>>) {
        let sps = self.params.sps();
        let first = self.samples_out;
        let target = ((self.symbols_in + symbols.len() as u64) as f64 * sps).round() as u64;
        self.impulses.clear();
        self.impulses.resize((target - first) as usize, 0.0);
        for (j, &sym) in symbols.iter().enumerate() {
            let at = ((self.symbols_in + j as u64) as f64 * sps).round() as u64 - first;
            self.impulses[at as usize] = self.params.mapping().level(sym);
        }
        self.symbols_in += symbols.len() as u64;
        self.samples_out = target;
        self.shaper.process(&self.impulses, &mut self.shaped);
        self.integrate(out);
    }

    /// Push one pulse length of silence through the shaper so the final symbols' tail reaches
    /// the output — a transmission that ends mid-tail hands the receiver's matched filter a
    /// truncated pulse it was not built for.
    pub fn flush(&mut self, out: &mut Vec<Complex<f32>>) {
        self.impulses.clear();
        self.impulses.resize(self.params.freq_pulse().len(), 0.0);
        self.samples_out += self.impulses.len() as u64;
        self.shaper.process(&self.impulses, &mut self.shaped);
        self.integrate(out);
    }

    /// One complete keyed transmission: `None` is a symbol period the transmitter neither
    /// modulates nor radiates, as a TDMA radio spends the other timeslot. The exciter keeps
    /// shaping through the gaps — a burst decays into silence with the pulse tails a matched
    /// filter expects — and the amplifier ramps over one symbol rather than stepping, because
    /// an envelope discontinuity is not something any radio puts on the air. Phase starts at
    /// zero and runs continuously through the gaps.
    ///
    /// Compatible with how `testgen` carves TDMA bursts today (`c4fm_keyed`): the returned
    /// waveform is `round(symbols·sps) + pulse_len` samples, tail included. One call is one
    /// transmission; the streaming state of [`modulate`](Self::modulate) is not touched.
    #[must_use]
    pub fn keyed(&self, symbols: &[Option<u8>]) -> Vec<Complex<f32>> {
        let sps = self.params.sps();
        let pulse_len = self.params.freq_pulse().len();
        let len = (symbols.len() as f64 * sps).round() as usize + pulse_len;
        let mut impulses = vec![0.0f32; len];
        for (k, &sym) in symbols.iter().enumerate() {
            if let Some(sym) = sym {
                impulses[(k as f64 * sps).round() as usize] = self.params.mapping().level(sym);
            }
        }
        let mut shaped = Vec::with_capacity(len);
        RealDecimator::new(self.params.freq_pulse(), 1).process(&impulses, &mut shaped);

        // The amplifier is on wherever a radiated symbol's pulse has support: symbol k's
        // energy spans samples [round(k·sps), round(k·sps) + pulse_len).
        let mut on = vec![false; shaped.len()];
        for (k, &sym) in symbols.iter().enumerate() {
            if sym.is_some() {
                let at = (k as f64 * sps).round() as usize;
                for slot in on.iter_mut().skip(at).take(pulse_len) {
                    *slot = true;
                }
            }
        }

        let ramp = sps.round() as usize;
        let mut phase = 0.0f64;
        shaped
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                phase = wrap(phase + self.step_scale * f64::from(s));
                let edge = (1..=ramp)
                    .find(|&d| !on[i.saturating_sub(d)] || !on[(i + d).min(on.len() - 1)])
                    .map_or(ramp, |d| d - 1);
                let gain = if on[i] {
                    0.5 * (1.0 - (PI * edge as f64 / ramp as f64).cos()) as f32
                } else {
                    0.0
                };
                Complex::from_polar(gain, phase as f32)
            })
            .collect()
    }

    /// Forget the stream: phase, shaper history and sample accounting. The next `modulate`
    /// call starts a fresh transmission.
    pub fn reset(&mut self) {
        self.shaper = RealDecimator::new(self.params.freq_pulse(), 1);
        self.phase = 0.0;
        self.symbols_in = 0;
        self.samples_out = 0;
    }

    fn integrate(&mut self, out: &mut Vec<Complex<f32>>) {
        for &s in &self.shaped {
            self.phase = wrap(self.phase + self.step_scale * f64::from(s));
            out.push(Complex::from_polar(1.0, self.phase as f32));
        }
    }
}

/// Keeps the accumulator in (−π, π]: the f64→f32 cast at every sample then never spends
/// mantissa bits on whole turns a minutes-long transmission accumulated.
fn wrap(phase: f64) -> f64 {
    if phase > PI {
        phase - TAU
    } else if phase <= -PI {
        phase + TAU
    } else {
        phase
    }
}

#[cfg(test)]
mod tests {
    use super::{super::params::Mapping, *};
    use crate::pulse::{self, Norm};

    fn dmr_params() -> CpmParams {
        CpmParams::from_deviation(
            Mapping::new(vec![1.0, 3.0, -1.0, -3.0]),
            1_944.0,
            4_800.0,
            pulse::root_raised_cosine(10.0, 0.2, 8, Norm::Area),
            10.0,
        )
    }

    fn symbols(len: usize, seed: u32) -> Vec<u8> {
        let mut state = seed | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state & 3) as u8
            })
            .collect()
    }

    #[test]
    fn every_sample_is_unit_magnitude_and_phase_continuous() {
        let mut m = CpmMod::new(dmr_params());
        let mut out = Vec::new();
        m.modulate(&symbols(200, 7), &mut out);
        m.flush(&mut out);
        assert!(!out.is_empty());
        for (n, s) in out.iter().enumerate() {
            assert!((s.norm() - 1.0).abs() < 1e-6, "sample {n}: |{s}|");
        }
        // Phase continuity: no sample-to-sample jump exceeds the worst instantaneous
        // frequency — all-outer same-sign symbols, whose pulses overlap as a stride-sps comb
        // over the taps, so the exact bound is the largest comb sum.
        let params = dmr_params();
        let sps = params.sps() as usize;
        let comb_max = (0..sps)
            .map(|r| {
                params.freq_pulse()[r..]
                    .iter()
                    .step_by(sps)
                    .map(|&t| f64::from(t.abs()))
                    .sum::<f64>()
            })
            .fold(0.0f64, f64::max);
        let bound = (PI * params.h() * 3.0 * comb_max) as f32 + 1e-3;
        for w in out.windows(2) {
            let jump = (w[1] * w[0].conj()).arg().abs();
            assert!(jump <= bound, "phase jump {jump} > {bound}");
        }
    }

    /// The full-response phase-step contract: one rect-pulse symbol at level L advances the
    /// carrier by exactly π·h·L (q(∞) = ½). h = 0.4 keeps every checked cumulative phase
    /// clear of the ±π wrap, where arg()'s sign is a coin toss.
    #[test]
    fn a_full_response_symbol_advances_phase_by_pi_h_level() {
        let h = 0.4;
        let sps = 8.0;
        let params = CpmParams::from_h(Mapping::natural(2), h, pulse::rect(sps, Norm::Area), sps);
        let mut m = CpmMod::new(params);
        let mut out = Vec::new();
        // Rect is one symbol long, so a symbol's step is complete at its last sample —
        // sample k·sps − 1, since each output sample carries its own increment.
        m.modulate(&[1, 1, 0, 1], &mut out);
        m.flush(&mut out);
        let phase_after = |k: usize| f64::from(out[k * sps as usize - 1].arg());
        for (k, want) in [(1, h), (2, 2.0 * h), (3, h)] {
            let got = phase_after(k) / PI;
            assert!(
                (got - want).abs() < 1e-4,
                "after {k} symbols: phase {got}π, want {want}π"
            );
        }
    }

    #[test]
    fn block_splits_do_not_change_the_waveform() {
        let syms = symbols(300, 41);
        let mut whole = CpmMod::new(dmr_params());
        let mut expected = Vec::new();
        whole.modulate(&syms, &mut expected);
        whole.flush(&mut expected);

        let mut split = CpmMod::new(dmr_params());
        let mut got = Vec::new();
        let mut pos = 0;
        for len in [7usize, 1, 64, 13, 111].iter().cycle() {
            if pos >= syms.len() {
                break;
            }
            let end = (pos + len).min(syms.len());
            split.modulate(&syms[pos..end], &mut got);
            pos = end;
        }
        split.flush(&mut got);
        assert_eq!(expected, got);
    }

    #[test]
    fn keyed_gaps_are_silent_and_edges_ramp() {
        let m = CpmMod::new(dmr_params());
        let mut syms: Vec<Option<u8>> = symbols(60, 9).into_iter().map(Some).collect();
        syms.extend(std::iter::repeat_n(None, 60));
        syms.extend(symbols(60, 10).into_iter().map(Some));
        let out = m.keyed(&syms);
        assert_eq!(out.len(), 180 * 10 + dmr_params().freq_pulse().len());

        // The gap: from a pulse length past the last on-symbol to the next burst's start.
        let pulse_len = dmr_params().freq_pulse().len();
        let gap = &out[60 * 10 + pulse_len..120 * 10];
        assert!(
            gap.iter().all(|s| s.norm() == 0.0),
            "the amplifier radiated into the dead time"
        );
        // Every sample bounded by the unit envelope; edges are not steps.
        assert!(out.iter().all(|s| s.norm() <= 1.0 + 1e-6));
        for w in out.windows(2) {
            assert!(
                (w[1].norm() - w[0].norm()).abs() < 0.5,
                "envelope step {} -> {}",
                w[0].norm(),
                w[1].norm()
            );
        }
    }

    #[test]
    fn reset_restarts_the_stream_exactly() {
        let syms = symbols(80, 3);
        let mut m = CpmMod::new(dmr_params());
        let mut first = Vec::new();
        m.modulate(&syms, &mut first);
        m.flush(&mut first);
        m.reset();
        let mut second = Vec::new();
        m.modulate(&syms, &mut second);
        m.flush(&mut second);
        assert_eq!(first, second);
    }
}
