use sdrmm_dsp::{ERASURE, Soft};

const BLOCK: usize = 128;
const CYCLE: usize = 32;
const TAIL: usize = 24;

const PI: [[u8; CYCLE]; 24] = [
    [
        1, 1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0,
        0, 0,
    ],
    [
        1, 1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0,
        0, 0,
    ],
    [
        1, 1, 0, 0, 1, 0, 0, 0, 1, 1, 0, 0, 1, 0, 0, 0, 1, 1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0,
        0, 0,
    ],
    [
        1, 1, 0, 0, 1, 0, 0, 0, 1, 1, 0, 0, 1, 0, 0, 0, 1, 1, 0, 0, 1, 0, 0, 0, 1, 1, 0, 0, 1, 0,
        0, 0,
    ],
    [
        1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 0, 0, 0, 1, 1, 0, 0, 1, 0, 0, 0, 1, 1, 0, 0, 1, 0,
        0, 0,
    ],
    [
        1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 0, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 0,
        0, 0,
    ],
    [
        1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 0,
        0, 0,
    ],
    [
        1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1,
        0, 0,
    ],
    [
        1, 1, 1, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1,
        0, 0,
    ],
    [
        1, 1, 1, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1,
        0, 0,
    ],
    [
        1, 1, 1, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1,
        0, 0,
    ],
    [
        1, 1, 1, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 1,
        0, 0,
    ],
    [
        1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 1,
        0, 0,
    ],
    [
        1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1,
        0, 0,
    ],
    [
        1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1,
        0, 0,
    ],
    [
        1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1,
        1, 0,
    ],
    [
        1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1,
        1, 0,
    ],
    [
        1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1,
        1, 0,
    ],
    [
        1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1,
        1, 0,
    ],
    [
        1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1,
        1, 0,
    ],
    [
        1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1,
        1, 0,
    ],
    [
        1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        1, 0,
    ],
    [
        1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        1, 0,
    ],
    [
        1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        1, 1,
    ],
];

const PI_TAIL: [u8; TAIL] = [
    1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0,
];

