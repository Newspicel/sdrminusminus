use num_complex::Complex;

pub const EQ_TAPS: usize = 7;

pub struct Lms {
    weights: [Complex<f32>; EQ_TAPS],
    window: [Complex<f32>; EQ_TAPS],
    pos: usize,
}

impl Lms {
    pub fn new() -> Self {
        let mut weights = [Complex::new(0.0f32, 0.0); EQ_TAPS];
        weights[EQ_TAPS / 2] = Complex::new(1.0, 0.0);
        Self {
            weights,
            window: [Complex::new(0.0, 0.0); EQ_TAPS],
            pos: 0,
        }
    }

    pub fn push(&mut self, sample: Complex<f32>) {
        self.pos = (self.pos + 1) % EQ_TAPS;
        self.window[self.pos] = sample;
    }

    fn delayed(&self, k: usize) -> Complex<f32> {
        self.window[(self.pos + EQ_TAPS - k) % EQ_TAPS]
    }

    pub fn exec(&self) -> Complex<f32> {
        let mut y = Complex::new(0.0f32, 0.0);
        for (k, weight) in self.weights.iter().enumerate() {
            y += weight * self.delayed(k);
        }
        y
    }

    pub fn step(&mut self, reference: Complex<f32>, output: Complex<f32>, mu: f32) {
        let error = reference - output;
        let energy: f32 = self
            .window
            .iter()
            .map(|v| v.norm_sqr())
            .sum::<f32>()
            .max(1e-9);
        let gain = mu / energy;
        for k in 0..EQ_TAPS {
            let delayed = self.delayed(k);
            self.weights[k] += gain * error * delayed.conj();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converges_on_scalar_channel() {
        let channel = Complex::new(0.0, 1.46f32);
        let mut rng = 0x12345u64;
        let mut bit = || {
            rng = rng.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            ((rng >> 33) & 1) as u8
        };
        let mut eq = Lms::new();
        let mut symbols: Vec<f32> = Vec::new();
        for _ in 0..EQ_TAPS {
            let s = if bit() == 1 { -1.0 } else { 1.0 };
            symbols.push(s);
            eq.push(channel * s);
        }
        let mut errors = Vec::new();
        for _ in 0..200 {
            let y = eq.exec();
            let d = Complex::new(symbols[symbols.len() - 4], 0.0);
            eq.step(d, y, 0.10);
            errors.push((d - y).norm());
            let s = if bit() == 1 { -1.0 } else { 1.0 };
            symbols.push(s);
            eq.push(channel * s);
        }
        let late: f32 = errors[180..].iter().sum::<f32>() / 20.0;
        assert!(late < 0.2, "late error {late}");
    }
}
