use std::{cell::RefCell, rc::Rc};

use num_complex::Complex;
use sdrmm_modem::{
    constellation::{
        Constellation,
        demap::{exact_llrs, noise_var_from_known},
    },
    soft::Llr,
};

#[derive(Debug, Default)]
pub struct GenieTap {
    tx_symbols: Vec<Complex<f32>>,
    clean_wave: Vec<Complex<f32>>,
}

impl GenieTap {
    #[must_use]
    pub fn shared() -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self::default()))
    }

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

    #[must_use]
    pub fn true_noise_var(&self, received: &[Complex<f32>]) -> f64 {
        noise_var_from_known(received, &self.clean_wave)
    }
}

pub fn genie_llrs(
    statistics: &[Complex<f32>],
    c: &Constellation,
    true_noise_var: f64,
    out: &mut Vec<Llr>,
) {
    let bits = c.bits_per_symbol();
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

    const INFO_BITS: usize = 1024;
    const FLUSH_BITS: usize = 4;

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

    #[derive(Clone, Copy)]
    enum LlrSource {
        Genie,
        MaxLog { noise_var_scale: f64 },
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
                        sdrmm_modem::constellation::demap::max_log_llrs(
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
                        *l = Llr(sdrmm_modem::soft::LLR_SATURATION.copysign(l.0));
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

    #[test]
    fn genie_separates_concept_failures_from_llr_quality() {
        let spec = ChannelSpec::default();
        let seed = 0x6e2e;
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

        assert!(real_gap < 0.3, "max-log vs genie gap {real_gap} dB");
        assert!(
            real_gap > -0.15,
            "demapper beats the genie by {real_gap} dB"
        );

        assert!(
            mis_gap > real_gap + 0.1,
            "10x mis-scale ({mis_gap} dB) not separated from calibrated ({real_gap} dB)"
        );
        assert!(
            (0.1..0.8).contains(&mis_gap),
            "10x mis-scale gap {mis_gap} dB outside the measured window"
        );

        assert!(
            clip_gap > mis_gap + 0.5,
            "hard-clip ({clip_gap} dB) not separated from 10x mis-scale ({mis_gap} dB)"
        );
        assert!(
            (1.8..3.5).contains(&clip_gap),
            "hard-clip gap {clip_gap} dB outside the measured window"
        );
    }

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
