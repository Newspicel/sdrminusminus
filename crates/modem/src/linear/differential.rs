use num_complex::Complex;

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

#[derive(Clone, Debug, Default)]
pub struct DifferentialDetector {
    previous: Option<Complex<f32>>,
}

impl DifferentialDetector {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

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
            let via_decoder: Vec<u32> = decoder.decode_all(&sent)[1..].to_vec();
            assert_eq!(steps, data, "phase {phase_turns}: steps");
            assert_eq!(via_decoder, data, "phase {phase_turns}: decoder pairing");
        }
    }

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
