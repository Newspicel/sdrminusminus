use num_complex::Complex;

/// Differentially detect a symbol stream in place-free form: output `k` is the normalised
/// product of input `k+1` with the conjugate of input `k`, so the result is one symbol shorter
/// than the input. The first input symbol is a phase reference and carries no data — which is
/// exactly the one symbol of overhead differential coding costs, and it is charged to the
/// entry's Eb accounting by its framing, not hidden here.
///
/// A zero reference symbol (an origin sample) yields a zero product rather than an infinity: no
/// vote, the same convention the carrier loop's detectors use.
pub fn differential_detect(symbols: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
    for pair in symbols.windows(2) {
        let (previous, current) = (pair[0], pair[1]);
        let magnitude = previous.norm();
        out.push(if magnitude > 0.0 {
            current * previous.conj() / magnitude
        } else {
            Complex::new(0.0, 0.0)
        });
    }
}

/// The streaming form: keeps the last symbol across calls, so a transmission may be detected in
/// blocks and any split gives the same products.
#[derive(Clone, Debug, Default)]
pub struct DifferentialDetector {
    previous: Option<Complex<f32>>,
}

impl DifferentialDetector {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Detect a block, appending one product per symbol *after the first ever seen*.
    pub fn process(&mut self, symbols: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        for &current in symbols {
            if let Some(previous) = self.previous {
                let magnitude = previous.norm();
                out.push(if magnitude > 0.0 {
                    current * previous.conj() / magnitude
                } else {
                    Complex::new(0.0, 0.0)
                });
            }
            self.previous = Some(current);
        }
    }

    pub fn reset(&mut self) {
        self.previous = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ber::rng::Rng,
        constellation::tables,
        symbolcode::{DifferentialSymbolDecoder, DifferentialSymbolEncoder},
    };

    /// The tier's defining property: differentially encoded indices survive an arbitrary,
    /// unknown carrier phase — the ambiguity a coherent loop cannot resolve by itself.
    #[test]
    fn an_unknown_carrier_phase_costs_nothing() {
        let table = tables::psk(4).unwrap();
        let mut rng = Rng::new(0xd9c);
        let data: Vec<u32> = (0..500).map(|_| (rng.next_u64() % 4) as u32).collect();
        let mut encoder = DifferentialSymbolEncoder::new(4);
        let sent: Vec<u32> = std::iter::once(0)
            .chain(data.iter().map(|&s| encoder.encode(s)))
            .collect();
        let point = |index: u32| {
            let i = table
                .labels()
                .iter()
                .position(|&l| l == tables::gray(index))
                .unwrap();
            table.points()[i]
        };
        for phase_turns in [0.0f64, 0.1, 0.25, 0.5, 0.77] {
            let theta = std::f64::consts::TAU * phase_turns;
            let rot = Complex::new(theta.cos() as f32, theta.sin() as f32);
            let wave: Vec<Complex<f32>> = sent.iter().map(|&s| point(s) * rot).collect();
            let mut products = Vec::new();
            differential_detect(&wave, &mut products);
            let steps: Vec<u32> = products
                .iter()
                .map(|&z| {
                    let label = table.hard_slice(z);
                    (0..4).find(|&i| tables::gray(i) == label).unwrap()
                })
                .collect();
            let mut decoder = DifferentialSymbolDecoder::new(4);
            // The detector's products *are* the differences, so the step sequence is the data
            // directly; the decoder is applied to the transmitted indices to prove the pairing.
            let via_decoder: Vec<u32> = decoder.decode_all(&sent)[1..].to_vec();
            assert_eq!(steps, data, "phase {phase_turns}: steps");
            assert_eq!(via_decoder, data, "phase {phase_turns}: decoder pairing");
        }
    }

    /// π/4-DQPSK's difference alphabet, checked by construction: the transmitted symbols walk an
    /// 8-PSK grid, and every product lands on the π/4-rotated QPSK table.
    #[test]
    fn pi4_dqpsk_differences_land_on_the_rotated_qpsk_table() {
        let differences = tables::psk_rotated(4, tables::PI_4_ROTATION).unwrap();
        let mut rng = Rng::new(0x914);
        let mut encoder = DifferentialSymbolEncoder::new(4);
        let mut phase = 0.0f64;
        let mut wave = vec![Complex::new(1.0f32, 0.0)];
        let mut steps = Vec::new();
        for _ in 0..200 {
            let data = (rng.next_u64() % 4) as u32;
            steps.push(data);
            let _ = encoder.encode(data);
            phase += std::f64::consts::FRAC_PI_2 * f64::from(data) + tables::PI_4_ROTATION;
            wave.push(Complex::new(phase.cos() as f32, phase.sin() as f32));
        }
        let mut products = Vec::new();
        differential_detect(&wave, &mut products);
        for z in &products {
            let l = differences.hard_slice(*z);
            let i = differences.labels().iter().position(|&x| x == l).unwrap();
            assert!(
                (z - differences.points()[i]).norm() < 1e-5,
                "product {z} is not on the difference table"
            );
        }
    }

    /// Amplitude is restored, not squared: a star-QAM difference must be read at the radius of
    /// the symbol itself, or its amplitude bits are demapped against the wrong ring.
    #[test]
    fn the_product_carries_the_current_symbols_amplitude() {
        let a = Complex::new(2.0f32, 0.0);
        let b = Complex::new(0.0f32, 3.0);
        let mut out = Vec::new();
        differential_detect(&[a, b], &mut out);
        assert!((out[0].norm() - 3.0).abs() < 1e-6, "{}", out[0]);
        assert!((out[0].arg() - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    }

    #[test]
    fn streaming_matches_the_block_form_and_an_origin_casts_no_vote() {
        let mut rng = Rng::new(0x5719);
        let wave: Vec<Complex<f32>> = (0..300)
            .map(|_| {
                let theta = std::f64::consts::TAU * (rng.next_u64() % 8) as f64 / 8.0;
                Complex::new(theta.cos() as f32, theta.sin() as f32)
            })
            .collect();
        let mut whole = Vec::new();
        differential_detect(&wave, &mut whole);
        let mut detector = DifferentialDetector::new();
        let mut split = Vec::new();
        for chunk in wave.chunks(29) {
            detector.process(chunk, &mut split);
        }
        assert_eq!(whole, split);

        let mut out = Vec::new();
        differential_detect(&[Complex::new(0.0, 0.0), Complex::new(1.0, 1.0)], &mut out);
        assert_eq!(out, [Complex::new(0.0, 0.0)]);
    }
}
