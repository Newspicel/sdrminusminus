//! The genie-LLR bound ( §7 phase 1 acceptance): the harness instrument that tells
//! a *concept* failure from an *LLR-quality* failure. A coded link is run twice over the same
//! seeds — once with its real demapper, once with LLRs a genie computes from the true channel
//! state — and the post-FEC curves are compared with
//! [`penalty_db_vs_curve`](super::sweep::penalty_db_vs_curve). The reading is binary: a bad
//! genie curve means the concept is broken (code, mapping, metric wiring, Eb accounting) and
//! no demapper work can save it; a clean genie curve with the real curve trailing it means the
//! gap belongs to the LLR path (demapper tier, noise-variance calibration, sync losses) — and
//! the gap *is* that path's measured cost.
//!
//! What the genie knows, precisely: the transmitted symbols and the clean waveform they shaped
//! into ([`GenieTap`]), and through them the noise the channel actually applied
//! ([`GenieTap::true_noise_var`]). What it never sees is the answer key: [`genie_llrs`] is the
//! honest exact-tier posterior of each received statistic, computed with perfect channel
//! knowledge but zero knowledge of which symbol the statistic carries. A genie that peeked at
//! the transmitted bit would decode error-free at any Eb/N0 and separate nothing; this one is
//! bounded by the channel, which is the point. The genie also draws no randomness of its own —
//! runs stay reproducible from the sweep seed alone, and a genie run and a real-demapper run
//! at the same seed see bit-identical payloads, waveforms and noise, so their curve gap is a
//! paired comparison carrying almost none of the counting noise two independent sweeps would.
//!
//! The committed demonstration (`genie_separates_concept_failures_from_llr_quality` below,
//! rate-1/2 K=5 Viterbi over Gray 4-PAM on the reference chain) measures three LLR qualities
//! at the same seeds, plus the concept side. Measured gaps vs the genie curve at post-FEC
//! BER 6e-3, seed 0x6e2e:
//!
//! - **max-log at the true noise variance:** +0.03 dB — on the bound to within the paired
//!   comparison's resolution; the max-log approximation costs 4-PAM essentially nothing at
//!   these distances. Asserted < 0.3 dB, the task-level bound for a healthy LLR path.
//! - **max-log at 10× the true variance:** +0.23 dB — real but modest, and the *mechanism*
//!   matters: a uniform LLR scale error is invisible to the Viterbi's metric comparisons, so
//!   the 10× axis is benign right up to [`Llr::to_fec`], where the fixed 8-nat saturation
//!   turns the ÷10 shrink into ~1.25 nats per i16 step and the weak bits soft decoding lives
//!   on collapse toward erasure. The 0.23 dB is that quantisation loss, not variance
//!   sensitivity in the decoder — stated so, rather than crediting the Viterbi with a
//!   discipline it does not have.
//! - **sign-preserving hard-clip of genie LLRs to full confidence:** +2.67 dB — every bit
//!   voting ±CONFIDENT is hard-decision decoding. Kept in the demonstration as the quality
//!   defect that stays loud even on a decoder whose metric normalisation would make a
//!   uniform mis-scale fully benign. Larger than the textbook ~2 dB soft-vs-hard figure
//!   because that is the deep-waterfall asymptote; at 6e-3 the hard curve is still on its
//!   shoulder.
//! - **concept failure** (`a_broken_mapping_fails_even_with_genie_llrs`): a natural-binary
//!   demapper against the Gray mapper floors at BER 0.506 with genie LLRs, at an Eb/N0 where
//!   the sound concept posts ≲1e-5 — the genie refuses to absolve a broken concept.

use std::{cell::RefCell, rc::Rc};

use num_complex::Complex;

use crate::{
    constellation::{
        Constellation,
        demap::{exact_llrs, noise_var_from_known},
    },
    soft::Llr,
};

/// The transmit-side record a genie-paired [`Link`](super::sweep::Link) keeps per trial: the
/// symbols it sent and the clean waveform they became, written by the modulate half and read
/// by the demodulate half. Shared behind `Rc<RefCell<_>>` because a `Link` is two separately
/// boxed `Fn` closures; the runners call them strictly modulate → channel → demodulate within
/// one trial, so the writer's borrow and the readers' never overlap.
#[derive(Debug, Default)]
pub struct GenieTap {
    tx_symbols: Vec<Complex<f32>>,
    clean_wave: Vec<Complex<f32>>,
}

