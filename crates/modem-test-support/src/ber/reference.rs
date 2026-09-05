use num_complex::Complex;
use sdrmm_modem::pulse::{self, Norm};

use super::sweep::Link;

const ALPHA: f64 = 0.35;
const SPAN: usize = 8;
const SPS: usize = 8;

const BITS_PER_TRIAL: usize = 4096;

fn unit_energy_rrc() -> Vec<f32> {
    pulse::root_raised_cosine(SPS as f64, ALPHA, SPAN, Norm::Energy)
}

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

    #[must_use]
    pub fn samples_per_symbol(&self) -> usize {
        SPS
    }

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

    #[test]
    fn noiseless_round_trip_is_error_free() {
        let link = ideal_bpsk();
        let mut rng = Rng::new(0xb175);
        let bits: Vec<bool> = (0..2048).map(|_| rng.next_u64() & 1 == 1).collect();
        let wave = (link.modulate)(&bits);
        let decoded = (link.demodulate)(&wave);
        assert_eq!(decoded, bits);
    }

    #[test]
    fn block_energy_is_one_per_bit() {
        let link = ideal_bpsk();
        let mut rng = Rng::new(0xeb);
        let bits: Vec<bool> = (0..4096).map(|_| rng.next_u64() & 1 == 1).collect();
        let eb = signal_energy(&(link.modulate)(&bits)) / bits.len() as f64;
        assert!((eb - 1.0).abs() < 0.01, "Eb = {eb}");
    }

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

    #[test]
    fn symbol_statistics_recover_a_noiseless_stream() {
        let shaping = IdealShaping::new();
        let mut rng = Rng::new(0x151);
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
