#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Robustness {
    A,
    B,
    C,
    D,
    E,
}

pub const DRM30_RATE_HZ: f64 = 48_000.0;
pub const DRM_PLUS_RATE_HZ: f64 = 192_000.0;

const FREQUENCY_A: [(i32, u16); 3] = [(18, 205), (54, 836), (72, 215)];
const FREQUENCY_B: [(i32, u16); 3] = [(16, 331), (48, 651), (64, 555)];
const FREQUENCY_C: [(i32, u16); 3] = [(11, 214), (33, 392), (44, 242)];
const FREQUENCY_D: [(i32, u16); 3] = [(7, 788), (21, 1014), (28, 332)];

const TIME_A: [(i32, u16); 21] = [
    (17, 973),
    (18, 205),
    (19, 717),
    (21, 264),
    (28, 357),
    (29, 357),
    (32, 952),
    (33, 440),
    (39, 856),
    (40, 88),
    (41, 88),
    (53, 68),
    (54, 836),
    (55, 836),
    (56, 836),
    (60, 1008),
    (61, 1008),
    (63, 752),
    (71, 215),
    (72, 215),
    (73, 727),
];
const TIME_B: [(i32, u16); 19] = [
    (14, 304),
    (16, 331),
    (18, 108),
    (20, 620),
    (24, 192),
    (26, 704),
    (32, 44),
    (36, 432),
    (42, 588),
    (44, 844),
    (48, 651),
    (49, 651),
    (50, 651),
    (54, 460),
    (56, 460),
    (62, 944),
    (64, 555),
    (66, 940),
    (68, 428),
];
const TIME_C: [(i32, u16); 19] = [
    (8, 722),
    (10, 466),
    (11, 214),
    (12, 214),
    (14, 479),
    (16, 516),
    (18, 260),
    (22, 577),
    (24, 662),
    (28, 3),
    (30, 771),
    (32, 392),
    (33, 392),
    (36, 37),
    (38, 37),
    (42, 474),
    (44, 242),
    (45, 242),
    (46, 754),
];
const TIME_D: [(i32, u16); 21] = [
    (5, 636),
    (6, 124),
    (7, 788),
    (8, 788),
    (9, 200),
    (11, 688),
    (12, 152),
    (14, 920),
    (15, 920),
    (17, 644),
    (18, 388),
    (20, 652),
    (21, 1014),
    (23, 176),
    (24, 176),
    (26, 752),
    (27, 496),
    (28, 332),
    (29, 432),
    (30, 964),
    (32, 452),
];
const TIME_E: [(i32, u16); 21] = [
    (-80, 219),
    (-79, 475),
    (-77, 987),
    (-53, 652),
    (-52, 652),
    (-51, 140),
    (-32, 819),
    (-31, 819),
    (12, 907),
    (13, 907),
    (14, 651),
    (21, 903),
    (22, 391),
    (23, 903),
    (40, 203),
    (41, 203),
    (42, 203),
    (67, 797),
    (68, 29),
    (79, 508),
    (80, 508),
];

const AFS_E: [(u16, u16); 54] = [
    (134, 115),
    (866, 135),
    (588, 194),
    (325, 293),
    (77, 431),
    (868, 608),
    (649, 825),
    (445, 57),
    (256, 353),
    (82, 688),
    (946, 38),
    (801, 452),
    (671, 905),
    (556, 373),
    (455, 905),
    (369, 452),
    (298, 39),
    (242, 689),
    (200, 354),
    (173, 59),
    (161, 827),
    (164, 610),
    (181, 433),
    (213, 295),
    (260, 197),
    (322, 138),
    (398, 118),
    (489, 138),
    (595, 197),
    (716, 295),
    (851, 433),
    (1001, 610),
    (142, 827),
    (322, 59),
    (516, 354),
    (725, 689),
    (949, 39),
    (164, 452),
    (417, 905),
    (685, 373),
    (968, 905),
    (242, 452),
    (554, 38),
    (881, 688),
    (199, 353),
    (556, 57),
    (927, 825),
    (289, 608),
    (690, 431),
    (82, 293),
    (512, 194),
    (957, 135),
    (393, 115),
    (868, 134),
];

