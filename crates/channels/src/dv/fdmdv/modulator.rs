use std::f32::consts::SQRT_2;

use super::{
    BITS, CARRIERS, CENTRE_HZ, CODEC2_PI, DATA_CARRIERS, PILOT, SYMBOL_SAMPLES, SYMBOL_SPAN,
    Sample, carrier_radians, cis, rotation, shaped, unit,
};

pub(super) struct Modulator {
    previous: [Sample; CARRIERS],
    invert_pilot: bool,
    history: [[Sample; SYMBOL_SPAN]; CARRIERS],
    phases: [Sample; CARRIERS],
    rotations: [Sample; CARRIERS],
    centre_phase: Sample,
    centre_rotation: Sample,
}

impl Modulator {
    pub(super) fn new() -> Self {
        let mut previous = [Sample::ONE; CARRIERS];
        previous[PILOT] = Sample::new(2.0, 0.0);
        Self {
            previous,
            invert_pilot: false,
            history: [[Sample::ZERO; SYMBOL_SPAN]; CARRIERS],
            phases: std::array::from_fn(|c| {
                cis((2.0 * CODEC2_PI * c as f64 / CARRIERS as f64) as f32)
            }),
            rotations: std::array::from_fn(|c| cis(carrier_radians(c))),
            centre_phase: Sample::ONE,
            centre_rotation: rotation(CENTRE_HZ),
        }
    }

    pub(super) fn modulate(&mut self, bits: &[bool; BITS]) -> [Sample; SYMBOL_SAMPLES] {
        let symbols = self.symbols(bits);
        self.previous = symbols;
        let gain = Sample::new(SQRT_2 / 2.0, 0.0);
        let mut output = [Sample::ZERO; SYMBOL_SAMPLES];
        for (carrier, symbol) in symbols.iter().enumerate() {
            let history = &mut self.history[carrier];
            history[SYMBOL_SPAN - 1] = symbol * gain;
            for (offset, out) in output.iter_mut().enumerate() {
                let baseband = shaped(history, offset);
                self.phases[carrier] *= self.rotations[carrier];
                *out += baseband * self.phases[carrier];
            }
            history.rotate_left(1);
            history[SYMBOL_SPAN - 1] = Sample::ZERO;
        }
        for out in &mut output {
            self.centre_phase *= self.centre_rotation;
            *out = Sample::new(2.0, 0.0) * (*out * self.centre_phase);
        }
        self.phases = self.phases.map(unit);
        self.centre_phase = unit(self.centre_phase);
        output
    }

    fn symbols(&mut self, bits: &[bool; BITS]) -> [Sample; CARRIERS] {
        let mut symbols = self.previous;
        for (symbol, pair) in symbols[..DATA_CARRIERS]
            .iter_mut()
            .zip(bits.as_chunks::<2>().0)
        {
            *symbol = match pair {
                [false, false] => *symbol,
                [false, true] => Sample::I * *symbol,
                [true, false] => -Sample::I * *symbol,
                [true, true] => -*symbol,
            };
        }
        if self.invert_pilot {
            symbols[PILOT] = -symbols[PILOT];
        }
        self.invert_pilot = !self.invert_pilot;
        symbols
    }
}
