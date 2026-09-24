use num_complex::Complex;

use super::demod::SYMBOL_RATE;
use super::frame::symbol_reverse;
use super::receiver::rrc_taps;

pub fn bits_to_symbols(bits: &[u8]) -> Vec<u8> {
    const INV_MAP: [u8; 4] = [0, 3, 1, 2];
    let mut old = 0u8;
    bits.as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            old = (old + INV_MAP[usize::from((pair[0] << 1) | pair[1])]) % 4;
            old
        })
        .collect()
}

pub fn modulate(
    bits: &[u8],
    pre_syms: usize,
    sample_rate: f64,
    freq_offset_hz: f64,
    amplitude: f32,
) -> Vec<Complex<f32>> {
    let sps = sample_rate / SYMBOL_RATE;
    let symbols = bits_to_symbols(&symbol_reverse(bits));
    let total = ((pre_syms + symbols.len() + 4) as f64 * sps) as usize;
    let mut impulses = vec![Complex::new(0.0f32, 0.0); total];
    for k in 0..pre_syms {
        impulses[(k as f64 * sps) as usize] = Complex::new(1.0, 0.0);
    }
    for (k, &q) in symbols.iter().enumerate() {
        let at = ((pre_syms + k) as f64 * sps) as usize;
        impulses[at] = Complex::from_polar(1.0, f32::from(q) * std::f32::consts::FRAC_PI_2);
    }
    let taps = rrc_taps(sps, ((8.0 * sps) as usize) | 1, 0.4);
    (0..total)
        .map(|n| {
            let shaped: Complex<f32> = taps
                .iter()
                .enumerate()
                .take(n + 1)
                .map(|(k, &tap)| impulses[n - k] * tap)
                .sum();
            let phase = std::f64::consts::TAU * freq_offset_hz * n as f64 / sample_rate;
            shaped * Complex::from_polar(amplitude, phase as f32)
        })
        .collect()
}