const W_A: [[u16; 3]; 5] = [
    [228, 341, 455],
    [455, 569, 683],
    [683, 796, 910],
    [910, 0, 114],
    [114, 228, 341],
];
const Z_A: [[u16; 3]; 5] = [
    [0, 81, 248],
    [18, 106, 106],
    [122, 116, 31],
    [129, 129, 39],
    [33, 32, 111],
];
const W_B: [[u16; 5]; 3] = [
    [512, 0, 512, 0, 512],
    [0, 512, 0, 512, 0],
    [512, 0, 512, 0, 512],
];
const Z_B: [[u16; 5]; 3] = [
    [0, 57, 164, 64, 12],
    [168, 255, 161, 106, 118],
    [25, 232, 132, 233, 38],
];
const W_C: [[u16; 10]; 2] = [
    [465, 372, 279, 186, 93, 0, 931, 838, 745, 652],
    [931, 838, 745, 652, 559, 465, 372, 279, 186, 93],
];
const Z_C: [[u16; 10]; 2] = [
    [0, 76, 29, 76, 9, 190, 161, 248, 33, 108],
    [179, 178, 83, 253, 127, 105, 101, 198, 250, 145],
];
const W_D: [[u16; 8]; 3] = [
    [366, 439, 512, 585, 658, 731, 805, 878],
    [731, 805, 878, 951, 0, 73, 146, 219],
    [73, 146, 219, 293, 366, 439, 512, 585],
];
const Z_D: [[u16; 8]; 3] = [
    [0, 240, 17, 60, 220, 38, 151, 101],
    [110, 7, 78, 82, 175, 150, 106, 25],
    [165, 7, 252, 124, 253, 177, 197, 142],
];
const R_E: [[u16; 10]; 4] = [
    [39, 118, 197, 276, 354, 433, 39, 118, 197, 276],
    [37, 183, 402, 37, 183, 402, 37, 183, 402, 37],
    [110, 329, 475, 110, 329, 475, 110, 329, 475, 110],
    [79, 158, 236, 315, 394, 473, 79, 158, 236, 315],
];
const Z_E: [[u16; 10]; 4] = [
    [473, 394, 315, 236, 158, 79, 0, 0, 0, 0],
    [183, 914, 402, 37, 475, 841, 768, 768, 987, 183],
    [549, 622, 475, 110, 37, 622, 256, 768, 329, 549],
    [79, 158, 236, 315, 394, 473, 158, 315, 473, 630],
];
const Q_E: [[u16; 10]; 4] = [
    [329, 489, 894, 419, 607, 519, 1020, 942, 817, 939],
    [824, 1023, 74, 319, 225, 207, 348, 422, 395, 92],
    [959, 379, 7, 738, 500, 920, 440, 727, 263, 733],
    [907, 946, 924, 91, 189, 133, 910, 804, 1022, 433],
];

