use std::{f32::consts::PI, sync::Arc};

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use sdrmm_dsp::{CONFIDENT, Soft};

pub const CARRIERS: usize = 1_536;
pub const USEFUL: usize = 2_048;
pub const GUARD: usize = 504;
pub const SYMBOL: usize = USEFUL + GUARD;
pub const NULL: usize = 2_656;
pub const SYMBOLS: usize = 76;
pub const FRAME: usize = NULL + SYMBOLS * SYMBOL;
pub const FIC_SYMBOLS: std::ops::Range<usize> = 1..4;
pub const MSC_SYMBOLS: std::ops::Range<usize> = 4..SYMBOLS;
pub const SYMBOL_BITS: usize = 2 * CARRIERS;

const PHASE_STEPS: [(i16, i16, u8, u8); 48] = [
    (-768, -737, 0, 1),
    (-736, -705, 1, 2),
    (-704, -673, 2, 0),
    (-672, -641, 3, 1),
    (-640, -609, 0, 3),
    (-608, -577, 1, 2),
    (-576, -545, 2, 2),
    (-544, -513, 3, 3),
    (-512, -481, 0, 2),
    (-480, -449, 1, 1),
    (-448, -417, 2, 2),
    (-416, -385, 3, 3),
    (-384, -353, 0, 1),
    (-352, -321, 1, 2),
    (-320, -289, 2, 3),
    (-288, -257, 3, 3),
    (-256, -225, 0, 2),
    (-224, -193, 1, 2),
    (-192, -161, 2, 2),
    (-160, -129, 3, 1),
    (-128, -97, 0, 1),
    (-96, -65, 1, 3),
    (-64, -33, 2, 1),
    (-32, -1, 3, 2),
    (1, 32, 0, 3),
    (33, 64, 3, 1),
    (65, 96, 2, 1),
    (97, 128, 1, 1),
    (129, 160, 0, 2),
    (161, 192, 3, 2),
    (193, 224, 2, 1),
    (225, 256, 1, 0),
    (257, 288, 0, 2),
    (289, 320, 3, 2),
    (321, 352, 2, 3),
    (353, 384, 1, 3),
    (385, 416, 0, 0),
    (417, 448, 3, 2),
    (449, 480, 2, 1),
    (481, 512, 1, 3),
    (513, 544, 0, 3),
    (545, 576, 3, 3),
    (577, 608, 2, 3),
    (609, 640, 1, 0),
    (641, 672, 0, 3),
    (673, 704, 3, 0),
    (705, 736, 2, 1),
    (737, 768, 1, 1),
];

const H: [[u8; 32]; 4] = [
    [
        0, 2, 0, 0, 0, 0, 1, 1, 2, 0, 0, 0, 2, 2, 1, 1, 0, 2, 0, 0, 0, 0, 1, 1, 2, 0, 0, 0, 2, 2,
        1, 1,
    ],
    [
        0, 3, 2, 3, 0, 1, 3, 0, 2, 1, 2, 3, 2, 3, 3, 0, 0, 3, 2, 3, 0, 1, 3, 0, 2, 1, 2, 3, 2, 3,
        3, 0,
    ],
    [
        0, 0, 0, 2, 0, 2, 1, 3, 2, 2, 0, 2, 2, 0, 1, 3, 0, 0, 0, 2, 0, 2, 1, 3, 2, 2, 0, 2, 2, 0,
        1, 3,
    ],
    [
        0, 1, 2, 1, 0, 3, 3, 2, 2, 3, 2, 1, 2, 1, 3, 2, 0, 1, 2, 1, 0, 3, 3, 2, 2, 3, 2, 1, 2, 1,
        3, 2,
    ],
];

#[must_use]
pub fn reference_phase(carrier: i16) -> Option<f32> {
    let &(low, _, table, offset) = PHASE_STEPS
        .iter()
        .find(|&&(low, high, _, _)| (low..=high).contains(&carrier))?;
    let step = usize::try_from(carrier - low).ok()?;
    Some(PI / 2.0 * f32::from(H[usize::from(table)][step] + offset))
}

#[must_use]
pub fn reference_symbol() -> Vec<Complex<f32>> {
    let mut bins = vec![Complex::new(0.0, 0.0); USEFUL];
    for carrier in -768i16..=768 {
        if carrier == 0 {
            continue;
        }
        let Some(phase) = reference_phase(carrier) else {
            continue;
        };
        bins[carrier_bin(carrier)] = Complex::from_polar(1.0, phase);
    }
    bins
}

#[must_use]
pub fn carrier_bin(carrier: i16) -> usize {
    (i32::from(carrier).rem_euclid(USEFUL as i32)) as usize
}

#[must_use]
pub fn interleaving() -> Vec<usize> {
    let mut value = 0usize;
    let mut table = Vec::with_capacity(CARRIERS);
    for _ in 0..USEFUL {
        value = (13 * value + 511) % USEFUL;
        if value == USEFUL / 2 || !(256..=1_792).contains(&value) {
            continue;
        }
        table.push(carrier_bin((value as i32 - USEFUL as i32 / 2) as i16));
    }
    table
}

