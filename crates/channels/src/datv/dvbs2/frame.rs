use num_complex::Complex;

use super::ldpc::{NORMAL, Rate, SHORT};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modulation {
    Qpsk,
    Psk8,
}

impl Modulation {
    #[must_use]
    pub const fn bits(self) -> usize {
        match self {
            Self::Qpsk => 2,
            Self::Psk8 => 3,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Qpsk => "QPSK",
            Self::Psk8 => "8PSK",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModCod {
    pub index: u8,
    pub modulation: Modulation,
    pub rate: Rate,
}

const CATALOGUE: [(u8, Modulation, Rate); 14] = [
    (4, Modulation::Qpsk, Rate::R1_2),
    (5, Modulation::Qpsk, Rate::R3_5),
    (6, Modulation::Qpsk, Rate::R2_3),
    (7, Modulation::Qpsk, Rate::R3_4),
    (8, Modulation::Qpsk, Rate::R4_5),
    (9, Modulation::Qpsk, Rate::R5_6),
    (10, Modulation::Qpsk, Rate::R8_9),
    (11, Modulation::Qpsk, Rate::R9_10),
    (13, Modulation::Psk8, Rate::R2_3),
    (14, Modulation::Psk8, Rate::R3_4),
    (15, Modulation::Psk8, Rate::R5_6),
    (16, Modulation::Psk8, Rate::R8_9),
    (17, Modulation::Psk8, Rate::R9_10),
    (12, Modulation::Psk8, Rate::R3_5),
];

impl ModCod {
    #[must_use]
    pub fn from_index(index: u8) -> Option<Self> {
        CATALOGUE
            .iter()
            .find(|&&(catalogued, ..)| catalogued == index)
            .filter(|&&(catalogued, ..)| catalogued != 12)
            .map(|&(index, modulation, rate)| Self {
                index,
                modulation,
                rate,
            })
    }

    #[must_use]
    pub fn find(modulation: Modulation, rate: Rate) -> Option<Self> {
        CATALOGUE
            .iter()
            .find(|&&(index, catalogued, catalogued_rate)| {
                index != 12 && catalogued == modulation && catalogued_rate == rate
            })
            .map(|&(index, modulation, rate)| Self {
                index,
                modulation,
                rate,
            })
    }

    #[must_use]
    pub const fn length(self, short: bool) -> usize {
        if short { SHORT } else { NORMAL }
    }

    #[must_use]
    pub fn symbols(self, short: bool) -> usize {
        self.length(short) / self.modulation.bits()
    }

    #[must_use]
    pub fn slots(self, short: bool) -> usize {
        self.symbols(short) / 90
    }

    #[must_use]
    pub const fn correct(self, short: bool) -> usize {
        if short {
            12
        } else {
            match self.rate {
                Rate::R2_3 | Rate::R5_6 => 10,
                Rate::R8_9 | Rate::R9_10 => 8,
                _ => 12,
            }
        }
    }
}

#[must_use]
pub fn interleave(coded: &[bool], modulation: Modulation) -> Vec<bool> {
    let columns = modulation.bits();
    if columns < 3 {
        return coded.to_vec();
    }
    let rows = coded.len() / columns;
    let mut out = vec![false; coded.len()];
    for row in 0..rows {
        for column in 0..columns {
            out[row * columns + column] = coded[column * rows + row];
        }
    }
    out
}

#[must_use]
pub fn deinterleave(llrs: &[f32], modulation: Modulation) -> Vec<f32> {
    let columns = modulation.bits();
    if columns < 3 {
        return llrs.to_vec();
    }
    let rows = llrs.len() / columns;
    let mut out = vec![0.0; llrs.len()];
    for row in 0..rows {
        for column in 0..columns {
            out[column * rows + row] = llrs[row * columns + column];
        }
    }
    out
}

const QPSK: [Complex<f32>; 4] = [
    Complex::new(
        std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
    ),
    Complex::new(
        std::f32::consts::FRAC_1_SQRT_2,
        -std::f32::consts::FRAC_1_SQRT_2,
    ),
    Complex::new(
        -std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
    ),
    Complex::new(
        -std::f32::consts::FRAC_1_SQRT_2,
        -std::f32::consts::FRAC_1_SQRT_2,
    ),
];

const PSK8_PHASES: [u8; 8] = [1, 0, 4, 5, 2, 7, 6, 3];

#[must_use]
pub fn point(modulation: Modulation, label: usize) -> Complex<f32> {
    match modulation {
        Modulation::Qpsk => QPSK[label & 3],
        Modulation::Psk8 => Complex::from_polar(
            1.0,
            f32::from(PSK8_PHASES[label & 7]) * std::f32::consts::FRAC_PI_4,
        ),
    }
}

pub fn modulate(bits: &[bool], modulation: Modulation, out: &mut Vec<Complex<f32>>) {
    let width = modulation.bits();
    for chunk in bits.chunks_exact(width) {
        let label = chunk
            .iter()
            .fold(0usize, |value, &bit| value << 1 | usize::from(bit));
        out.push(point(modulation, label));
    }
}

pub fn demodulate(
    symbols: &[Complex<f32>],
    modulation: Modulation,
    noise: f32,
    out: &mut Vec<f32>,
) {
    let width = modulation.bits();
    let count = 1usize << width;
    let scale = 1.0 / noise.max(1e-6);
    for &symbol in symbols {
        for bit in 0..width {
            let mut zero = f32::NEG_INFINITY;
            let mut one = f32::NEG_INFINITY;
            for label in 0..count {
                let metric = -(symbol - point(modulation, label)).norm_sqr() * scale;
                if label >> (width - 1 - bit) & 1 == 0 {
                    zero = zero.max(metric);
                } else {
                    one = one.max(metric);
                }
            }
            out.push(zero - one);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalogued_mode_round_trips_through_its_index() {
        for &(index, modulation, rate) in &CATALOGUE {
            let Some(modcod) = ModCod::from_index(index) else {
                assert_eq!(index, 12, "only 8PSK 3/5 is left out");
                continue;
            };
            assert_eq!(modcod.modulation, modulation);
            assert_eq!(modcod.rate, rate);
            assert_eq!(ModCod::find(modulation, rate), Some(modcod));
        }
        assert!(ModCod::from_index(0).is_none());
        assert!(ModCod::from_index(18).is_none());
    }

    #[test]
    fn a_frame_is_a_whole_number_of_slots() {
        for &(index, ..) in &CATALOGUE {
            let Some(modcod) = ModCod::from_index(index) else {
                continue;
            };
            for short in [true, false] {
                assert!(
                    modcod.symbols(short).is_multiple_of(90),
                    "modcod {index} short={short}"
                );
                assert_eq!(modcod.slots(short) * 90, modcod.symbols(short));
            }
        }
    }

    #[test]
    fn the_bit_interleaver_is_reversible() {
        for modulation in [Modulation::Qpsk, Modulation::Psk8] {
            let coded: Vec<bool> = (0..NORMAL).map(|index| index % 5 == 0).collect();
            let sent = interleave(&coded, modulation);
            let llrs: Vec<f32> = sent
                .iter()
                .map(|&bit| if bit { -1.0 } else { 1.0 })
                .collect();
            let restored = deinterleave(&llrs, modulation);
            let bits: Vec<bool> = restored.iter().map(|&value| value < 0.0).collect();
            assert_eq!(bits, coded, "{modulation:?}");
        }
    }

    #[test]
    fn every_constellation_point_is_unit_energy_and_distinct() {
        for modulation in [Modulation::Qpsk, Modulation::Psk8] {
            let points: Vec<Complex<f32>> = (0..1 << modulation.bits())
                .map(|label| point(modulation, label))
                .collect();
            for value in &points {
                assert!((value.norm() - 1.0).abs() < 1e-6, "{modulation:?}");
            }
            for (index, first) in points.iter().enumerate() {
                for second in &points[index + 1..] {
                    assert!((first - second).norm() > 0.5, "{modulation:?}");
                }
            }
        }
    }

    #[test]
    fn clean_symbols_demodulate_back_to_their_bits() {
        for modulation in [Modulation::Qpsk, Modulation::Psk8] {
            let bits: Vec<bool> = (0..3 * 8 * modulation.bits())
                .map(|index| index % 3 == 0 || index % 7 == 1)
                .collect();
            let mut symbols = Vec::new();
            modulate(&bits, modulation, &mut symbols);
            let mut llrs = Vec::new();
            demodulate(&symbols, modulation, 0.1, &mut llrs);
            let decoded: Vec<bool> = llrs.iter().map(|&value| value < 0.0).collect();
            assert_eq!(decoded, bits, "{modulation:?}");
        }
    }
}