const FAC_A: [&[i32]; 15] = [
    &[],
    &[],
    &[26, 46, 66, 86],
    &[10, 30, 50, 70, 90],
    &[14, 22, 34, 62, 74, 94],
    &[26, 38, 58, 66, 78],
    &[22, 30, 42, 62, 70, 82],
    &[26, 34, 46, 66, 74, 86],
    &[10, 30, 38, 50, 58, 70, 78, 90],
    &[14, 22, 34, 42, 62, 74, 82, 94],
    &[26, 38, 46, 66, 86],
    &[10, 30, 50, 70, 90],
    &[14, 34, 74, 94],
    &[38, 58, 78],
    &[],
];
const FAC_B: [&[i32]; 15] = [
    &[],
    &[],
    &[13, 25, 43, 55, 67],
    &[15, 27, 45, 57, 69],
    &[17, 29, 47, 59, 71],
    &[19, 31, 49, 61, 73],
    &[9, 21, 33, 51, 63, 75],
    &[11, 23, 35, 53, 65, 77],
    &[13, 25, 37, 55, 67, 79],
    &[15, 27, 39, 57, 69, 81],
    &[17, 29, 41, 59, 71, 83],
    &[19, 31, 43, 61, 73],
    &[21, 33, 45, 63, 75],
    &[23, 35, 47, 65, 77],
    &[],
];
const FAC_C: [&[i32]; 20] = [
    &[],
    &[],
    &[],
    &[9, 21, 45, 57],
    &[23, 35, 47],
    &[13, 25, 37, 49],
    &[15, 27, 39, 51],
    &[5, 17, 29, 41, 53],
    &[7, 19, 31, 43, 55],
    &[9, 21, 45, 57],
    &[23, 35, 47],
    &[13, 25, 37, 49],
    &[15, 27, 39, 51],
    &[5, 17, 29, 41, 53],
    &[7, 19, 31, 43, 55],
    &[9, 21, 45, 57],
    &[23, 35, 47],
    &[13, 25, 37, 49],
    &[15, 27, 39, 51],
    &[],
];
const FAC_D: [&[i32]; 24] = [
    &[],
    &[],
    &[],
    &[9, 18, 27],
    &[10, 19],
    &[11, 20, 29],
    &[12, 30],
    &[13, 22, 31],
    &[5, 14, 23, 32],
    &[6, 15, 24, 33],
    &[16, 25, 34],
    &[8, 17, 26, 35],
    &[9, 18, 27, 36],
    &[10, 19, 37],
    &[11, 20, 29],
    &[12, 30],
    &[13, 22, 31],
    &[5, 14, 23, 32],
    &[6, 15, 24, 33],
    &[16, 25, 34],
    &[8, 17, 26, 35],
    &[9, 18, 27, 36],
    &[10, 19, 37],
    &[],
];
const FAC_E_1: [i32; 11] = [-78, -62, -46, -30, -14, 2, 18, 34, 50, 66, 82];
const FAC_E_2: [i32; 12] = [-90, -74, -58, -42, -26, -10, 6, 22, 38, 54, 70, 86];
const FAC_E_3: [i32; 12] = [-86, -70, -54, -38, -22, -6, 10, 26, 42, 58, 74, 90];
const FAC_E_4: [i32; 11] = [-82, -66, -50, -34, -18, -2, 14, 30, 46, 62, 78];
const FAC_E_LAST: [i32; 3] = [-90, -74, -58];

impl Robustness {
    pub const ALL: [Self; 5] = [Self::A, Self::B, Self::C, Self::D, Self::E];

    #[must_use]
    pub const fn useful(self) -> usize {
        match self {
            Self::A => 1152,
            Self::B => 1024,
            Self::C => 704,
            Self::D => 448,
            Self::E => 432,
        }
    }

    #[must_use]
    pub const fn guard(self) -> usize {
        match self {
            Self::A => 128,
            Self::B | Self::C => 256,
            Self::D => 352,
            Self::E => 48,
        }
    }

    #[must_use]
    pub const fn symbol(self) -> usize {
        self.useful() + self.guard()
    }

    #[must_use]
    pub const fn symbols(self) -> usize {
        match self {
            Self::A | Self::B => 15,
            Self::C => 20,
            Self::D => 24,
            Self::E => 40,
        }
    }

    #[must_use]
    pub const fn frames(self) -> usize {
        match self {
            Self::E => 4,
            _ => 3,
        }
    }

    #[must_use]
    pub const fn plus(self) -> bool {
        matches!(self, Self::E)
    }

    #[must_use]
    pub const fn rate_hz(self) -> f64 {
        if self.plus() {
            DRM_PLUS_RATE_HZ
        } else {
            DRM30_RATE_HZ
        }
    }

    #[must_use]
    pub fn spacing_hz(self) -> f64 {
        self.rate_hz() / self.useful() as f64
    }

    #[must_use]
    pub const fn sdc_symbols(self) -> usize {
        match self {
            Self::A | Self::B => 2,
            Self::C | Self::D => 3,
            Self::E => 5,
        }
    }

    #[must_use]
    pub const fn fac_bits(self) -> usize {
        if self.plus() { 116 } else { 72 }
    }

    #[must_use]
    pub const fn fac_cells(self) -> usize {
        if self.plus() { 244 } else { 65 }
    }

    #[must_use]
    pub const fn interleave_depth(self) -> usize {
        if self.plus() { 6 } else { 5 }
    }

    #[must_use]
    pub const fn label(self) -> char {
        match self {
            Self::A => 'A',
            Self::B => 'B',
            Self::C => 'C',
            Self::D => 'D',
            Self::E => 'E',
        }
    }

