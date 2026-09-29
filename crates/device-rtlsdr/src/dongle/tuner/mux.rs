#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Mux {
    pub(crate) open_drain: u8,
    pub(crate) rf_mux: u8,
    pub(crate) tracking: u8,
}

const fn band(open_drain: u8, rf_mux: u8, tracking: u8) -> Mux {
    Mux {
        open_drain,
        rf_mux,
        tracking,
    }
}

const BANDS: [(u32, Mux); 21] = [
    (0, band(0x08, 0x02, 0xdf)),
    (50, band(0x08, 0x02, 0xbe)),
    (55, band(0x08, 0x02, 0x8b)),
    (60, band(0x08, 0x02, 0x7b)),
    (65, band(0x08, 0x02, 0x69)),
    (70, band(0x08, 0x02, 0x58)),
    (75, band(0x00, 0x02, 0x44)),
    (80, band(0x00, 0x02, 0x44)),
    (90, band(0x00, 0x02, 0x34)),
    (100, band(0x00, 0x02, 0x34)),
    (110, band(0x00, 0x02, 0x24)),
    (120, band(0x00, 0x02, 0x24)),
    (140, band(0x00, 0x02, 0x14)),
    (180, band(0x00, 0x02, 0x13)),
    (220, band(0x00, 0x02, 0x13)),
    (250, band(0x00, 0x02, 0x11)),
    (280, band(0x00, 0x02, 0x00)),
    (310, band(0x00, 0x41, 0x00)),
    (450, band(0x00, 0x41, 0x00)),
    (588, band(0x00, 0x40, 0x00)),
    (650, band(0x00, 0x40, 0x00)),
];

pub(crate) fn mux_for(lo_hz: u64) -> Mux {
    let mhz = lo_hz / 1_000_000;
    BANDS
        .iter()
        .rev()
        .find(|(start, _)| u64::from(*start) <= mhz)
        .map_or(BANDS[0].1, |(_, mux)| *mux)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_band_starting_at_or_below_the_lo_wins() {
        assert_eq!(mux_for(0), band(0x08, 0x02, 0xdf));
        assert_eq!(mux_for(49_999_999), band(0x08, 0x02, 0xdf));
        assert_eq!(mux_for(50_000_000), band(0x08, 0x02, 0xbe));
        assert_eq!(mux_for(103_570_000), band(0x00, 0x02, 0x34));
        assert_eq!(mux_for(437_490_000), band(0x00, 0x41, 0x00));
        assert_eq!(mux_for(1_093_570_000), band(0x00, 0x40, 0x00));
    }
}
