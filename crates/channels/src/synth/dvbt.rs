use num_complex::Complex;
use rustfft::FftPlanner;
use sdrmm_wire::DatvCodeRate;

use crate::datv::{
    dvbs::DvbsEncoder,
    dvbt::{
        mapping::{self, Mapping},
        tps::{self, Parameters},
    },
};

pub fn defaults() -> Parameters {
    Parameters {
        fft: 2048,
        guard: 256,
        bits: 4,
        alpha: 1,
        hierarchical: false,
        high_rate: DatvCodeRate::ThreeQuarters,
        low_rate: DatvCodeRate::Half,
        frame: 0,
        cell: 0x12,
    }
}

pub fn waveform(mut params: Parameters, symbols: usize) -> Vec<Complex<f32>> {
    let map = Mapping::new(params.fft);
    let inverse = FftPlanner::new().plan_fft_inverse(params.fft);
    let mut encoder = DvbsEncoder::new(params.high_rate);
    let mut low_encoder = DvbsEncoder::new(params.low_rate);
    let mut multiplex = super::datv::Multiplex::new();
    let mut low_multiplex = super::datv::Multiplex::new();
    let mut high = std::collections::VecDeque::new();
    let mut low = std::collections::VecDeque::new();
    let mut iq = Vec::with_capacity(symbols * (params.fft + params.guard));
    let mut tps_bits = tps::encode(params);
    let mut tps_phase = 1.0;
    for symbol in 0..symbols {
        if symbol % 68 == 0 {
            params.frame = ((symbol / 68) % 4) as u8;
            tps_bits = tps::encode(params);
            tps_phase = 1.0;
        } else if tps_bits[symbol % 68] {
            tps_phase = -tps_phase;
        }
        let words = interleave(
            &mut high,
            &mut low,
            &mut encoder,
            &mut low_encoder,
            &mut multiplex,
            &mut low_multiplex,
            params,
        );
        let mut spectrum = vec![Complex::new(0.0, 0.0); params.fft];
        let mut symbols = vec![0usize; map.permutation.len()];
        for (i, &p) in map.permutation.iter().enumerate() {
            if symbol % 2 == 0 {
                symbols[p] = words[i];
            } else {
                symbols[i] = words[p];
            }
        }
        for (i, &k) in map.data[symbol % 4].iter().enumerate() {
            spectrum[map.bin(k, 0)] = mapping::point(symbols[i], params.bits, params.alpha);
        }
        for &k in &map.pilots[symbol % 4] {
            spectrum[map.bin(k, 0)] = Complex::new(map.reference[k] * 4.0 / 3.0, 0.0);
        }
        for &k in &map.tps {
            spectrum[map.bin(k, 0)] = Complex::new(map.reference[k] * tps_phase, 0.0);
        }
        inverse.process(&mut spectrum);
        let scale = 1.0 / (params.fft as f32).sqrt();
        iq.extend(
            spectrum[params.fft - params.guard..]
                .iter()
                .chain(&spectrum)
                .map(|v| v * scale),
        );
    }
    iq
}

fn fill(
    bits: &mut std::collections::VecDeque<bool>,
    encoder: &mut DvbsEncoder,
    multiplex: &mut super::datv::Multiplex,
    count: usize,
) {
    let mut symbols = Vec::new();
    while bits.len() < count {
        symbols.clear();
        encoder.packet(&multiplex.packet(), &mut symbols);
        for point in &symbols {
            bits.extend([point.re < 0.0, point.im < 0.0]);
        }
    }
}

fn interleave(
    high: &mut std::collections::VecDeque<bool>,
    low: &mut std::collections::VecDeque<bool>,
    encoder: &mut DvbsEncoder,
    low_encoder: &mut DvbsEncoder,
    multiplex: &mut super::datv::Multiplex,
    low_multiplex: &mut super::datv::Multiplex,
    params: Parameters,
) -> Vec<usize> {
    let count = params.fft * 189 / 256;
    let mut words = vec![0usize; count];
    fill(high, encoder, multiplex, count * params.bits);
    fill(low, low_encoder, low_multiplex, count * params.bits);
    let high_lanes: &[usize] = match (params.bits, params.hierarchical) {
        (_, true) => &[0, 1],
        (2, _) => &[0, 1],
        (4, _) => &[0, 2, 1, 3],
        _ => &[0, 2, 4, 1, 3, 5],
    };
    let low_lanes: &[usize] = if !params.hierarchical {
        &[]
    } else if params.bits == 4 {
        &[2, 3]
    } else {
        &[2, 4, 3, 5]
    };
    let shifts = [0, 63, 105, 42, 21, 84];
    for block in words.as_chunks_mut::<126>().0 {
        let mut lanes = [[false; 6]; 126];
        for row in &mut lanes {
            for &lane in high_lanes {
                row[lane] = high.pop_front().unwrap_or(false);
            }
            for &lane in low_lanes {
                row[lane] = low.pop_front().unwrap_or(false);
            }
        }
        for (i, word) in block.iter_mut().enumerate() {
            for bit in 0..params.bits {
                *word = (*word << 1) | usize::from(lanes[(i + shifts[bit]) % 126][bit]);
            }
        }
    }
    words
}