    #[must_use]
    pub const fn carriers(self, occupancy: u8) -> Option<(i32, i32)> {
        match (self, occupancy) {
            (Self::A, 0) => Some((2, 102)),
            (Self::A, 1) => Some((2, 114)),
            (Self::A, 2) => Some((-102, 102)),
            (Self::A, 3) => Some((-114, 114)),
            (Self::A, 4) => Some((-98, 314)),
            (Self::A, 5) => Some((-110, 350)),
            (Self::B, 0) => Some((1, 91)),
            (Self::B, 1) => Some((1, 103)),
            (Self::B, 2) => Some((-91, 91)),
            (Self::B, 3) => Some((-103, 103)),
            (Self::B, 4) => Some((-87, 279)),
            (Self::B, 5) => Some((-99, 311)),
            (Self::C, 3) => Some((-69, 69)),
            (Self::C, 5) => Some((-67, 213)),
            (Self::D, 3) => Some((-44, 44)),
            (Self::D, 5) => Some((-43, 135)),
            (Self::E, 0) => Some((-106, 106)),
            _ => None,
        }
    }

    #[must_use]
    pub const fn span(self) -> (i32, i32) {
        match self {
            Self::A => (-114, 350),
            Self::B => (-103, 311),
            Self::C => (-69, 213),
            Self::D => (-44, 135),
            Self::E => (-106, 106),
        }
    }

    #[must_use]
    pub const fn widest(self) -> u8 {
        if self.plus() { 0 } else { 5 }
    }

    #[must_use]
    pub const fn unused(self, k: i32) -> bool {
        match self {
            Self::A => k >= -1 && k <= 1,
            Self::B | Self::C | Self::D => k == 0,
            Self::E => false,
        }
    }

    #[must_use]
    pub const fn boosted(self, occupancy: u8) -> [i32; 4] {
        match (self, occupancy) {
            (Self::A, 0) => [2, 6, 98, 102],
            (Self::A, 1) => [2, 6, 110, 114],
            (Self::A, 2) => [-102, -98, 98, 102],
            (Self::A, 3) => [-114, -110, 110, 114],
            (Self::A, 4) => [-98, -94, 310, 314],
            (Self::A, _) => [-110, -106, 346, 350],
            (Self::B, 0) => [1, 3, 89, 91],
            (Self::B, 1) => [1, 3, 101, 103],
            (Self::B, 2) => [-91, -89, 89, 91],
            (Self::B, 3) => [-103, -101, 101, 103],
            (Self::B, 4) => [-87, -85, 277, 279],
            (Self::B, _) => [-99, -97, 309, 311],
            (Self::C, 3) => [-69, -67, 67, 69],
            (Self::C, _) => [-67, -65, 211, 213],
            (Self::D, 3) => [-44, -43, 43, 44],
            (Self::D, _) => [-43, -42, 134, 135],
            (Self::E, _) => [-106, -102, 102, 106],
        }
    }