const UEP_PROFILES: [(u16, u8, [u16; 4], [u8; 4]); 64] = [
    (32, 5, [3, 4, 17, 0], [5, 3, 2, 0]),
    (32, 4, [3, 3, 18, 0], [11, 6, 5, 0]),
    (32, 3, [3, 4, 14, 3], [15, 9, 6, 8]),
    (32, 2, [3, 4, 14, 3], [22, 13, 8, 13]),
    (32, 1, [3, 5, 13, 3], [24, 17, 12, 17]),
    (48, 5, [4, 3, 26, 3], [5, 4, 2, 3]),
    (48, 4, [3, 4, 26, 3], [9, 6, 4, 6]),
    (48, 3, [3, 4, 26, 3], [15, 10, 6, 9]),
    (48, 2, [3, 4, 26, 3], [24, 14, 8, 15]),
    (48, 1, [3, 5, 25, 3], [24, 18, 13, 18]),
    (56, 5, [6, 10, 23, 3], [5, 4, 2, 3]),
    (56, 4, [6, 10, 23, 3], [9, 6, 4, 5]),
    (56, 3, [6, 12, 21, 3], [16, 7, 6, 9]),
    (56, 2, [6, 10, 23, 3], [23, 13, 8, 13]),
    (64, 5, [6, 9, 31, 2], [5, 3, 2, 3]),
    (64, 4, [6, 9, 33, 0], [11, 6, 5, 0]),
    (64, 3, [6, 12, 27, 3], [16, 8, 6, 9]),
    (64, 2, [6, 10, 29, 3], [23, 13, 8, 13]),
    (64, 1, [6, 11, 28, 3], [24, 18, 12, 18]),
    (80, 5, [6, 10, 41, 3], [6, 3, 2, 3]),
    (80, 4, [6, 10, 41, 3], [11, 6, 5, 6]),
    (80, 3, [6, 11, 40, 3], [16, 8, 6, 7]),
    (80, 2, [6, 10, 41, 3], [23, 13, 8, 13]),
    (80, 1, [6, 10, 41, 3], [24, 7, 12, 18]),
    (96, 5, [7, 9, 53, 3], [5, 4, 2, 4]),
    (96, 4, [7, 10, 52, 3], [9, 6, 4, 6]),
    (96, 3, [6, 12, 51, 3], [16, 9, 6, 10]),
    (96, 2, [6, 10, 53, 3], [22, 12, 9, 12]),
    (96, 1, [6, 13, 50, 3], [24, 18, 13, 19]),
    (112, 5, [14, 17, 50, 3], [5, 4, 2, 5]),
    (112, 4, [11, 21, 49, 3], [9, 6, 4, 8]),
    (112, 3, [11, 23, 47, 3], [16, 8, 6, 9]),
    (112, 2, [11, 21, 49, 3], [23, 12, 9, 14]),
    (128, 5, [12, 19, 62, 3], [5, 3, 2, 4]),
    (128, 4, [11, 21, 61, 3], [11, 6, 5, 7]),
    (128, 3, [11, 22, 60, 3], [16, 9, 6, 10]),
    (128, 2, [11, 21, 61, 3], [22, 12, 9, 14]),
    (128, 1, [11, 20, 62, 3], [24, 17, 13, 19]),
    (160, 5, [11, 19, 87, 3], [5, 4, 2, 4]),
    (160, 4, [11, 23, 83, 3], [11, 6, 5, 9]),
    (160, 3, [11, 24, 82, 3], [16, 8, 6, 11]),
    (160, 2, [11, 21, 85, 3], [22, 11, 9, 13]),
    (160, 1, [11, 22, 84, 3], [24, 18, 12, 19]),
    (192, 5, [11, 20, 110, 3], [6, 4, 2, 5]),
    (192, 4, [11, 22, 108, 3], [10, 6, 4, 9]),
    (192, 3, [11, 24, 106, 3], [16, 10, 6, 11]),
    (192, 2, [11, 20, 110, 3], [22, 13, 9, 13]),
    (192, 1, [11, 21, 109, 3], [24, 20, 13, 24]),
    (224, 5, [12, 22, 131, 3], [8, 6, 2, 6]),
    (224, 4, [12, 26, 127, 3], [12, 8, 4, 11]),
    (224, 3, [11, 20, 134, 3], [16, 10, 7, 9]),
    (224, 2, [11, 22, 132, 3], [24, 16, 10, 15]),
    (224, 1, [11, 24, 130, 3], [24, 20, 12, 20]),
    (256, 5, [11, 24, 154, 3], [6, 5, 2, 5]),
    (256, 4, [11, 24, 154, 3], [12, 9, 5, 10]),
    (256, 3, [11, 27, 151, 3], [16, 10, 7, 10]),
    (256, 2, [11, 22, 156, 3], [24, 14, 10, 13]),
    (256, 1, [11, 26, 152, 3], [24, 19, 14, 18]),
    (320, 5, [11, 26, 200, 3], [8, 5, 2, 6]),
    (320, 4, [11, 25, 201, 3], [13, 9, 5, 10]),
    (320, 2, [11, 26, 200, 3], [24, 17, 9, 17]),
    (384, 5, [11, 27, 247, 3], [8, 6, 2, 7]),
    (384, 3, [11, 24, 250, 3], [16, 9, 7, 10]),
    (384, 1, [12, 28, 245, 3], [24, 20, 14, 23]),
];