fn clamp(value: f32) -> Soft {
    let limit = f32::from(CONFIDENT);
    (value * limit).clamp(-limit, limit) as Soft
}

pub struct SymbolDemod {
    fft: Arc<dyn Fft<f32>>,
    bins: Vec<usize>,
    scratch: Vec<Complex<f32>>,
    previous: Vec<Complex<f32>>,
    current: Vec<Complex<f32>>,
    have_reference: bool,
}

impl SymbolDemod {
    #[must_use]
    pub fn new() -> Self {
        let mut planner = FftPlanner::<f32>::new();
        Self {
            fft: planner.plan_fft_forward(USEFUL),
            bins: interleaving(),
            scratch: vec![Complex::new(0.0, 0.0); USEFUL],
            previous: vec![Complex::new(0.0, 0.0); USEFUL],
            current: vec![Complex::new(0.0, 0.0); USEFUL],
            have_reference: false,
        }
    }

    pub fn reset(&mut self) {
        self.have_reference = false;
    }

    #[must_use]
    pub fn spectrum(&self) -> &[Complex<f32>] {
        &self.current
    }

    pub fn transform(&mut self, symbol: &[Complex<f32>]) {
        std::mem::swap(&mut self.previous, &mut self.current);
        self.scratch.clear();
        self.scratch
            .extend_from_slice(&symbol[GUARD..GUARD + USEFUL]);
        self.fft.process(&mut self.scratch);
        self.current.copy_from_slice(&self.scratch);
    }

    pub fn demodulate(&mut self, symbol: &[Complex<f32>], out: &mut Vec<Soft>) -> bool {
        if symbol.len() < SYMBOL {
            return false;
        }
        self.transform(symbol);
        if !self.have_reference {
            self.have_reference = true;
            return false;
        }
        let mut power = 0.0f32;
        for &bin in &self.bins {
            power += self.current[bin].norm_sqr() + self.previous[bin].norm_sqr();
        }
        let mean = (power / (2 * self.bins.len()) as f32).max(1e-20);
        let scale = std::f32::consts::SQRT_2 / mean;
        let start = out.len();
        out.resize(start + SYMBOL_BITS, 0);
        for (index, &bin) in self.bins.iter().enumerate() {
            let product = self.current[bin] * self.previous[bin].conj();
            out[start + index] = clamp(-product.re * scale);
            out[start + CARRIERS + index] = clamp(-product.im * scale);
        }
        true
    }
}

impl Default for SymbolDemod {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use]
pub fn map_symbol(bits: &[bool]) -> Vec<Complex<f32>> {
    let amplitude = std::f32::consts::FRAC_1_SQRT_2;
    (0..CARRIERS)
        .map(|index| {
            Complex::new(
                if bits[index] { -amplitude } else { amplitude },
                if bits[CARRIERS + index] {
                    -amplitude
                } else {
                    amplitude
                },
            )
        })
        .collect()
}

pub struct FrameSync {
    average: f32,
    quiet: usize,
    started: bool,
}

impl FrameSync {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            average: 0.0,
            quiet: 0,
            started: false,
        }
    }

    pub fn reset(&mut self) {
        self.average = 0.0;
        self.quiet = 0;
        self.started = false;
    }

    pub fn push(&mut self, sample: Complex<f32>) -> bool {
        let power = sample.norm_sqr();
        if self.average == 0.0 {
            self.average = power.max(1e-12);
        }
        let quiet = power < self.average * 0.15;
        if quiet {
            self.quiet += 1;
        } else {
            self.average += 0.00002 * (power - self.average);
            let ended = self.started && (NULL / 2..2 * NULL).contains(&self.quiet);
            self.quiet = 0;
            self.started = true;
            return ended;
        }
        self.started = true;
        false
    }
}