    #[must_use]
    pub const fn frequency_refs(self) -> &'static [(i32, u16)] {
        match self {
            Self::A => &FREQUENCY_A,
            Self::B => &FREQUENCY_B,
            Self::C => &FREQUENCY_C,
            Self::D => &FREQUENCY_D,
            Self::E => &[],
        }
    }

    #[must_use]
    pub const fn time_refs(self) -> &'static [(i32, u16)] {
        match self {
            Self::A => &TIME_A,
            Self::B => &TIME_B,
            Self::C => &TIME_C,
            Self::D => &TIME_D,
            Self::E => &TIME_E,
        }
    }

    #[must_use]
    pub const fn gain_grid(self) -> (i32, i32, i32) {
        match self {
            Self::A => (4, 5, 2),
            Self::B => (2, 3, 1),
            Self::C => (2, 2, 1),
            Self::D => (1, 3, 1),
            Self::E => (4, 4, 2),
        }
    }

    #[must_use]
    pub const fn is_gain_ref(self, symbol: usize, k: i32) -> bool {
        let (x, y, k0) = self.gain_grid();
        (k - k0 - x * (symbol as i32 % y)).rem_euclid(x * y) == 0
    }

    #[must_use]
    pub fn gain_phase(self, symbol: usize, k: i32) -> u16 {
        let (x, y, k0) = self.gain_grid();
        let s = symbol as i64;
        let n = (s % i64::from(y)) as usize;
        let m = (s / i64::from(y)) as usize;
        let p = i64::from((k - k0 - x * n as i32) / (x * y));
        let phase = match self {
            Self::A => drm30_phase(Z_A[n][m], W_A[n][m], 36, p, s),
            Self::B => drm30_phase(Z_B[n][m], W_B[n][m], 12, p, s),
            Self::C => drm30_phase(Z_C[n][m], W_C[n][m], 12, p, s),
            Self::D => drm30_phase(Z_D[n][m], W_D[n][m], 14, p, s),
            Self::E => {
                p * p * i64::from(R_E[n][m]) + p * i64::from(Z_E[n][m]) + i64::from(Q_E[n][m])
            }
        };
        phase.rem_euclid(1024) as u16
    }

    #[must_use]
    pub fn frequency_phase(self, symbol: usize, k: i32) -> Option<u16> {
        let &(_, phase) = self
            .frequency_refs()
            .iter()
            .find(|&&(carrier, _)| carrier == k)?;
        let flip = self == Self::D && k != 28 && symbol % 2 == 1;
        Some(if flip { (phase + 512) % 1024 } else { phase })
    }

    #[must_use]
    pub fn time_phase(self, k: i32) -> Option<u16> {
        self.time_refs()
            .iter()
            .find(|&&(carrier, _)| carrier == k)
            .map(|&(_, phase)| phase)
    }

    #[must_use]
    pub fn afs_phase(self, frame: usize, symbol: usize, k: i32) -> Option<u16> {
        if self != Self::E || (k + 106) % 4 != 0 || !(-106..=106).contains(&k) {
            return None;
        }
        let index = ((k + 106) / 4) as usize;
        match (frame, symbol) {
            (0, 4) => Some(AFS_E[index].0),
            (3, 39) => Some(AFS_E[index].1),
            _ => None,
        }
    }

    #[must_use]
    pub fn fac_carriers(self, symbol: usize) -> &'static [i32] {
        match self {
            Self::A => FAC_A.get(symbol).copied().unwrap_or(&[]),
            Self::B => FAC_B.get(symbol).copied().unwrap_or(&[]),
            Self::C => FAC_C.get(symbol).copied().unwrap_or(&[]),
            Self::D => FAC_D.get(symbol).copied().unwrap_or(&[]),
            Self::E => match symbol {
                5..=25 => match (symbol - 5) % 4 {
                    0 => &FAC_E_1,
                    1 => &FAC_E_2,
                    2 => &FAC_E_3,
                    _ => &FAC_E_4,
                },
                26 => &FAC_E_LAST,
                _ => &[],
            },
        }
    }
}

fn drm30_phase(z: u16, w: u16, q: i64, p: i64, s: i64) -> i64 {
    4 * i64::from(z) + p * i64::from(w) + p * p * (1 + s) * q
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fac_tables_hold_the_specified_cell_counts() {
        for mode in Robustness::ALL {
            let total: usize = (0..mode.symbols())
                .map(|symbol| mode.fac_carriers(symbol).len())
                .sum();
            assert_eq!(total, mode.fac_cells(), "{mode:?}");
        }
    }

    #[test]
    fn frequency_references_agree_with_time_references() {
        for mode in Robustness::ALL {
            for &(k, phase) in mode.frequency_refs() {
                assert_eq!(mode.time_phase(k), Some(phase), "{mode:?} {k}");
            }
        }
    }

    #[test]
    fn afs_cells_on_the_gain_grid_share_its_phase() {
        let mode = Robustness::E;
        for (frame, symbol) in [(0, 4), (3, 39)] {
            for k in (-106..=106).step_by(4) {
                if mode.is_gain_ref(symbol, k) {
                    assert_eq!(
                        mode.afs_phase(frame, symbol, k),
                        Some(mode.gain_phase(symbol, k)),
                        "{frame}/{symbol}/{k}"
                    );
                }
            }
        }
    }

    #[test]
    fn edge_carriers_are_gain_references() {
        for mode in Robustness::ALL {
            for occupancy in 0..6 {
                let Some((low, high)) = mode.carriers(occupancy) else {
                    continue;
                };
                let (_, y, _) = mode.gain_grid();
                for edge in [low, high] {
                    assert!(
                        (0..y as usize).any(|symbol| mode.is_gain_ref(symbol, edge)),
                        "{mode:?} {occupancy} {edge}"
                    );
                }
                for k in mode.boosted(occupancy) {
                    assert!((low..=high).contains(&k));
                }
            }
        }
    }
}
