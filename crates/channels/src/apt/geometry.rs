pub(crate) const WORD_RATE_HZ: f64 = 4_160.0;
pub(crate) const LINE_WORDS: usize = 2_080;
pub(crate) const SIDE_WORDS: usize = LINE_WORDS / 2;
pub(crate) const SYNC_WORDS: usize = 39;
pub(crate) const SPACE_WORDS: usize = 47;
pub(crate) const IMAGE_WORDS: usize = 909;
pub(crate) const TELEMETRY_WORDS: usize = 45;
pub(crate) const SPACE_START: usize = SYNC_WORDS;
pub(crate) const IMAGE_START: usize = SPACE_START + SPACE_WORDS;
pub(crate) const TELEMETRY_START: usize = IMAGE_START + IMAGE_WORDS;
pub(crate) const LINE_SECONDS: f64 = LINE_WORDS as f64 / WORD_RATE_HZ;

pub(crate) const SUBCARRIER_HZ: f64 = 2_400.0;
pub(crate) const DEVIATION_HZ: f64 = 17_000.0;

pub(crate) const WEDGE_LINES: usize = 8;
pub(crate) const FRAME_WEDGES: usize = 16;
pub(crate) const FRAME_LINES: usize = WEDGE_LINES * FRAME_WEDGES;
pub(crate) const RAMP_WEDGES: usize = 8;
pub(crate) const ZERO_WEDGE: usize = 9;
pub(crate) const CHANNEL_WEDGE: usize = 16;

pub(crate) const MINUTE_LINES: usize = 120;
pub(crate) const MARKER_LINES: usize = 2;

const SYNC_CYCLES: usize = 7;
const SYNC_LEAD_WORDS: usize = 4;

const fn sync_pattern(period: usize, high: usize) -> [bool; SYNC_WORDS] {
    let mut pattern = [false; SYNC_WORDS];
    let mut word = 0;
    while word < SYNC_CYCLES * period {
        pattern[SYNC_LEAD_WORDS + word] = word % period < high;
        word += 1;
    }
    pattern
}

pub(crate) const SYNC_A: [bool; SYNC_WORDS] = sync_pattern(4, 2);
pub(crate) const SYNC_B: [bool; SYNC_WORDS] = sync_pattern(5, 3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    A,
    B,
}

impl Side {
    pub(crate) const fn offset(self) -> usize {
        match self {
            Self::A => 0,
            Self::B => SIDE_WORDS,
        }
    }

    pub(crate) const fn image(self) -> std::ops::Range<usize> {
        self.offset() + IMAGE_START..self.offset() + TELEMETRY_START
    }

    pub(crate) const fn telemetry(self) -> std::ops::Range<usize> {
        self.offset() + TELEMETRY_START..self.offset() + TELEMETRY_START + TELEMETRY_WORDS
    }
}

pub(crate) const fn wedge_level(wedge: usize) -> u8 {
    if wedge >= 1 && wedge <= RAMP_WEDGES {
        ((wedge * 255 + RAMP_WEDGES / 2) / RAMP_WEDGES) as u8
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_adds_up_to_2080_words() {
        assert_eq!(TELEMETRY_START + TELEMETRY_WORDS, SIDE_WORDS);
        assert_eq!(Side::B.telemetry().end, LINE_WORDS);
        assert_eq!(Side::A.image().len(), IMAGE_WORDS);
    }

    #[test]
    fn sync_a_carries_seven_1040_hz_cycles() {
        let highs = SYNC_A.iter().filter(|&&high| high).count();
        assert_eq!(highs, 14);
        let rises = SYNC_A.windows(2).filter(|pair| !pair[0] && pair[1]).count();
        assert_eq!(rises, 7);
        assert_eq!(SYNC_B.iter().filter(|&&high| high).count(), 21);
    }

    #[test]
    fn the_ramp_climbs_to_full_scale() {
        assert_eq!(wedge_level(1), 32);
        assert_eq!(wedge_level(RAMP_WEDGES), 255);
        assert_eq!(wedge_level(ZERO_WEDGE), 0);
    }
}