impl GenieTap {
    /// One handle cloned into both closures of a link.
    #[must_use]
    pub fn shared() -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self::default()))
    }

    /// The modulate half calls this before returning: the trial's symbols and shaped
    /// waveform, copied because the channel then mutates the returned waveform in place.
    pub fn record(&mut self, tx_symbols: &[Complex<f32>], clean_wave: &[Complex<f32>]) {
        self.tx_symbols.clear();
        self.tx_symbols.extend_from_slice(tx_symbols);
        self.clean_wave.clear();
        self.clean_wave.extend_from_slice(clean_wave);
    }

    #[must_use]
    pub fn tx_symbols(&self) -> &[Complex<f32>] {
        &self.tx_symbols
    }

    #[must_use]
    pub fn clean_wave(&self) -> &[Complex<f32>] {
        &self.clean_wave
    }

    /// The true channel state the demappers under test have to estimate: total complex noise
    /// variance N0 = mean |received − clean|², measured against the clean waveform the genie
    /// kept — sample-exact knowledge no receiver has. Read back rather than passed in because
    /// inside [`sweep_ber`](super::sweep::sweep_ber) the AWGN axis derives its sigma per point
    /// from the waveform's own measured energy, so no constructor value exists for a link to
    /// be told; reading the applied noise back is the impair module's own applied == measured
    /// doctrine, and its 1/√n wobble (~0.6% on a 4096-bit trial, ~0.026 dB) sits below
    /// anything a BER gate resolves. Everything the channel did besides noise — rotation,
    /// ISI — lands in the estimate too, understating LLRs rather than overstating them,
    /// exactly as [`noise_var_from_known`] documents.
    ///
    /// The waveform-level number is also the symbol-statistic-level number: unit-energy
    /// matched taps pass white noise at its per-sample total variance (see
    /// [`IdealShaping::symbol_statistics`](super::reference::IdealShaping::symbol_statistics)),
    /// so this value feeds [`genie_llrs`] and the real demappers unchanged.
    ///
    /// # Panics
    /// If `received`'s length differs from the recorded clean waveform's, or no trial has
    /// been recorded — per [`noise_var_from_known`].
    #[must_use]
    pub fn true_noise_var(&self, received: &[Complex<f32>]) -> f64 {
        noise_var_from_known(received, &self.clean_wave)
    }
}

