use sdrmm_wire::cps::Tone;

pub const CTCSS_DECIHERTZ: [u16; 51] = [
    625, 670, 693, 719, 744, 770, 797, 825, 854, 885, 915, 948, 974, 1000, 1035, 1072, 1109, 1148,
    1188, 1230, 1273, 1318, 1365, 1413, 1462, 1514, 1567, 1598, 1622, 1655, 1679, 1713, 1738, 1773,
    1799, 1835, 1862, 1899, 1928, 1966, 1995, 2035, 2065, 2107, 2181, 2257, 2291, 2336, 2418, 2503,
    2541,
];

pub const CTCSS_NONE_INDEX: u8 = 51;

#[must_use]
pub fn ctcss_index(decihertz: u16) -> Option<u8> {
    CTCSS_DECIHERTZ
        .iter()
        .position(|entry| *entry == decihertz)
        .and_then(|index| u8::try_from(index).ok())
}

#[must_use]
pub fn nearest_ctcss_index(decihertz: u16) -> u8 {
    CTCSS_DECIHERTZ
        .iter()
        .enumerate()
        .min_by_key(|(_, entry)| entry.abs_diff(decihertz))
        .and_then(|(index, _)| u8::try_from(index).ok())
        .unwrap_or(CTCSS_NONE_INDEX)
}

#[must_use]
pub fn ctcss_from_index(index: u8) -> Option<u16> {
    CTCSS_DECIHERTZ.get(usize::from(index)).copied()
}

#[must_use]
pub fn dcs_to_binary(code: u16) -> u16 {
    let mut binary = 0u16;
    let mut scale = 1u16;
    let mut rest = code;
    while rest > 0 && scale <= 0o100 {
        binary += (rest % 10) * scale;
        rest /= 10;
        scale = scale.saturating_mul(8);
    }
    binary & 0x1ff
}

#[must_use]
pub fn dcs_from_binary(binary: u16) -> u16 {
    let mut code = 0u16;
    let mut scale = 1u16;
    let mut rest = binary & 0x1ff;
    while rest > 0 {
        code += (rest % 8) * scale;
        rest /= 8;
        scale = scale.saturating_mul(10);
    }
    code
}

#[must_use]
pub fn is_standard_dcs(code: u16) -> bool {
    dcs_from_binary(dcs_to_binary(code)) == code
}

#[must_use]
pub fn describe(tone: Tone) -> String {
    match tone {
        Tone::Ctcss { decihertz } => format!("{:.1} Hz", f64::from(decihertz) / 10.0),
        Tone::Dcs { code, inverted } => {
            format!("D{code:03}{}", if inverted { "I" } else { "N" })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_anytone_table_indexes_the_standard_tones() {
        assert_eq!(ctcss_index(670), Some(1));
        assert_eq!(ctcss_index(1000), Some(13));
        assert_eq!(ctcss_index(2541), Some(50));
        assert_eq!(ctcss_index(1234), None);
        assert_eq!(ctcss_from_index(13), Some(1000));
        assert_eq!(ctcss_from_index(CTCSS_NONE_INDEX), None);
    }

    #[test]
    fn a_non_standard_tone_snaps_to_its_nearest_table_entry() {
        assert_eq!(nearest_ctcss_index(1001), 13);
        assert_eq!(ctcss_from_index(nearest_ctcss_index(1001)), Some(1000));
    }

    #[test]
    fn dcs_codes_are_octal_on_the_wire_and_decimal_on_the_face() {
        assert_eq!(dcs_to_binary(23), 0o23);
        assert_eq!(dcs_from_binary(0o23), 23);
        assert_eq!(dcs_to_binary(754), 0o754);
        assert_eq!(dcs_from_binary(0o754), 754);
        assert!(is_standard_dcs(131));
        assert!(!is_standard_dcs(199));
    }

    #[test]
    fn tones_describe_themselves_for_a_report() {
        assert_eq!(describe(Tone::Ctcss { decihertz: 1000 }), "100.0 Hz");
        assert_eq!(
            describe(Tone::Dcs {
                code: 23,
                inverted: true
            }),
            "D023I"
        );
    }
}
