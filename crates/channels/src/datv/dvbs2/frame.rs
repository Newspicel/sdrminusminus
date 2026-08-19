use num_complex::Complex;

use super::ldpc::{NORMAL, Rate, SHORT};

pub const MAX_POINTS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modulation {
    Qpsk,
    Psk8,
    Apsk16,
    Apsk32,
}

impl Modulation {
    #[must_use]
    pub const fn bits(self) -> usize {
        match self {
            Self::Qpsk => 2,
            Self::Psk8 => 3,
            Self::Apsk16 => 4,
            Self::Apsk32 => 5,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Qpsk => "QPSK",
            Self::Psk8 => "8PSK",
            Self::Apsk16 => "16APSK",
            Self::Apsk32 => "32APSK",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModCod {
    pub index: u8,
    pub modulation: Modulation,
    pub rate: Rate,
}

const CATALOGUE: [(u8, Modulation, Rate); 28] = [
    (1, Modulation::Qpsk, Rate::R1_4),
    (2, Modulation::Qpsk, Rate::R1_3),
    (3, Modulation::Qpsk, Rate::R2_5),
    (4, Modulation::Qpsk, Rate::R1_2),
    (5, Modulation::Qpsk, Rate::R3_5),
    (6, Modulation::Qpsk, Rate::R2_3),
    (7, Modulation::Qpsk, Rate::R3_4),
    (8, Modulation::Qpsk, Rate::R4_5),
    (9, Modulation::Qpsk, Rate::R5_6),
    (10, Modulation::Qpsk, Rate::R8_9),
    (11, Modulation::Qpsk, Rate::R9_10),
    (12, Modulation::Psk8, Rate::R3_5),
    (13, Modulation::Psk8, Rate::R2_3),
    (14, Modulation::Psk8, Rate::R3_4),
    (15, Modulation::Psk8, Rate::R5_6),
    (16, Modulation::Psk8, Rate::R8_9),
    (17, Modulation::Psk8, Rate::R9_10),
    (18, Modulation::Apsk16, Rate::R2_3),
    (19, Modulation::Apsk16, Rate::R3_4),
    (20, Modulation::Apsk16, Rate::R4_5),
    (21, Modulation::Apsk16, Rate::R5_6),
    (22, Modulation::Apsk16, Rate::R8_9),
    (23, Modulation::Apsk16, Rate::R9_10),
    (24, Modulation::Apsk32, Rate::R3_4),
    (25, Modulation::Apsk32, Rate::R4_5),
    (26, Modulation::Apsk32, Rate::R5_6),
    (27, Modulation::Apsk32, Rate::R8_9),
    (28, Modulation::Apsk32, Rate::R9_10),
];

impl ModCod {
    #[must_use]
    pub fn from_index(index: u8) -> Option<Self> {
        CATALOGUE
            .iter()
            .find(|&&(catalogued, ..)| catalogued == index)
            .map(|&(index, modulation, rate)| Self {
                index,
                modulation,
                rate,
            })
    }

    #[cfg(any(test, feature = "test-signals"))]
    #[must_use]
    pub fn find(modulation: Modulation, rate: Rate) -> Option<Self> {
        CATALOGUE
            .iter()
            .find(|&&(_, catalogued, catalogued_rate)| {
                catalogued == modulation && catalogued_rate == rate
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
fn column_order(modulation: Modulation, rate: Rate) -> &'static [usize] {
    match (modulation, rate) {
        (Modulation::Qpsk, _) => &[],
        (Modulation::Psk8, Rate::R3_5) => &[2, 1, 0],
        (Modulation::Psk8, _) => &[0, 1, 2],
        (Modulation::Apsk16, _) => &[0, 1, 2, 3],
        (Modulation::Apsk32, _) => &[0, 1, 2, 3, 4],
    }
}

#[cfg(any(test, feature = "test-signals"))]
#[must_use]
pub fn interleave(coded: &[bool], modulation: Modulation, rate: Rate) -> Vec<bool> {
    let order = column_order(modulation, rate);
    if order.is_empty() {
        return coded.to_vec();
    }
    let rows = coded.len() / order.len();
    let mut out = vec![false; coded.len()];
    for row in 0..rows {
        for (position, &column) in order.iter().enumerate() {
            out[row * order.len() + position] = coded[column * rows + row];
        }
    }
    out
}

#[must_use]
pub fn deinterleave(llrs: &[f32], modulation: Modulation, rate: Rate) -> Vec<f32> {
    let order = column_order(modulation, rate);
    if order.is_empty() {
        return llrs.to_vec();
    }
    let rows = llrs.len() / order.len();
    let mut out = vec![0.0; llrs.len()];
    for row in 0..rows {
        for (position, &column) in order.iter().enumerate() {
            out[column * rows + row] = llrs[row * order.len() + position];
        }
    }
    out
}

const QPSK_PHASES: [f32; 4] = [1.0, 7.0, 3.0, 5.0];
const PSK8_PHASES: [f32; 8] = [1.0, 0.0, 4.0, 5.0, 2.0, 7.0, 3.0, 6.0];

const APSK16_ANGLES: [f32; 16] = [
    3.0, -3.0, 9.0, -9.0, 1.0, -1.0, 11.0, -11.0, 5.0, -5.0, 7.0, -7.0, 3.0, -3.0, 9.0, -9.0,
];
const APSK16_RINGS: [u8; 16] = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0];

const APSK32_ANGLES: [f32; 32] = [
    6.0, 10.0, -6.0, -10.0, 18.0, 14.0, -18.0, -14.0, 3.0, 9.0, -6.0, -12.0, 18.0, 12.0, -21.0,
    -15.0, 2.0, 6.0, -2.0, -6.0, 22.0, 18.0, -22.0, -18.0, 0.0, 6.0, -3.0, -9.0, 21.0, 15.0, 24.0,
    -18.0,
];
const APSK32_RINGS: [u8; 32] = [
    1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 1, 0, 1, 0, 1, 0, 1, 0, 2, 2, 2, 2, 2, 2, 2, 2,
];

fn apsk16_radii(rate: Rate) -> [f32; 2] {
    let gamma = match rate {
        Rate::R2_3 => 3.15,
        Rate::R3_4 => 2.85,
        Rate::R4_5 => 2.75,
        Rate::R5_6 => 2.70,
        Rate::R8_9 => 2.60,
        _ => 2.57,
    };
    let outer = 1.0f32;
    let inner = outer / gamma;
    let scale = (4.0 / (inner * inner + 3.0 * outer * outer)).sqrt();
    [inner * scale, outer * scale]
}

fn apsk32_radii(rate: Rate) -> [f32; 3] {
    let (middle, outer) = match rate {
        Rate::R3_4 => (2.84, 5.27),
        Rate::R4_5 => (2.72, 4.87),
        Rate::R5_6 => (2.64, 4.64),
        Rate::R8_9 => (2.54, 4.33),
        _ => (2.53, 4.30),
    };
    let far = 1.0f32;
    let inner = far / outer;
    let mid = inner * middle;
    let scale = (8.0 / (inner * inner + 3.0 * mid * mid + 4.0 * far * far)).sqrt();
    [inner * scale, mid * scale, far * scale]
}

#[derive(Clone, Copy, Debug)]
pub struct Constellation {
    points: [Complex<f32>; MAX_POINTS],
    bits: usize,
}

impl Constellation {
    #[must_use]
    pub fn new(modulation: Modulation, rate: Rate) -> Self {
        let mut points = [Complex::new(0.0, 0.0); MAX_POINTS];
        let bits = modulation.bits();
        match modulation {
            Modulation::Qpsk => {
                for (label, slot) in points.iter_mut().take(4).enumerate() {
                    *slot =
                        Complex::from_polar(1.0, QPSK_PHASES[label] * std::f32::consts::FRAC_PI_4);
                }
            }
            Modulation::Psk8 => {
                for (label, slot) in points.iter_mut().take(8).enumerate() {
                    *slot =
                        Complex::from_polar(1.0, PSK8_PHASES[label] * std::f32::consts::FRAC_PI_4);
                }
            }
            Modulation::Apsk16 => {
                let radii = apsk16_radii(rate);
                for (label, slot) in points.iter_mut().take(16).enumerate() {
                    *slot = Complex::from_polar(
                        radii[usize::from(APSK16_RINGS[label])],
                        APSK16_ANGLES[label] * std::f32::consts::PI / 12.0,
                    );
                }
            }
            Modulation::Apsk32 => {
                let radii = apsk32_radii(rate);
                for (label, slot) in points.iter_mut().take(32).enumerate() {
                    *slot = Complex::from_polar(
                        radii[usize::from(APSK32_RINGS[label])],
                        APSK32_ANGLES[label] * std::f32::consts::PI / 24.0,
                    );
                }
            }
        }
        Self { points, bits }
    }

    #[must_use]
    pub const fn bits(&self) -> usize {
        self.bits
    }

    #[must_use]
    pub const fn count(&self) -> usize {
        1 << self.bits
    }

    #[must_use]
    pub fn point(&self, label: usize) -> Complex<f32> {
        self.points[label & (self.count() - 1)]
    }
}

#[cfg(any(test, feature = "test-signals"))]
pub fn modulate(bits: &[bool], constellation: &Constellation, out: &mut Vec<Complex<f32>>) {
    let width = constellation.bits();
    for chunk in bits.chunks_exact(width) {
        let label = chunk
            .iter()
            .fold(0usize, |value, &bit| value << 1 | usize::from(bit));
        out.push(constellation.point(label));
    }
}

pub fn demodulate(
    symbols: &[Complex<f32>],
    constellation: &Constellation,
    noise: f32,
    out: &mut Vec<f32>,
) {
    let width = constellation.bits();
    let count = constellation.count();
    let scale = 1.0 / noise.max(1e-6);
    let mut metrics = [f32::NEG_INFINITY; MAX_POINTS];
    for &symbol in symbols {
        for (label, metric) in metrics.iter_mut().take(count).enumerate() {
            *metric = -(symbol - constellation.point(label)).norm_sqr() * scale;
        }
        for bit in 0..width {
            let mut zero = f32::NEG_INFINITY;
            let mut one = f32::NEG_INFINITY;
            for (label, &metric) in metrics.iter().take(count).enumerate() {
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

    const MODULATIONS: [Modulation; 4] = [
        Modulation::Qpsk,
        Modulation::Psk8,
        Modulation::Apsk16,
        Modulation::Apsk32,
    ];

    #[test]
    fn every_catalogued_mode_round_trips_through_its_index() {
        for &(index, modulation, rate) in &CATALOGUE {
            let modcod = ModCod::from_index(index).expect("a catalogued index");
            assert_eq!(modcod.modulation, modulation);
            assert_eq!(modcod.rate, rate);
            assert_eq!(ModCod::find(modulation, rate), Some(modcod));
        }
        assert!(ModCod::from_index(0).is_none());
        assert!(ModCod::from_index(29).is_none());
    }

    #[test]
    fn a_frame_is_a_whole_number_of_slots() {
        for &(index, ..) in &CATALOGUE {
            let modcod = ModCod::from_index(index).expect("a catalogued index");
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
        for &(_, modulation, rate) in &CATALOGUE {
            let coded: Vec<bool> = (0..NORMAL).map(|index| index % 5 == 0).collect();
            let sent = interleave(&coded, modulation, rate);
            let llrs: Vec<f32> = sent
                .iter()
                .map(|&bit| if bit { -1.0 } else { 1.0 })
                .collect();
            let restored = deinterleave(&llrs, modulation, rate);
            let bits: Vec<bool> = restored.iter().map(|&value| value < 0.0).collect();
            assert_eq!(bits, coded, "{modulation:?} {}", rate.label());
        }
    }

    #[test]
    fn only_eight_psk_at_three_fifths_twists_its_columns() {
        for &(_, modulation, rate) in &CATALOGUE {
            let order = column_order(modulation, rate);
            let twisted = order.iter().enumerate().any(|(at, &column)| at != column);
            assert_eq!(
                twisted,
                modulation == Modulation::Psk8 && rate == Rate::R3_5,
                "{modulation:?} {}",
                rate.label()
            );
        }
        let coded: Vec<bool> = (0..SHORT).map(|index| index % 3 == 0).collect();
        let rows = SHORT / 3;
        let sent = interleave(&coded, Modulation::Psk8, Rate::R3_5);
        assert_eq!(sent[0], coded[2 * rows]);
        assert_eq!(sent[1], coded[rows]);
        assert_eq!(sent[2], coded[0]);
    }

    #[test]
    fn every_constellation_carries_unit_average_energy_with_distinct_points() {
        for &(_, modulation, rate) in &CATALOGUE {
            let constellation = Constellation::new(modulation, rate);
            let points: Vec<Complex<f32>> = (0..constellation.count())
                .map(|label| constellation.point(label))
                .collect();
            let energy: f32 = points
                .iter()
                .map(num_complex::Complex::norm_sqr)
                .sum::<f32>()
                / points.len() as f32;
            assert!(
                (energy - 1.0).abs() < 1e-5,
                "{modulation:?} {}: {energy}",
                rate.label()
            );
            for (index, first) in points.iter().enumerate() {
                for second in &points[index + 1..] {
                    assert!(
                        (first - second).norm() > 0.1,
                        "{modulation:?} {} has coincident points",
                        rate.label()
                    );
                }
            }
        }
    }

    fn rings(constellation: &Constellation) -> Vec<(f32, usize)> {
        let mut radii: Vec<f32> = (0..constellation.count())
            .map(|label| constellation.point(label).norm())
            .collect();
        radii.sort_by(f32::total_cmp);
        let mut out: Vec<(f32, usize)> = Vec::new();
        for radius in radii {
            match out.last_mut() {
                Some(ring) if (ring.0 - radius).abs() < 1e-4 => ring.1 += 1,
                _ => out.push((radius, 1)),
            }
        }
        out
    }

    #[test]
    fn the_rings_hold_the_counts_the_standard_names() {
        let sixteen = rings(&Constellation::new(Modulation::Apsk16, Rate::R3_4));
        assert_eq!(
            sixteen.iter().map(|ring| ring.1).collect::<Vec<_>>(),
            [4, 12]
        );
        assert!((sixteen[1].0 / sixteen[0].0 - 2.85).abs() < 1e-4);

        let thirty_two = rings(&Constellation::new(Modulation::Apsk32, Rate::R4_5));
        assert_eq!(
            thirty_two.iter().map(|ring| ring.1).collect::<Vec<_>>(),
            [4, 12, 16]
        );
        assert!((thirty_two[1].0 / thirty_two[0].0 - 2.72).abs() < 1e-4);
        assert!((thirty_two[2].0 / thirty_two[0].0 - 4.87).abs() < 1e-4);
    }

    fn turned_to(point: Complex<f32>, angle: f32) -> bool {
        let apart = (point.arg() - angle).abs();
        apart.min((apart - std::f32::consts::TAU).abs()) < 1e-5
    }

    #[test]
    fn neighbouring_points_differ_in_one_bit() {
        for modulation in [Modulation::Qpsk, Modulation::Psk8] {
            let constellation = Constellation::new(modulation, Rate::R3_4);
            let mut order: Vec<usize> = (0..constellation.count()).collect();
            order.sort_by(|&a, &b| {
                constellation
                    .point(a)
                    .arg()
                    .total_cmp(&constellation.point(b).arg())
            });
            for step in 0..order.len() {
                let (here, next) = (order[step], order[(step + 1) % order.len()]);
                assert_eq!(
                    (here ^ next).count_ones(),
                    1,
                    "{modulation:?}: {here} and {next} are neighbours"
                );
            }
        }
    }

    #[test]
    fn each_label_lands_on_the_point_the_standard_gives_it() {
        let quarter = std::f32::consts::FRAC_PI_4;
        let twelfth = std::f32::consts::PI / 12.0;
        let eighth = std::f32::consts::PI / 8.0;

        let qpsk = Constellation::new(Modulation::Qpsk, Rate::R3_4);
        for (label, turns) in [(0, 1.0), (1, -1.0), (2, 3.0), (3, -3.0)] {
            assert!(
                turned_to(qpsk.point(label), turns * quarter),
                "QPSK label {label}"
            );
        }

        let psk8 = Constellation::new(Modulation::Psk8, Rate::R3_4);
        for (label, turns) in [(0, 1.0), (1, 0.0), (2, 4.0), (3, -3.0), (6, 3.0), (7, -2.0)] {
            assert!(
                turned_to(psk8.point(label), turns * quarter),
                "8PSK label {label}"
            );
        }

        let sixteen = Constellation::new(Modulation::Apsk16, Rate::R3_4);
        let outer = sixteen.point(0).norm();
        let inner = sixteen.point(12).norm();
        assert!(inner < outer);
        for (label, turns, ring) in [
            (0usize, 3.0f32, outer),
            (4, 1.0, outer),
            (7, -11.0, outer),
            (12, 3.0, inner),
            (15, -9.0, inner),
        ] {
            let point = sixteen.point(label);
            assert!(
                turned_to(point, turns * twelfth),
                "16APSK label {label} angle"
            );
            assert!(
                (point.norm() - ring).abs() < 1e-5,
                "16APSK label {label} ring"
            );
        }

        let thirty_two = Constellation::new(Modulation::Apsk32, Rate::R3_4);
        let (near, mid, far) = (
            thirty_two.point(17).norm(),
            thirty_two.point(0).norm(),
            thirty_two.point(8).norm(),
        );
        assert!(near < mid && mid < far);
        for (label, angle, ring) in [
            (0usize, 2.0f32 * eighth, mid),
            (8, eighth, far),
            (16, twelfth, mid),
            (17, 2.0 * eighth, near),
            (24, 0.0, far),
            (30, 8.0 * eighth, far),
            (31, -6.0 * eighth, far),
        ] {
            let point = thirty_two.point(label);
            assert!(turned_to(point, angle), "32APSK label {label} angle");
            assert!(
                (point.norm() - ring).abs() < 1e-5,
                "32APSK label {label} ring"
            );
        }
    }

    #[test]
    fn clean_symbols_demodulate_back_to_their_bits() {
        for modulation in MODULATIONS {
            let rate = Rate::R3_4;
            let constellation = Constellation::new(modulation, rate);
            let bits: Vec<bool> = (0..3 * 8 * modulation.bits())
                .map(|index| index % 3 == 0 || index % 7 == 1)
                .collect();
            let mut symbols = Vec::new();
            modulate(&bits, &constellation, &mut symbols);
            let mut llrs = Vec::new();
            demodulate(&symbols, &constellation, 0.05, &mut llrs);
            let decoded: Vec<bool> = llrs.iter().map(|&value| value < 0.0).collect();
            assert_eq!(decoded, bits, "{modulation:?}");
        }
    }
}