/// The genie LLR stream — the best any demapper of this front end could do: the exact-tier
/// posterior of every symbol statistic at the true noise variance, appended to `out` label bit
/// 0 first (the [`demap`](crate::constellation::demap) convention). Nothing here is new
/// arithmetic — it is [`exact_llrs`] fed perfect channel state; what makes it a *bound* is who
/// supplies the inputs: statistics from a known-timing front end and N0 from
/// [`GenieTap::true_noise_var`], leaving no estimation error, no timing loss and no
/// approximation tier between the channel and the FEC.
///
/// # Panics
/// As [`exact_llrs`]: if `true_noise_var` is not a positive finite number.
pub fn genie_llrs(
    statistics: &[Complex<f32>],
    c: &Constellation,
    true_noise_var: f64,
    out: &mut Vec<Llr>,
) {
    let bits = c.bits_per_symbol();
    // Labels are u32 — the same scratch bound the demappers size against.
    let mut scratch = [Llr(0.0); 32];
    for &y in statistics {
        exact_llrs(y, c, true_noise_var, &mut scratch[..bits]);
        out.extend_from_slice(&scratch[..bits]);
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use num_complex::Complex;
    use sdrmm_dsp::fec::conv::{self, Viterbi5};

    use super::*;
    use crate::ber::{
        impair::{Awgn, ChannelSpec, Impairment},
        reference::IdealShaping,
        rng::Rng,
        sweep::{Link, penalty_db_vs_curve, sweep_ber},
    };

    /// Information bits per trial. The +4 flush bits and the edge pulse tails are charged to
    /// the link (Eb is per information bit), a ~0.02 dB overhead every run here shares.
    const INFO_BITS: usize = 1024;
    const FLUSH_BITS: usize = 4;

    /// Gray 4-PAM handed in as the ±1/±3 grid, normalised by construction to mean Es = 1 —
    /// the same table the demap hand-computation tests pin.
    fn gray_4pam() -> Constellation {
        Constellation::from_points(
            vec![
                Complex::new(-3.0, 0.0),
                Complex::new(-1.0, 0.0),
                Complex::new(1.0, 0.0),
                Complex::new(3.0, 0.0),
            ],
            vec![0b00, 0b01, 0b11, 0b10],
        )
        .unwrap()
    }

    /// The one stage the genie swap replaces: how symbol statistics and the true N0 become
    /// the FEC's LLRs.
    #[derive(Clone, Copy)]
    enum LlrSource {
        /// [`genie_llrs`] — the bound.
        Genie,
        /// The real demapper at `noise_var_scale` × the true variance: 1.0 is a correctly
        /// calibrated max-log tier, 10.0 the deliberate mis-calibration.
        MaxLog { noise_var_scale: f64 },
        /// Genie LLRs with every magnitude forced to full confidence — hard-decision
        /// decoding wearing the soft interface, the harshest sign-preserving quality defect.
        HardClip,
    }

    impl LlrSource {
        fn label(self) -> String {
            match self {
                Self::Genie => "genie LLRs".to_string(),
                Self::MaxLog { noise_var_scale } => {
                    format!("max-log LLRs at {noise_var_scale}x true noise var")
                }
                Self::HardClip => "hard-clipped genie LLRs".to_string(),
            }
        }
    }

    /// The demonstration link: payload + flush → rate-1/2 K=5 convolutional code → coded bit
    /// pairs onto Gray 4-PAM (first coded bit = label bit 0) → the reference chain's shaping
    /// → matched-filter statistics at known timing → `source` LLRs → [`Llr::to_fec`] →
    /// Viterbi. `rx_table` is the demapper's constellation — the sound concept hands the
    /// mapper's own table, the broken-concept test hands a different labelling.
    fn coded_pam4_with(source: LlrSource, rx_table: Constellation) -> Link {
        let shaping = Rc::new(IdealShaping::new());
        let tx_table = gray_4pam();
        let mut point_of_label = [Complex::new(0.0f32, 0.0); 4];
        for (p, &l) in tx_table.points().iter().zip(tx_table.labels()) {
            point_of_label[l as usize] = *p;
        }
        let tap = GenieTap::shared();
        let label = format!("coded Gray 4-PAM over ideal chain, {}", source.label());

        let mod_shaping = Rc::clone(&shaping);
        let mod_tap = Rc::clone(&tap);
        let modulate = move |bits: &[bool]| {
            let mut with_flush = bits.to_vec();
            with_flush.resize(bits.len() + FLUSH_BITS, false);
            let mut coded = Vec::new();
            conv::encode(&with_flush, &mut coded);
            let (pairs, _) = coded.as_chunks::<2>();
            let symbols: Vec<Complex<f32>> = pairs
                .iter()
                .map(|&[first, second]| {
                    point_of_label[usize::from(first) | usize::from(second) << 1]
                })
                .collect();
            let wave = mod_shaping.modulate(&symbols);
            mod_tap.borrow_mut().record(&symbols, &wave);
            wave
        };

        let viterbi = RefCell::new(Viterbi5::new());
        let demodulate = move |wave: &[Complex<f32>]| {
            let statistics = shaping.symbol_statistics(wave);
            let n0 = tap.borrow().true_noise_var(wave);
            let mut llrs = Vec::with_capacity(statistics.len() * 2);
            match source {
                LlrSource::Genie => genie_llrs(&statistics, &rx_table, n0, &mut llrs),
                LlrSource::MaxLog { noise_var_scale } => {
                    let mut out = [Llr(0.0); 2];
                    for &y in &statistics {
                        crate::constellation::demap::max_log_llrs(
                            y,
                            &rx_table,
                            n0 * noise_var_scale,
                            &mut out,
                        );
                        llrs.extend_from_slice(&out);
                    }
                }
                LlrSource::HardClip => {
                    genie_llrs(&statistics, &rx_table, n0, &mut llrs);
                    for l in &mut llrs {
                        *l = Llr(crate::soft::LLR_SATURATION.copysign(l.0));
                    }
                }
            }
            let soft: Vec<conv::Soft> = llrs.iter().map(|l| l.to_fec()).collect();
            let mut decoded = Vec::new();
            viterbi.borrow_mut().decode(&soft, &mut decoded);
            decoded.truncate(decoded.len().saturating_sub(FLUSH_BITS));
            decoded
        };

        Link {
            label,
            bits_per_trial: INFO_BITS,
            modulate: Box::new(modulate),
            demodulate: Box::new(demodulate),
        }
    }

    fn coded_pam4(source: LlrSource) -> Link {
        coded_pam4_with(source, gray_4pam())
    }

    /// The stream wrapper adds no arithmetic of its own: first statistic against the demap
    /// module's hand-computed exact-tier constants, second against [`exact_llrs`] called
    /// directly, order label-bit-0-first.
    #[test]
    fn genie_llrs_stream_the_exact_tier_per_symbol() {
        let c = gray_4pam();
        let statistics = [Complex::new(0.6f32, 0.0), Complex::new(-0.9, 0.3)];
        let mut out = Vec::new();
        genie_llrs(&statistics, &c, 0.5, &mut out);
        assert_eq!(out.len(), 4);
        assert!(
            (f64::from(out[0].0) - 1.162_316_7).abs() < 1e-5,
            "bit 0: {}",
            out[0].0
        );
        assert!(
            (f64::from(out[1].0) - 2.441_057_2).abs() < 1e-5,
            "bit 1: {}",
            out[1].0
        );
        let mut direct = [Llr(0.0); 2];
        exact_llrs(statistics[1], &c, 0.5, &mut direct);
        assert_eq!([out[2], out[3]], direct);
    }

    /// The genie's channel-state read against impair-injected AWGN of known sigma, at both
    /// levels the module doc claims are the same number: per-component σ = 0.25 means total
    /// N0 = 0.125 at the waveform (≈65k samples, estimator SE ~0.55%, so the 2% gate reads
    /// correctness) and the *same* N0 at the matched-filter statistics (8192 of them, SE
    /// ~1.6%, 5% gate) — the unit-energy-taps identity that lets one measurement feed the
    /// demappers directly.
    #[test]
    fn true_noise_var_reads_the_applied_awgn() {
        let shaping = IdealShaping::new();
        let c = gray_4pam();
        let mut rng = Rng::new(0x6e01);
        let symbols: Vec<Complex<f32>> = (0..8192)
            .map(|_| c.points()[(rng.next_u64() & 3) as usize])
            .collect();
        let clean = shaping.modulate(&symbols);
        let tap = GenieTap::shared();
        tap.borrow_mut().record(&symbols, &clean);

        let sigma = 0.25;
        let mut received = clean;
        Awgn::with_sigma(sigma).apply(&mut received, &mut rng);
        let truth = 2.0 * sigma * sigma;

        let n0 = tap.borrow().true_noise_var(&received);
        assert!(
            (n0 / truth - 1.0).abs() < 0.02,
            "waveform-level {n0}, injected {truth}"
        );

        let statistics = shaping.symbol_statistics(&received);
        let stat_var = noise_var_from_known(&statistics, &symbols);
        assert!(
            (stat_var / truth - 1.0).abs() < 0.05,
            "statistic-level {stat_var}, injected {truth}"
        );
    }

    /// The committed proof behind the module docs — see there for the reading of each gap.
    /// Setup: four LLR qualities on the identical link, identical seed, so every trial's
    /// payload, waveform and noise realisation are bit-identical across the runs sharing a
    /// point grid and the curve gaps are paired comparisons.
    ///
    /// Error budget (the `MIN_ERRORS_PER_POINT` doc note): 200 errors per point is a ±14%
    /// two-sided 95% vertical interval, ~0.06 decades; over the waterfall's measured
    /// ~0.7–1.3 decade/dB local slope that is at most ~0.09 dB horizontal per curve, before
    /// the pairing cancels the shared noise — an order under the 0.3 dB gate and well under
    /// the asserted orderings. The floor of 100 is asserted on every point. The hard-clip
    /// curve is right-shifted past the shared grid, so it gets its own points bracketing the
    /// comparison BER.
    ///
    /// Measured at seed 0x6e2e (deterministic; the asserted windows leave room only for
    /// cross-platform libm wobble, which moves single error counts): gaps vs genie at BER
    /// 6e-3 — max-log(true N0) +0.025 dB, max-log(10× N0) +0.231 dB, hard-clip +2.670 dB.
    #[test]
    fn genie_separates_concept_failures_from_llr_quality() {
        let spec = ChannelSpec::default();
        let seed = 0x6e2e;
        // The waterfall region (BER ~9e-3 → ~3e-3 for the bound); the hard-clip curve is
        // right-shifted far enough that it needs its own points around the comparison BER.
        let points = [4.0, 4.5, 5.0];
        let points_clip = [6.5, 7.0, 7.5];
        let sweep = |source: LlrSource, points: &[f64]| {
            let curve = sweep_ber(&coded_pam4(source), &spec, points, seed, 200, 600_000);
            for p in &curve.points {
                assert!(p.errors >= 100, "point {p:?} under the error floor");
            }
            curve
        };

        let genie = sweep(LlrSource::Genie, &points);
        let real = sweep(
            LlrSource::MaxLog {
                noise_var_scale: 1.0,
            },
            &points,
        );
        let mis = sweep(
            LlrSource::MaxLog {
                noise_var_scale: 10.0,
            },
            &points,
        );
        let clip = sweep(LlrSource::HardClip, &points_clip);

        let at_ber = 6e-3;
        let real_gap = penalty_db_vs_curve(&real, &genie, at_ber);
        let mis_gap = penalty_db_vs_curve(&mis, &genie, at_ber);
        let clip_gap = penalty_db_vs_curve(&clip, &genie, at_ber);
        println!(
            "gaps vs genie at BER {at_ber}: max-log {real_gap:+.3} dB, \
             10x mis-scaled {mis_gap:+.3} dB, hard-clip {clip_gap:+.3} dB"
        );

        // Quality intact: the correctly fed max-log tier sits on the bound. The lower bound
        // is the bound property itself — a demapper visibly *beating* the genie would mean
        // the genie (or the pairing) is broken.
        assert!(real_gap < 0.3, "max-log vs genie gap {real_gap} dB");
        assert!(
            real_gap > -0.15,
            "demapper beats the genie by {real_gap} dB"
        );

        // Quality broken, mildly: 10x mis-scale costs through Llr::to_fec quantisation only
        // (a float metric would shrug a uniform scale off) — asserted as ordering plus a
        // window around the measured value, not a knife-edge.
        assert!(
            mis_gap > real_gap + 0.1,
            "10x mis-scale ({mis_gap} dB) not separated from calibrated ({real_gap} dB)"
        );
        assert!(
            (0.1..0.8).contains(&mis_gap),
            "10x mis-scale gap {mis_gap} dB outside the measured window"
        );

        // Quality broken, harshly: full-confidence clipping is hard-decision decoding — the
        // defect that stays visible even on a decoder whose metric makes uniform scaling
        // fully benign. Larger than the textbook ~2 dB soft-vs-hard figure because that is
        // the deep-waterfall asymptote and at 6e-3 the hard curve is still on its shoulder.
        assert!(
            clip_gap > mis_gap + 0.5,
            "hard-clip ({clip_gap} dB) not separated from 10x mis-scale ({mis_gap} dB)"
        );
        assert!(
            (1.8..3.5).contains(&clip_gap),
            "hard-clip gap {clip_gap} dB outside the measured window"
        );
    }

    /// The concept side of the separation: a natural-binary demapper against the Gray mapper
    /// swaps the labels of the two positive-rail points, so a quarter of all coded bits
    /// arrive inverted at full genie confidence and the Viterbi floors — measured BER 0.506
    /// at 7.5 dB, where the sound concept posts ≲1e-5. Genie LLRs did not absolve it: the
    /// failure is the concept's, which is exactly the verdict the bound exists to deliver.
    #[test]
    fn a_broken_mapping_fails_even_with_genie_llrs() {
        let natural = Constellation::from_points(
            vec![
                Complex::new(-3.0, 0.0),
                Complex::new(-1.0, 0.0),
                Complex::new(1.0, 0.0),
                Complex::new(3.0, 0.0),
            ],
            vec![0b00, 0b01, 0b10, 0b11],
        )
        .unwrap();
        let link = coded_pam4_with(LlrSource::Genie, natural);
        let curve = sweep_ber(&link, &ChannelSpec::default(), &[7.5], 0x6e2e, 100, 20_000);
        let ber = curve.points[0].rate();
        println!("broken-mapping post-FEC BER with genie LLRs: {ber:.3}");
        assert!(
            ber > 0.1,
            "genie must not absolve a broken concept: BER {ber}"
        );
    }

    /// Determinism (harness doctrine): the genie adds no randomness, so a genie-paired coded
    /// run is byte-identical from its seed — across fresh link constructions, whose tap and
    /// Viterbi state are per-link — and a different seed is a different realisation.
    #[test]
    fn same_seed_reproduces_the_identical_genie_curve() {
        let run = |seed: u64| {
            sweep_ber(
                &coded_pam4(LlrSource::Genie),
                &ChannelSpec::default(),
                &[4.0],
                seed,
                100,
                60_000,
            )
        };
        let a = run(0xd6e);
        assert_eq!(a, run(0xd6e));
        assert_ne!(a, run(0xd6f), "a different seed must differ");
    }
}