impl Default for FrameSync {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use]
pub fn prefix_offset(frame: &[Complex<f32>]) -> Option<(f32, f32)> {
    if frame.len() < SYMBOL {
        return None;
    }
    let mut correlation = Complex::new(0.0f32, 0.0);
    let mut energy = 0.0f32;
    for index in 0..GUARD {
        let prefix = frame[index];
        let tail = frame[USEFUL + index];
        correlation += prefix * tail.conj();
        energy += prefix.norm_sqr() + tail.norm_sqr();
    }
    let coherence = 2.0 * correlation.norm() / energy.max(1e-20);
    Some((coherence, -correlation.arg() / (2.0 * PI * USEFUL as f32)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interleaving_table_covers_every_carrier_exactly_once() {
        let table = interleaving();
        assert_eq!(table.len(), CARRIERS);
        let mut seen = table.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), CARRIERS);
        assert!(seen.iter().all(|&bin| bin <= 768 || bin >= 1_280));
        assert!(!seen.contains(&0));
        assert_eq!(table[0], carrier_bin(511 - 1_024));
    }

    #[test]
    fn the_reference_symbol_has_unit_carriers_on_the_documented_grid() {
        let bins = reference_symbol();
        assert_eq!(bins[0], Complex::new(0.0, 0.0));
        for carrier in [-768i16, -1, 1, 768] {
            assert!(
                (bins[carrier_bin(carrier)].norm() - 1.0).abs() < 1e-6,
                "carrier {carrier}"
            );
        }
        assert!((reference_phase(-768).expect("first carrier") - 0.5 * PI).abs() < 1e-6);
        assert!(reference_phase(0).is_none());
        assert!(reference_phase(769).is_none());
    }

    fn modulate(symbols: &[Vec<bool>]) -> Vec<Complex<f32>> {
        let table = interleaving();
        let mut planner = FftPlanner::<f32>::new();
        let inverse = planner.plan_fft_inverse(USEFUL);
        let mut previous: Vec<Complex<f32>> = {
            let reference = reference_symbol();
            table.iter().map(|&bin| reference[bin]).collect()
        };
        let mut iq = vec![Complex::new(0.0, 0.0); NULL];
        let emit = |points: &[Complex<f32>], table: &[usize], out: &mut Vec<Complex<f32>>| {
            let mut bins = vec![Complex::new(0.0, 0.0); USEFUL];
            for (index, &bin) in table.iter().enumerate() {
                bins[bin] = points[index];
            }
            inverse.process(&mut bins);
            let scale = 1.0 / (USEFUL as f32).sqrt();
            out.extend(bins[USEFUL - GUARD..].iter().map(|&value| value * scale));
            out.extend(bins.iter().map(|&value| value * scale));
        };
        emit(&previous, &table, &mut iq);
        for bits in symbols {
            let mapped = map_symbol(bits);
            let points: Vec<Complex<f32>> = mapped
                .iter()
                .zip(&previous)
                .map(|(&point, &reference)| point * reference)
                .collect();
            emit(&points, &table, &mut iq);
            previous = points;
        }
        iq
    }

    fn payload(index: usize) -> Vec<bool> {
        let mut state = 0x2545_F491u32 ^ index as u32;
        (0..SYMBOL_BITS)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state & 1 == 1
            })
            .collect()
    }

    #[test]
    fn a_differentially_modulated_symbol_comes_back_bit_for_bit() {
        let sent: Vec<Vec<bool>> = (0..4).map(payload).collect();
        let iq = modulate(&sent);
        let mut demod = SymbolDemod::new();
        let mut bits = Vec::new();
        for index in 0..=sent.len() {
            let start = NULL + index * SYMBOL;
            demod.demodulate(&iq[start..start + SYMBOL], &mut bits);
        }
        assert_eq!(bits.len(), sent.len() * SYMBOL_BITS);
        for (index, expected) in sent.iter().enumerate() {
            let decoded: Vec<bool> = bits[index * SYMBOL_BITS..(index + 1) * SYMBOL_BITS]
                .iter()
                .map(|&value| value > 0)
                .collect();
            assert_eq!(&decoded, expected, "symbol {index}");
        }
    }

    #[test]
    fn the_null_symbol_marks_the_start_of_a_frame() {
        let iq = modulate(&(0..4).map(payload).collect::<Vec<_>>());
        let mut sync = FrameSync::new();
        let mut starts = Vec::new();
        let mut lead = vec![Complex::new(0.4, -0.3); 4_000];
        lead.extend_from_slice(&iq);
        for (index, &sample) in lead.iter().enumerate() {
            if sync.push(sample) {
                starts.push(index);
            }
        }
        assert_eq!(starts.len(), 1);
        assert!(
            starts[0].abs_diff(4_000 + NULL) < 8,
            "found the frame at {} rather than {}",
            starts[0],
            4_000 + NULL
        );
    }

    #[test]
    fn the_cyclic_prefix_reports_a_clean_symbol_and_no_offset() {
        let iq = modulate(&(0..2).map(payload).collect::<Vec<_>>());
        let (coherence, offset) = prefix_offset(&iq[NULL..]).expect("a full symbol");
        assert!(coherence > 0.9, "coherence {coherence}");
        assert!(offset.abs() < 1e-6, "offset {offset}");
    }

    #[test]
    fn a_frequency_offset_shows_up_in_the_prefix_phase() {
        let iq = modulate(&(0..2).map(payload).collect::<Vec<_>>());
        let shift = 120.0f32;
        let rate = 2_048_000.0f32;
        let turned: Vec<Complex<f32>> = iq
            .iter()
            .enumerate()
            .map(|(index, &value)| {
                value * Complex::from_polar(1.0, 2.0 * PI * shift * index as f32 / rate)
            })
            .collect();
        let (_, offset) = prefix_offset(&turned[NULL..]).expect("a full symbol");
        assert!(
            (offset * rate - shift).abs() < 2.0,
            "estimated {} Hz for {shift} Hz",
            offset * rate
        );
    }
}
