pub(crate) const DEFAULT_IF_HZ: u32 = 3_570_000;
const WIDE_IF_HZ: u32 = 4_570_000;
const NARROW_IF_START_HZ: u32 = 2_300_000;
const LOW_PASS_HZ: [u32; 10] = [
    1_700_000, 1_600_000, 1_550_000, 1_450_000, 1_200_000, 900_000, 700_000, 550_000, 450_000,
    350_000,
];
const HIGH_PASS_1_HZ: u32 = 350_000;
const HIGH_PASS_2_HZ: u32 = 380_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Filter {
    pub(crate) if_hz: u32,
    pub(crate) reg0a: u8,
    pub(crate) reg0b: u8,
}

pub(crate) fn filter(bandwidth_hz: u32) -> Filter {
    match bandwidth_hz {
        7_000_001.. => wide(0x0b),
        6_000_001..=7_000_000 => wide(0x2a),
        2_430_001..=6_000_000 => Filter {
            if_hz: DEFAULT_IF_HZ,
            reg0a: 0x10,
            reg0b: 0x6b,
        },
        _ => narrow(bandwidth_hz),
    }
}

const fn wide(reg0b: u8) -> Filter {
    Filter {
        if_hz: WIDE_IF_HZ,
        reg0a: 0x10,
        reg0b,
    }
}

fn narrow(bandwidth_hz: u32) -> Filter {
    let mut if_hz = NARROW_IF_START_HZ;
    let mut reg0b = 0x80u8;
    let mut passed = 0;
    let mut left = bandwidth_hz;
    let top = LOW_PASS_HZ[0];
    if left > top + HIGH_PASS_1_HZ {
        left -= HIGH_PASS_2_HZ;
        if_hz += HIGH_PASS_2_HZ;
        passed += HIGH_PASS_2_HZ;
    } else {
        reg0b |= 0x20;
    }
    if left > top {
        left -= HIGH_PASS_1_HZ;
        if_hz += HIGH_PASS_1_HZ;
        passed += HIGH_PASS_1_HZ;
    } else {
        reg0b |= 0x40;
    }
    let index = low_pass_index(left);
    reg0b |= (15 - index as u8) & 0x0f;
    passed += LOW_PASS_HZ[index];
    Filter {
        if_hz: if_hz - passed / 2,
        reg0a: 0x00,
        reg0b,
    }
}

fn low_pass_index(bandwidth_hz: u32) -> usize {
    LOW_PASS_HZ
        .iter()
        .take_while(|cutoff| **cutoff >= bandwidth_hz)
        .count()
        .saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_match_the_vectors() {
        for (bandwidth, if_hz, reg0a, reg0b) in [
            (8_000_000, 4_570_000, 0x10, 0x0b),
            (7_000_001, 4_570_000, 0x10, 0x0b),
            (7_000_000, 4_570_000, 0x10, 0x2a),
            (6_000_001, 4_570_000, 0x10, 0x2a),
            (6_000_000, 3_570_000, 0x10, 0x6b),
            (3_200_000, 3_570_000, 0x10, 0x6b),
            (2_880_000, 3_570_000, 0x10, 0x6b),
            (2_560_000, 3_570_000, 0x10, 0x6b),
            (2_430_001, 3_570_000, 0x10, 0x6b),
            (2_430_000, 1_815_000, 0x00, 0x8f),
            (2_400_000, 1_815_000, 0x00, 0x8f),
            (2_050_001, 1_640_000, 0x00, 0xcf),
            (2_050_000, 1_625_000, 0x00, 0xaf),
            (2_048_000, 1_625_000, 0x00, 0xaf),
            (1_920_000, 1_675_000, 0x00, 0xae),
            (1_700_001, 1_750_000, 0x00, 0xac),
            (1_700_000, 1_450_000, 0x00, 0xef),
            (1_600_000, 1_500_000, 0x00, 0xee),
            (1_536_000, 1_525_000, 0x00, 0xed),
            (1_500_000, 1_525_000, 0x00, 0xed),
            (1_024_000, 1_700_000, 0x00, 0xeb),
            (1_000_000, 1_700_000, 0x00, 0xeb),
            (900_000, 1_850_000, 0x00, 0xea),
            (500_000, 2_025_000, 0x00, 0xe8),
            (300_000, 2_125_000, 0x00, 0xe6),
            (290_000, 2_125_000, 0x00, 0xe6),
            (250_000, 2_125_000, 0x00, 0xe6),
        ] {
            assert_eq!(
                filter(bandwidth),
                Filter {
                    if_hz,
                    reg0a,
                    reg0b
                },
                "{bandwidth}"
            );
        }
    }

    #[test]
    fn the_low_pass_index_is_the_last_cutoff_still_wide_enough() {
        assert_eq!(low_pass_index(1_800_000), 0);
        assert_eq!(low_pass_index(1_700_000), 0);
        assert_eq!(low_pass_index(1_650_000), 0);
        assert_eq!(low_pass_index(1_600_000), 1);
        assert_eq!(low_pass_index(350_000), 9);
        assert_eq!(low_pass_index(0), 9);
    }
}
