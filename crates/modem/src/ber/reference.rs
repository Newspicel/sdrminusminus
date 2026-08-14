//! The harness's own reference link — the calibration standard behind the trust chain in
//! [`ber`](super): an RRC-shaped BPSK modulator and a matched-filter receiver with *known*
//! timing, deliberately containing no receiver loss of its own. No timing recovery, no carrier
//! recovery, no level estimation: every one of those would add its own implementation loss,
//! and this link exists so that the measured curve differs from ½·erfc(√γ) only by what the
//! *harness* does — payload generation, energy accounting, noise calibration, error counting.
//! A gap from the closed form here is a harness bug by definition ( §4.1), and the
//! phase-0 acceptance gate pins that gap below 0.2 dB across 0–10 dB.
//!
//! The classic ways a harness fails this gate, so the next reader debugs in order: Eb accounted
//! per coded or per channel bit instead of per information bit (curve shifts by the rate
//! factor), noise sigma set per complex sample instead of per component (a 3 dB shift), and a
//! pulse whose energy is not unity (a shift of 10·log10 Σh²). The third is why the taps come
//! from [`pulse::root_raised_cosine`] at [`Norm::Energy`]: `design_rrc`'s native normalisation
//! is unit *DC gain* — right for a channel filter, wrong for a pulse.

use num_complex::Complex;

use super::sweep::Link;
use crate::pulse::{self, Norm};

/// Standard narrowband shaping: the roll-off half the catalog's protocols use, a span long
/// enough that truncation ISI sits ~40 dB under the symbol energy, and 8 samples/symbol so the
/// waveform is generously oversampled for any later impairment axis.
const ALPHA: f64 = 0.35;
const SPAN: usize = 8;
const SPS: usize = 8;

/// Bits per trial block: large enough to amortise per-block work across the 1e7-bit points of
/// the full gate, small enough that low-SNR points do not overshoot their 100 errors by much.
const BITS_PER_TRIAL: usize = 4096;

/// The RRC pulse at unit energy, `Σ h[n]² = 1` (crate-root convention): the matched-filter
/// output at the correct instant is then exactly the ±1 symbol, and a block's measured energy
/// is its bit count — so Eb/N0 set from measured energy is the textbook one.
fn unit_energy_rrc() -> Vec<f32> {
    // Bit-identical to normalising `design_rrc`'s taps by hand (§1 minimal duplication): the
    // wrap under `Norm::Energy` performs the same f64 operations in the same order, so the
    // committed BPSK calibration cannot move.
    pulse::root_raised_cosine(SPS as f64, ALPHA, SPAN, Norm::Energy)
}

/// Ideal coherent BPSK over RRC pulses: bit 1 → +1, bit 0 → −1 (crate-root sign convention),
/// full-length pulse superposition on transmit, matched RRC on receive, sampled at the known
/// cascade group delay of `taps−1` samples. The receiver's decision statistic is the real
/// rail after matched filtering — filtering commutes with `Re` for real taps, so filtering
/// only the real part is the same statistic at half the work.
#[must_use]
pub fn ideal_bpsk() -> Link {
    let tx_taps = unit_energy_rrc();
    let rx_taps = tx_taps.clone();
    Link {
        label: format!(
            "ideal BPSK, RRC α={ALPHA} span={SPAN} sps={SPS}, matched filter, known timing"
        ),
        bits_per_trial: BITS_PER_TRIAL,
        modulate: Box::new(move |bits| modulate(&tx_taps, bits)),
        demodulate: Box::new(move |wave| demodulate(&rx_taps, wave)),
    }
}

/// Full pulse superposition — the edge symbols keep their whole tails, so block energy is
/// `n_bits · Σh²` up to the Nyquist cross-terms and the Eb accounting has no edge deficit.
fn modulate(taps: &[f32], bits: &[bool]) -> Vec<Complex<f32>> {
    if bits.is_empty() {
        return Vec::new();
    }
    let mut out = vec![Complex::new(0.0f32, 0.0); (bits.len() - 1) * SPS + taps.len()];
    for (k, &bit) in bits.iter().enumerate() {
        let a: f32 = if bit { 1.0 } else { -1.0 };
        let base = k * SPS;
        for (m, &h) in taps.iter().enumerate() {
            out[base + m].re += a * h;
        }
    }
    out
}