const UEP_TABLE: [(u16, u8, u16); 64] = [
    // (subchannel size in capacity units, protection level, bit rate in kbit/s)
    (16, 5, 32),
    (21, 4, 32),
    (24, 3, 32),
    (29, 2, 32),
    (35, 1, 32),
    (24, 5, 48),
    (29, 4, 48),
    (35, 3, 48),
    (42, 2, 48),
    (52, 1, 48),
    (29, 5, 56),
    (35, 4, 56),
    (42, 3, 56),
    (52, 2, 56),
    (32, 5, 64),
    (42, 4, 64),
    (48, 3, 64),
    (58, 2, 64),
    (70, 1, 64),
    (40, 5, 80),
    (52, 4, 80),
    (58, 3, 80),
    (70, 2, 80),
    (84, 1, 80),
    (48, 5, 96),
    (58, 4, 96),
    (70, 3, 96),
    (84, 2, 96),
    (104, 1, 96),
    (58, 5, 112),
    (70, 4, 112),
    (84, 3, 112),
    (104, 2, 112),
    (64, 5, 128),
    (84, 4, 128),
    (96, 3, 128),
    (116, 2, 128),
    (140, 1, 128),
    (80, 5, 160),
    (104, 4, 160),
    (116, 3, 160),
    (140, 2, 160),
    (168, 1, 160),
    (96, 5, 192),
    (116, 4, 192),
    (140, 3, 192),
    (168, 2, 192),
    (208, 1, 192),
    (116, 5, 224),
    (140, 4, 224),
    (168, 3, 224),
    (208, 2, 224),
    (232, 1, 224),
    (128, 5, 256),
    (168, 4, 256),
    (192, 3, 256),
    (232, 2, 256),
    (280, 1, 256),
    (160, 5, 320),
    (208, 4, 320),
    (280, 2, 320),
    (192, 5, 384),
    (280, 3, 384),
    (416, 1, 384),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eep {
    A,
    B,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Protection {
    segments: Vec<(usize, usize)>,
    frame_bits: usize,
    coded_bits: usize,
}

fn eep_a(bitrate: u16, level: u8) -> Option<(usize, usize, usize, usize)> {
    let rate = usize::from(bitrate);
    match level {
        1 => Some((6 * rate / 8 - 3, 24, 3, 23)),
        2 if bitrate == 8 => Some((5, 13, 1, 12)),
        2 => Some((2 * rate / 8 - 3, 14, 4 * rate / 8 + 3, 13)),
        3 => Some((6 * rate / 8 - 3, 8, 3, 7)),
        4 => Some((4 * rate / 8 - 3, 3, 2 * rate / 8 + 3, 2)),
        _ => None,
    }
}

fn eep_b(bitrate: u16, level: u8) -> Option<(usize, usize, usize, usize)> {
    let blocks = 24 * usize::from(bitrate) / 32;
    let first = blocks.checked_sub(3)?;
    match level {
        1 => Some((first, 10, 3, 9)),
        2 => Some((first, 6, 3, 5)),
        3 => Some((first, 4, 3, 3)),
        4 => Some((first, 2, 3, 1)),
        _ => None,
    }
}

impl Protection {
    fn build(frame_bits: usize, segments: Vec<(usize, usize)>) -> Option<Self> {
        if segments.iter().map(|&(count, _)| count).sum::<usize>() * CYCLE != frame_bits {
            return None;
        }
        let coded_bits = segments
            .iter()
            .map(|&(count, index)| count * BLOCK / CYCLE * kept(index))
            .sum::<usize>()
            + PI_TAIL.iter().filter(|&&keep| keep == 1).count();
        Some(Self {
            segments,
            frame_bits,
            coded_bits,
        })
    }

    #[must_use]
    pub fn eep(bitrate_kbps: u16, profile: Eep, level: u8) -> Option<Self> {
        let (l1, pi1, l2, pi2) = match profile {
            Eep::A => eep_a(bitrate_kbps, level)?,
            Eep::B => eep_b(bitrate_kbps, level)?,
        };
        Self::build(24 * usize::from(bitrate_kbps), vec![(l1, pi1), (l2, pi2)])
    }

    #[must_use]
    pub fn uep(table_index: u8) -> Option<Self> {
        let &(_, level, bitrate) = UEP_TABLE.get(usize::from(table_index))?;
        let &(_, _, lengths, indices) = UEP_PROFILES
            .iter()
            .find(|&&(rate, protection, ..)| rate == bitrate && protection == level)?;
        let segments = lengths
            .iter()
            .zip(indices)
            .filter(|&(&count, index)| count > 0 && index > 0)
            .map(|(&count, index)| (usize::from(count), usize::from(index)))
            .collect();
        Self::build(24 * usize::from(bitrate), segments)
    }

    #[must_use]
    pub fn fic() -> Self {
        Self {
            segments: vec![(21, 16), (3, 15)],
            frame_bits: 768,
            coded_bits: 2_304,
        }
    }

    #[must_use]
    pub const fn frame_bits(&self) -> usize {
        self.frame_bits
    }

    #[must_use]
    pub const fn coded_bits(&self) -> usize {
        self.coded_bits
    }

    fn mask(&self) -> impl Iterator<Item = bool> + '_ {
        self.segments
            .iter()
            .flat_map(|&(count, index)| {
                (0..count * BLOCK).map(move |position| PI[index - 1][position % CYCLE] == 1)
            })
            .chain(PI_TAIL.iter().map(|&keep| keep == 1))
    }

    pub fn depuncture(&self, received: &[Soft], out: &mut Vec<Soft>) {
        let mut source = received.iter();
        for keep in self.mask() {
            out.push(match keep.then(|| source.next()) {
                Some(Some(&value)) => value,
                _ => ERASURE,
            });
        }
    }

    pub fn puncture(&self, coded: &[bool], out: &mut Vec<bool>) {
        for (position, keep) in self.mask().enumerate() {
            if keep && let Some(&bit) = coded.get(position) {
                out.push(bit);
            }
        }
    }
}

fn kept(index: usize) -> usize {
    PI[index - 1].iter().filter(|&&keep| keep == 1).count()
}

#[must_use]
pub fn uep_bitrate_kbps(table_index: u8) -> Option<u16> {
    UEP_TABLE
        .get(usize::from(table_index))
        .map(|&(_, _, bitrate)| bitrate)
}

#[must_use]
pub fn uep_size_cu(table_index: u8) -> Option<u16> {
    UEP_TABLE
        .get(usize::from(table_index))
        .map(|&(size, ..)| size)
}

#[must_use]
pub fn eep_bitrate_kbps(size_cu: u16, profile: Eep, level: u8) -> Option<u16> {
    let size = u32::from(size_cu);
    let bitrate = match (profile, level) {
        (Eep::A, 1) => size / 12 * 8,
        (Eep::A, 2) => size / 8 * 8,
        (Eep::A, 3) => size / 6 * 8,
        (Eep::A, 4) => size / 4 * 8,
        (Eep::B, 1) => size / 27 * 32,
        (Eep::B, 2) => size / 21 * 32,
        (Eep::B, 3) => size / 18 * 32,
        (Eep::B, 4) => size / 15 * 32,
        _ => return None,
    };
    (bitrate > 0).then_some(bitrate as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_puncturing_vectors_grow_monotonically() {
        for index in 1..24 {
            let previous = kept(index);
            let current = kept(index + 1);
            assert!(
                current > previous,
                "PI_{} is not denser than PI_{index}",
                index + 1
            );
            for (position, &keep) in PI[index - 1].iter().enumerate() {
                if keep == 1 {
                    assert_eq!(PI[index][position], 1, "PI_{} lost a bit", index + 1);
                }
            }
        }
        assert_eq!(kept(24), CYCLE);
        assert_eq!(kept(1), 9);
    }

    #[test]
    fn the_fic_profile_matches_the_documented_block_counts() {
        let fic = Protection::fic();
        assert_eq!(fic.frame_bits(), 768);
        assert_eq!(fic.coded_bits(), 2_304);
        assert_eq!(fic.mask().count(), 4 * 768 + TAIL);
        assert_eq!(fic.mask().filter(|&keep| keep).count(), 2_304);
    }

    #[test]
    fn every_equal_error_profile_is_consistent() {
        for bitrate in [8u16, 16, 32, 64, 96, 128, 192, 256, 384] {
            for level in 1..=4u8 {
                let Some(protection) = Protection::eep(bitrate, Eep::A, level) else {
                    continue;
                };
                assert_eq!(protection.frame_bits(), 24 * usize::from(bitrate));
                assert_eq!(
                    protection.mask().count(),
                    4 * protection.frame_bits() + TAIL
                );
                assert_eq!(
                    protection.mask().filter(|&keep| keep).count(),
                    protection.coded_bits()
                );
            }
        }
    }

    #[test]
    fn the_equal_error_size_maps_back_to_its_bit_rate() {
        for (size, profile, level, expected) in [
            (96u16, Eep::A, 1u8, 64u16),
            (64, Eep::A, 2, 64),
            (48, Eep::A, 3, 64),
            (32, Eep::A, 4, 64),
            (54, Eep::B, 1, 64),
            (42, Eep::B, 2, 64),
            (36, Eep::B, 3, 64),
            (30, Eep::B, 4, 64),
        ] {
            assert_eq!(eep_bitrate_kbps(size, profile, level), Some(expected));
        }
    }

    #[test]
    fn the_unequal_error_table_agrees_with_its_capacity_units() {
        assert_eq!(uep_bitrate_kbps(0), Some(32));
        assert_eq!(uep_size_cu(0), Some(16));
        assert_eq!(uep_bitrate_kbps(63), Some(384));
        assert_eq!(uep_size_cu(63), Some(416));
    }

    #[test]
    fn every_unequal_error_table_entry_builds_a_profile() {
        for index in 0..64u8 {
            let protection =
                Protection::uep(index).unwrap_or_else(|| panic!("UEP index {index} is missing"));
            assert_eq!(
                protection.frame_bits(),
                24 * usize::from(uep_bitrate_kbps(index).unwrap_or(0))
            );
            assert_eq!(
                protection.mask().filter(|&keep| keep).count(),
                protection.coded_bits()
            );
        }
    }

    #[test]
    fn depuncturing_restores_the_mother_length_and_marks_the_gaps() {
        let protection = Protection::eep(64, Eep::A, 3).expect("EEP-A 3 at 64 kbps");
        let coded: Vec<bool> = (0..4 * protection.frame_bits() + TAIL)
            .map(|index| index % 3 == 0)
            .collect();
        let mut sent = Vec::new();
        protection.puncture(&coded, &mut sent);
        assert_eq!(sent.len(), protection.coded_bits());
        let softs: Vec<Soft> = sent.iter().map(|&bit| sdrmm_dsp::soft(bit)).collect();
        let mut mother = Vec::new();
        protection.depuncture(&softs, &mut mother);
        assert_eq!(mother.len(), coded.len());
        let mut source = sent.iter();
        for (position, keep) in protection.mask().enumerate() {
            if keep {
                assert_eq!(mother[position] > 0, *source.next().expect("a sent bit"));
            } else {
                assert_eq!(mother[position], ERASURE);
            }
        }
    }
}
