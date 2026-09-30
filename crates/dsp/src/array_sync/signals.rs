use std::f64::consts::TAU;

use num_complex::Complex;

use crate::fft::{FftPair, Transform};

pub(super) struct Gaussian {
    state: u64,
}

impl Gaussian {
    pub(super) fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
        }
    }

    fn uniform(&mut self) -> f64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        let bits = self.state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11;
        (bits as f64 + 1.0) / (1u64 << 53) as f64
    }

    pub(super) fn sample(&mut self) -> Complex<f32> {
        let radius = (-self.uniform().ln()).sqrt();
        let angle = TAU * self.uniform();
        Complex::new((radius * angle.cos()) as f32, (radius * angle.sin()) as f32)
    }

    pub(super) fn block(&mut self, len: usize) -> Vec<Complex<f32>> {
        (0..len).map(|_| self.sample()).collect()
    }
}

pub(super) fn frequency(bin: usize, len: usize) -> f64 {
    if bin < len.div_ceil(2) {
        bin as f64 / len as f64
    } else {
        (bin as f64 - len as f64) / len as f64
    }
}

pub(super) fn shaped(
    input: &[Complex<f32>],
    response: impl Fn(f64) -> Complex<f64>,
) -> Vec<Complex<f32>> {
    let len = input.len();
    let mut fft = FftPair::<f64>::new(len);
    let mut spectrum: Vec<Complex<f64>> = input
        .iter()
        .map(|value| Complex::new(f64::from(value.re), f64::from(value.im)))
        .collect();
    fft.forward(&mut spectrum);
    for (bin, value) in spectrum.iter_mut().enumerate() {
        *value *= response(frequency(bin, len));
    }
    fft.inverse(&mut spectrum);
    spectrum
        .iter()
        .map(|value| {
            Complex::new(
                (value.re / len as f64) as f32,
                (value.im / len as f64) as f32,
            )
        })
        .collect()
}

pub(super) fn lane_response(delay: f64, phase_rad: f64, gain: f64) -> impl Fn(f64) -> Complex<f64> {
    move |nu| Complex::from_polar(gain, phase_rad - TAU * nu * delay)
}

pub(super) fn with_noise(
    input: &[Complex<f32>],
    amplitude: f32,
    noise: &mut Gaussian,
) -> Vec<Complex<f32>> {
    input
        .iter()
        .map(|value| value + noise.sample() * amplitude)
        .collect()
}

pub(super) fn delay_between(
    reference: &[Complex<f32>],
    lane: &[Complex<f32>],
    block: usize,
) -> f64 {
    let mut fft = Transform::<f64>::forward(block);
    let window: Vec<f64> = (0..block)
        .map(|n| 0.5 - 0.5 * (TAU * n as f64 / block as f64).cos())
        .collect();
    let mut cross = vec![Complex::<f64>::default(); block];
    let mut start = 0;
    while start + block <= reference.len().min(lane.len()) {
        let mut spectrum = |samples: &[Complex<f32>]| {
            let mut buf: Vec<Complex<f64>> = samples[start..start + block]
                .iter()
                .zip(&window)
                .map(|(value, w)| Complex::new(f64::from(value.re), f64::from(value.im)) * w)
                .collect();
            fft.process(&mut buf);
            buf
        };
        let a = spectrum(reference);
        let b = spectrum(lane);
        for ((sum, a), b) in cross.iter_mut().zip(&a).zip(&b) {
            *sum += b * a.conj();
        }
        start += block / 2;
    }
    let (mut sw, mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (bin, value) in cross.iter().enumerate() {
        let nu = frequency(bin, block);
        if nu.abs() > 0.4 {
            continue;
        }
        let (w, x, y) = (value.norm(), -TAU * nu, value.arg());
        sw += w;
        sx += w * x;
        sy += w * y;
        sxx += w * x * x;
        sxy += w * x * y;
    }
    (sw * sxy - sx * sy) / (sw * sxx - sx * sx)
}