/// The reference chain's shaping for arbitrary complex symbol streams: the same unit-energy
/// RRC taps, full-tail superposition and known-timing sampling as [`ideal_bpsk`], held as a
/// value so links that carry constellations — the genie-bound demonstration in
/// [`genie`](super::genie) runs coded 4-PAM this way — measure over the calibrated chain
/// instead of a second, unproven one. A ±1 real symbol stream reproduces the BPSK link's
/// waveform sample-for-sample and its decisions bit-for-bit (asserted below), which is what
/// lets measurements over this struct inherit the phase-0 erfc gate.
#[derive(Clone, Debug)]
pub struct IdealShaping {
    taps: Vec<f32>,
}

impl IdealShaping {
    #[must_use]
    pub fn new() -> Self {
        Self {
            taps: unit_energy_rrc(),
        }
    }

    /// Callers sizing symbol buffers against waveform lengths need the chain's own rate.
    #[must_use]
    pub fn samples_per_symbol(&self) -> usize {
        SPS
    }

    /// Full-tail superposition of `symbols` on the unit-energy pulse — [`modulate`] for a
    /// complex stream: block energy is `Σ|s|²` up to the Nyquist cross-terms, so a
    /// unit-mean-Es constellation keeps the crate's Eb accounting exact.
    #[must_use]
    pub fn modulate(&self, symbols: &[Complex<f32>]) -> Vec<Complex<f32>> {
        if symbols.is_empty() {
            return Vec::new();
        }
        let mut out = vec![Complex::new(0.0f32, 0.0); (symbols.len() - 1) * SPS + self.taps.len()];
        for (k, &s) in symbols.iter().enumerate() {
            let base = k * SPS;
            for (m, &h) in self.taps.iter().enumerate() {
                out[base + m] += s * h;
            }
        }
        out
    }

    /// Matched-filter statistics at the known symbol instants — [`demodulate`] before its
    /// slicer, kept complex so a demapper can turn them into LLRs. Two properties of this
    /// chain make the statistic exactly the demap model's `symbol + noise`: unit-energy taps
    /// pass white noise at its per-sample total variance, so the waveform's N0 *is* the
    /// statistic's, and the RRC⊗RRC cascade is Nyquist, so consecutive statistics' noise is
    /// uncorrelated. The genie bound in [`genie`](super::genie) rests on both.
    #[must_use]
    pub fn symbol_statistics(&self, wave: &[Complex<f32>]) -> Vec<Complex<f32>> {
        let nt = self.taps.len();
        if wave.len() < nt {
            return Vec::new();
        }
        let n = (wave.len() - nt) / SPS + 1;
        (0..n)
            .map(|k| {
                let base = k * SPS;
                let mut acc = Complex::new(0.0f32, 0.0);
                for (m, &h) in self.taps.iter().enumerate() {
                    acc += wave[base + nt - 1 - m] * h;
                }
                acc
            })
            .collect()
    }
}

impl Default for IdealShaping {
    fn default() -> Self {
        Self::new()
    }
}

