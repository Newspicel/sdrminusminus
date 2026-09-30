use std::f32::consts::FRAC_1_SQRT_2;

use super::{BITS, CARRIERS, DATA_CARRIERS, PILOT, Sample, magnitude};

const EIGHTH_TURN: Sample = Sample::new(FRAC_1_SQRT_2, FRAC_1_SQRT_2);
const NORM_FLOOR: f64 = 1e-6;

pub(super) struct Decision {
    pub(super) bits: [bool; BITS],
    pub(super) sync_bit: bool,
    pub(super) frequency_error: f32,
}

pub(super) fn decide(symbols: &[Sample; CARRIERS], previous: &[Sample; CARRIERS]) -> Decision {
    let mut bits = [false; BITS];
    for ((pair, symbol), previous) in bits
        .as_chunks_mut::<2>()
        .0
        .iter_mut()
        .zip(&symbols[..DATA_CARRIERS])
        .zip(&previous[..DATA_CARRIERS])
    {
        let difference = difference(*symbol, *previous) * EIGHTH_TURN;
        *pair = [difference.im < 0.0, difference.re < 0.0];
    }
    let norm = inverse_magnitude(previous[PILOT]);
    let pilot = difference(symbols[PILOT], previous[PILOT]);
    let sync_bit = pilot.re < 0.0;
    let frequency_error = if sync_bit { pilot.im } else { -pilot.im } * norm;
    Decision {
        bits,
        sync_bit,
        frequency_error,
    }
}

fn difference(symbol: Sample, previous: Sample) -> Sample {
    symbol * previous.conj().scale(inverse_magnitude(previous))
}

fn inverse_magnitude(value: Sample) -> f32 {
    (1.0 / (f64::from(magnitude(value)) + NORM_FLOOR)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbols_with(data: Sample, pilot: Sample) -> [Sample; CARRIERS] {
        let mut symbols = [data; CARRIERS];
        symbols[PILOT] = pilot;
        symbols
    }

    #[test]
    fn each_quadrant_maps_to_its_dibit() {
        let previous = [Sample::ONE; CARRIERS];
        for (rotation, dibit) in [
            (Sample::ONE, [false, false]),
            (Sample::I, [false, true]),
            (-Sample::I, [true, false]),
            (-Sample::ONE, [true, true]),
        ] {
            let decision = decide(&symbols_with(rotation, Sample::ONE), &previous);
            assert!(
                decision
                    .bits
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .all(|pair| *pair == dibit),
                "{rotation}"
            );
        }
    }

    #[test]
    fn decisions_ignore_amplitude() {
        let previous = [Sample::new(0.01, 0.0); CARRIERS];
        let decision = decide(
            &symbols_with(Sample::new(0.0, 50.0), Sample::ONE),
            &previous,
        );
        assert!(
            decision
                .bits
                .as_chunks::<2>()
                .0
                .iter()
                .all(|pair| *pair == [false, true])
        );
    }

    #[test]
    fn a_flipped_pilot_is_a_sync_bit_and_its_rotation_a_frequency_error() {
        let previous = [Sample::ONE; CARRIERS];
        let ahead = Sample::from_polar(1.0, 0.1);
        let steady = decide(&symbols_with(Sample::ONE, ahead), &previous);
        assert!(!steady.sync_bit);
        assert!(steady.frequency_error < -0.09);
        let flipped = decide(&symbols_with(Sample::ONE, -ahead), &previous);
        assert!(flipped.sync_bit);
        assert!(flipped.frequency_error < -0.09);
    }
}