/// Matched filter evaluated only at the symbol instants: symbol `k`'s statistic is the full
/// convolution's sample `k·SPS + taps−1` — pulse peak at `(taps−1)/2` plus the matched
/// filter's equal delay — which is the dot product of the taps with the window starting at
/// `k·SPS`. Computing just those dot products skips the 7/8 of the filter output the sampler
/// would discard.
fn demodulate(taps: &[f32], wave: &[Complex<f32>]) -> Vec<bool> {
    let nt = taps.len();
    if wave.len() < nt {
        return Vec::new();
    }
    let n_bits = (wave.len() - nt) / SPS + 1;
    (0..n_bits)
        .map(|k| {
            let base = k * SPS;
            let mut acc = 0.0f32;
            for (m, &h) in taps.iter().enumerate() {
                acc += h * wave[base + nt - 1 - m].re;
            }
            // Positive statistic decides logical 1 — the crate-root sign convention.
            acc > 0.0
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::ber::{
        impair::{Awgn, ChannelSpec, Impairment, signal_energy},
        rng::Rng,
        sweep::{load_json, save_json, sweep_ber, worst_penalty_db, worst_penalty_db_vs_curve},
        theory,
    };

    #[test]
    fn pulse_is_unit_energy() {
        let taps = unit_energy_rrc();
        let energy: f64 = taps.iter().map(|&h| f64::from(h) * f64::from(h)).sum();
        assert!((energy - 1.0).abs() < 1e-6, "Σh² = {energy}");
    }

    /// Noiseless loopback is exact — any hard-decision error with zero noise means the known
    /// delay or the sign convention is wrong, which would poison every measured curve.
    #[test]
    fn noiseless_round_trip_is_error_free() {
        let link = ideal_bpsk();
        let mut rng = Rng::new(0xb175);
        let bits: Vec<bool> = (0..2048).map(|_| rng.next_u64() & 1 == 1).collect();
        let wave = (link.modulate)(&bits);
        let decoded = (link.demodulate)(&wave);
        assert_eq!(decoded, bits);
    }

    /// The Eb accounting the whole gate rests on: with unit-energy pulses and ±1 symbols a
    /// block's energy is its bit count, up to the RRC cascade's Nyquist cross-terms.
    #[test]
    fn block_energy_is_one_per_bit() {
        let link = ideal_bpsk();
        let mut rng = Rng::new(0xeb);
        let bits: Vec<bool> = (0..4096).map(|_| rng.next_u64() & 1 == 1).collect();
        let eb = signal_energy(&(link.modulate)(&bits)) / bits.len() as f64;
        assert!((eb - 1.0).abs() < 0.01, "Eb = {eb}");
    }

    /// Phase-0 acceptance gate, smoke tier: the harness reads BPSK within 0.2 dB of ½erfc(√γ)
    /// on a fast subset. The full 0–10 dB tier is `bpsk_matches_erfc_full`.
    ///
    /// Error budget: [`MIN_ERRORS_PER_POINT`](crate::ber::MIN_ERRORS_PER_POINT) is a floor,
    /// not the gate's budget. Its ±20% vertical confidence is tight *divided by the curve's
    /// log-slope*, and at 0–2 dB that slope is a shallow ~0.15–0.21 decade/dB — 100 errors
    /// there is a ±0.3–0.4 dB horizontal interval that would trip a 0.2 dB gate on counting
    /// noise alone. 5000 errors puts every point's 95% interval under 0.09 dB, and low-SNR
    /// errors are nearly free (5000 at 0 dB is ~64k bits).
    #[test]
    fn bpsk_matches_erfc_smoke() {
        let link = ideal_bpsk();
        let points = [0.0, 2.0, 4.0, 6.0];
        let curve = sweep_ber(
            &link,
            &ChannelSpec::default(),
            &points,
            0x0b9,
            5000,
            4_000_000,
        );
        for p in &curve.points {
            assert!(p.errors >= 100, "point {p:?} under the error floor");
        }
        let worst = worst_penalty_db(&curve, theory::bpsk_ber, 0.0, 6.0);
        assert!(worst.abs() < 0.2, "worst penalty {worst} dB\n{curve:?}");
    }

    /// The claim that lets [`IdealShaping`] measurements inherit the erfc gate: on the same
    /// symbols it *is* the BPSK link — waveform sample-identical, and decisions bit-identical
    /// even under noise, because the real-rail arithmetic is the same operations in the same
    /// order.
    #[test]
    fn ideal_shaping_reproduces_the_bpsk_link_exactly() {
        let link = ideal_bpsk();
        let shaping = IdealShaping::new();
        let mut rng = Rng::new(0x5a9e);
        let bits: Vec<bool> = (0..1024).map(|_| rng.next_u64() & 1 == 1).collect();
        let symbols: Vec<Complex<f32>> = bits
            .iter()
            .map(|&b| Complex::new(if b { 1.0 } else { -1.0 }, 0.0))
            .collect();
        let wave = shaping.modulate(&symbols);
        assert_eq!(wave, (link.modulate)(&bits));

        let mut noisy = wave;
        Awgn::with_sigma(0.5).apply(&mut noisy, &mut rng);
        let statistic_decisions: Vec<bool> = shaping
            .symbol_statistics(&noisy)
            .iter()
            .map(|y| y.re > 0.0)
            .collect();
        assert_eq!(statistic_decisions, (link.demodulate)(&noisy));
    }

    /// Noiseless statistics must return the symbols themselves up to the chain's residual ISI
    /// — the −40 dB truncation tail plus the discrete RRC⊗RRC's Nyquist error — which bounds
    /// the statistic error near 1% of a unit symbol. A miss here would bias every LLR the
    /// genie demonstration computes.
    #[test]
    fn symbol_statistics_recover_a_noiseless_stream() {
        let shaping = IdealShaping::new();
        let mut rng = Rng::new(0x151);
        // ±1/±3-shaped complex points exercise both rails and unequal magnitudes.
        let symbols: Vec<Complex<f32>> = (0..512)
            .map(|_| {
                let re = [-3.0f32, -1.0, 1.0, 3.0][(rng.next_u64() & 3) as usize];
                let im = [-1.0f32, 1.0][(rng.next_u64() & 1) as usize];
                Complex::new(re * 0.3, im * 0.3)
            })
            .collect();
        let stats = shaping.symbol_statistics(&shaping.modulate(&symbols));
        assert_eq!(stats.len(), symbols.len());
        let worst = stats
            .iter()
            .zip(&symbols)
            .map(|(y, s)| (y - s).norm())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.02, "worst statistic error {worst}");
    }

    fn baseline_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("baselines/bpsk_ideal_awgn.json")
    }

    /// Phase-0 acceptance gate, full tier ( §7 phase 0): 0–10 dB in 1 dB steps,
    /// ≥100 errors per point, worst |penalty| vs the exact closed form under 0.2 dB — and the
    /// measurement guarded within 0.1 dB against the committed baseline curve, which this test
    /// creates on first run and thereafter treats as the regression reference. Run with
    /// `cargo test -p sdrmm-modem --release bpsk_matches_erfc_full -- --ignored --nocapture`.
    ///
    /// The 10 000-error target (floor still asserted at 100) exists for the reason documented
    /// on the smoke tier, scaled to a worst-of-eleven gate: at the shallow low-SNR slopes a
    /// 100-error point's horizontal confidence interval is wider than the gate itself. The
    /// 5e7-bit cap bounds the steep high-SNR points instead, where ~200 errors already read
    /// within ±0.07 dB.
    #[test]
    #[ignore = "full gate: ~2e8 trial bits, run in release (see doc comment)"]
    fn bpsk_matches_erfc_full() {
        let link = ideal_bpsk();
        let points: Vec<f64> = (0..=10).map(f64::from).collect();
        let curve = sweep_ber(
            &link,
            &ChannelSpec::default(),
            &points,
            0x5eed,
            10_000,
            50_000_000,
        );
        for p in &curve.points {
            assert!(p.errors >= 100, "point {p:?} under the error floor");
        }
        for p in &curve.points {
            println!(
                "{:>5.1} dB  {:>10} / {:<12} BER {:.3e}",
                p.ebn0_db,
                p.errors,
                p.trials,
                p.rate()
            );
        }
        let worst = worst_penalty_db(&curve, theory::bpsk_ber, 0.0, 10.0);
        println!("worst penalty vs erfc: {worst:+.4} dB");
        assert!(worst.abs() < 0.2, "worst penalty {worst} dB");

        let path = baseline_path();
        if path.exists() {
            let baseline = load_json(&path).unwrap();
            let drift = worst_penalty_db_vs_curve(&curve, &baseline, 0.0, 10.0);
            println!("worst drift vs committed baseline: {drift:+.4} dB");
            assert!(drift.abs() < 0.1, "drift vs baseline {drift} dB");
        } else {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            save_json(&curve, &path).unwrap();
            println!("baseline created at {}", path.display());
        }
    }
}
