use sdrmm_wire::SstvMode;

pub(crate) const SYNC_HZ: f64 = 1_200.0;
pub(crate) const PORCH_HZ: f64 = 1_500.0;
pub(crate) const CHROMA_PORCH_HZ: f64 = 1_900.0;
pub(crate) const HIGH_SEPARATOR_HZ: f64 = 2_300.0;
pub(crate) const BLACK_HZ: f64 = 1_500.0;
pub(crate) const WHITE_HZ: f64 = 2_300.0;
pub(crate) const LEADER_HZ: f64 = 1_900.0;
pub(crate) const VIS_ONE_HZ: f64 = 1_100.0;
pub(crate) const VIS_ZERO_HZ: f64 = 1_300.0;

#[cfg(any(test, feature = "test-signals"))]
pub(crate) const LEADER_MS: f64 = 300.0;
#[cfg(any(test, feature = "test-signals"))]
pub(crate) const BREAK_MS: f64 = 10.0;
pub(crate) const VIS_BIT_MS: f64 = 30.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scan {
    Red,
    Green,
    Blue,
    Luma(u8),
    ChromaR,
    ChromaB,
    ChromaAlternating,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Part {
    Sync,
    Gap(f64),
    AlternatingGap,
    Scan(Scan),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Segment {
    pub(crate) part: Part,
    pub(crate) ms: f64,
}

const fn seg(part: Part, ms: f64) -> Segment {
    Segment { part, ms }
}

pub(crate) struct Timing {
    pub(crate) mode: SstvMode,
    pub(crate) rows_per_line: u16,
    pub(crate) line_ms: f64,
    pub(crate) lead_in_ms: f64,
    pub(crate) sync_offset_ms: f64,
    pub(crate) sync_ms: f64,
    pub(crate) segments: &'static [Segment],
}

impl Timing {
    pub(crate) fn size(&self) -> (u16, u16) {
        self.mode.size()
    }

    #[cfg(any(test, feature = "test-signals"))]
    pub(crate) fn lines(&self) -> u16 {
        self.mode.size().1 / self.rows_per_line
    }

    #[cfg(test)]
    pub(crate) fn seconds(&self) -> f64 {
        (self.lead_in_ms + self.line_ms * f64::from(self.lines())) / 1_000.0
    }

    pub(crate) fn alternates_chroma(&self) -> bool {
        self.segments
            .iter()
            .any(|s| s.part == Part::Scan(Scan::ChromaAlternating))
    }

    pub(crate) fn carries_luma(&self) -> bool {
        self.segments
            .iter()
            .any(|s| matches!(s.part, Part::Scan(Scan::Luma(_))))
    }
}

const ROBOT36: &[Segment] = &[
    seg(Part::Sync, 9.0),
    seg(Part::Gap(PORCH_HZ), 3.0),
    seg(Part::Scan(Scan::Luma(0)), 88.0),
    seg(Part::AlternatingGap, 4.5),
    seg(Part::Gap(CHROMA_PORCH_HZ), 1.5),
    seg(Part::Scan(Scan::ChromaAlternating), 44.0),
];

const ROBOT72: &[Segment] = &[
    seg(Part::Sync, 9.0),
    seg(Part::Gap(PORCH_HZ), 3.0),
    seg(Part::Scan(Scan::Luma(0)), 138.0),
    seg(Part::Gap(PORCH_HZ), 4.5),
    seg(Part::Gap(CHROMA_PORCH_HZ), 1.5),
    seg(Part::Scan(Scan::ChromaR), 69.0),
    seg(Part::Gap(HIGH_SEPARATOR_HZ), 4.5),
    seg(Part::Gap(CHROMA_PORCH_HZ), 1.5),
    seg(Part::Scan(Scan::ChromaB), 69.0),
];

const MARTIN_M1: &[Segment] = &martin(146.432);
const MARTIN_M2: &[Segment] = &martin(73.216);

const fn martin(scan_ms: f64) -> [Segment; 8] {
    [
        seg(Part::Sync, 4.862),
        seg(Part::Gap(PORCH_HZ), 0.572),
        seg(Part::Scan(Scan::Green), scan_ms),
        seg(Part::Gap(PORCH_HZ), 0.572),
        seg(Part::Scan(Scan::Blue), scan_ms),
        seg(Part::Gap(PORCH_HZ), 0.572),
        seg(Part::Scan(Scan::Red), scan_ms),
        seg(Part::Gap(PORCH_HZ), 0.572),
    ]
}

const SCOTTIE_S1: &[Segment] = &scottie(138.24);
const SCOTTIE_S2: &[Segment] = &scottie(88.064);
const SCOTTIE_DX: &[Segment] = &scottie(345.6);

const fn scottie(scan_ms: f64) -> [Segment; 7] {
    [
        seg(Part::Gap(PORCH_HZ), 1.5),
        seg(Part::Scan(Scan::Green), scan_ms),
        seg(Part::Gap(PORCH_HZ), 1.5),
        seg(Part::Scan(Scan::Blue), scan_ms),
        seg(Part::Sync, 9.0),
        seg(Part::Gap(PORCH_HZ), 1.5),
        seg(Part::Scan(Scan::Red), scan_ms),
    ]
}

const fn scottie_sync_offset(scan_ms: f64) -> f64 {
    1.5 + scan_ms + 1.5 + scan_ms
}

const PD50: &[Segment] = &pd(91.52);
const PD90: &[Segment] = &pd(170.24);
const PD120: &[Segment] = &pd(121.6);
const PD180: &[Segment] = &pd(183.04);

const fn pd(scan_ms: f64) -> [Segment; 6] {
    [
        seg(Part::Sync, 20.0),
        seg(Part::Gap(PORCH_HZ), 2.08),
        seg(Part::Scan(Scan::Luma(0)), scan_ms),
        seg(Part::Scan(Scan::ChromaR), scan_ms),
        seg(Part::Scan(Scan::ChromaB), scan_ms),
        seg(Part::Scan(Scan::Luma(1)), scan_ms),
    ]
}

const SC2_180: &[Segment] = &[
    seg(Part::Sync, 5.5225),
    seg(Part::Gap(PORCH_HZ), 0.5),
    seg(Part::Scan(Scan::Red), 235.0),
    seg(Part::Scan(Scan::Green), 235.0),
    seg(Part::Scan(Scan::Blue), 235.0),
];

const TIMINGS: &[Timing] = &[
    Timing {
        mode: SstvMode::Robot36,
        rows_per_line: 1,
        line_ms: 150.0,
        lead_in_ms: 0.0,
        sync_offset_ms: 0.0,
        sync_ms: 9.0,
        segments: ROBOT36,
    },
    Timing {
        mode: SstvMode::Robot72,
        rows_per_line: 1,
        line_ms: 300.0,
        lead_in_ms: 0.0,
        sync_offset_ms: 0.0,
        sync_ms: 9.0,
        segments: ROBOT72,
    },
    Timing {
        mode: SstvMode::MartinM1,
        rows_per_line: 1,
        line_ms: 446.446,
        lead_in_ms: 0.0,
        sync_offset_ms: 0.0,
        sync_ms: 4.862,
        segments: MARTIN_M1,
    },
    Timing {
        mode: SstvMode::MartinM2,
        rows_per_line: 1,
        line_ms: 226.798,
        lead_in_ms: 0.0,
        sync_offset_ms: 0.0,
        sync_ms: 4.862,
        segments: MARTIN_M2,
    },
    Timing {
        mode: SstvMode::ScottieS1,
        rows_per_line: 1,
        line_ms: 428.22,
        lead_in_ms: 9.0,
        sync_offset_ms: scottie_sync_offset(138.24),
        sync_ms: 9.0,
        segments: SCOTTIE_S1,
    },
    Timing {
        mode: SstvMode::ScottieS2,
        rows_per_line: 1,
        line_ms: 277.692,
        lead_in_ms: 9.0,
        sync_offset_ms: scottie_sync_offset(88.064),
        sync_ms: 9.0,
        segments: SCOTTIE_S2,
    },
    Timing {
        mode: SstvMode::ScottieDx,
        rows_per_line: 1,
        line_ms: 1_050.3,
        lead_in_ms: 9.0,
        sync_offset_ms: scottie_sync_offset(345.6),
        sync_ms: 9.0,
        segments: SCOTTIE_DX,
    },
    Timing {
        mode: SstvMode::Pd50,
        rows_per_line: 2,
        line_ms: 388.16,
        lead_in_ms: 0.0,
        sync_offset_ms: 0.0,
        sync_ms: 20.0,
        segments: PD50,
    },
    Timing {
        mode: SstvMode::Pd90,
        rows_per_line: 2,
        line_ms: 703.04,
        lead_in_ms: 0.0,
        sync_offset_ms: 0.0,
        sync_ms: 20.0,
        segments: PD90,
    },
    Timing {
        mode: SstvMode::Pd120,
        rows_per_line: 2,
        line_ms: 508.48,
        lead_in_ms: 0.0,
        sync_offset_ms: 0.0,
        sync_ms: 20.0,
        segments: PD120,
    },
    Timing {
        mode: SstvMode::Pd180,
        rows_per_line: 2,
        line_ms: 754.24,
        lead_in_ms: 0.0,
        sync_offset_ms: 0.0,
        sync_ms: 20.0,
        segments: PD180,
    },
    Timing {
        mode: SstvMode::Sc2180,
        rows_per_line: 1,
        line_ms: 711.0225,
        lead_in_ms: 0.0,
        sync_offset_ms: 0.0,
        sync_ms: 5.5225,
        segments: SC2_180,
    },
];

pub(crate) fn timing(mode: SstvMode) -> &'static Timing {
    match TIMINGS.iter().find(|row| row.mode == mode) {
        Some(row) => row,
        None => &TIMINGS[0],
    }
}

#[cfg(test)]
pub(crate) fn longest_line_ms() -> f64 {
    TIMINGS
        .iter()
        .map(|t| t.line_ms)
        .fold(0.0f64, |acc, ms| acc.max(ms))
}

#[cfg(any(test, feature = "test-signals"))]
#[must_use]
pub(crate) fn level_to_hz(level: u8) -> f64 {
    BLACK_HZ + (WHITE_HZ - BLACK_HZ) * f64::from(level) / 255.0
}

#[must_use]
pub(crate) fn hz_to_level(hz: f32) -> u8 {
    let scaled = (f64::from(hz) - BLACK_HZ) * 255.0 / (WHITE_HZ - BLACK_HZ);
    scaled.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_has_one_timing_row_in_wire_order() {
        assert_eq!(TIMINGS.len(), SstvMode::ALL.len());
        for (row, &mode) in TIMINGS.iter().zip(SstvMode::ALL.iter()) {
            assert_eq!(row.mode, mode, "timing table order");
            assert_eq!(timing(mode).mode, mode);
        }
    }

    #[test]
    fn segments_add_up_to_the_stated_line_time() {
        for row in TIMINGS {
            let total: f64 = row.segments.iter().map(|s| s.ms).sum();
            assert!(
                (total - row.line_ms).abs() < 1e-6,
                "{:?} segments total {total} ms against a {} ms line",
                row.mode,
                row.line_ms
            );
        }
    }

    #[test]
    fn the_sync_offset_points_at_the_sync_segment() {
        for row in TIMINGS {
            let mut at = 0.0;
            let mut found = None;
            for segment in row.segments {
                if segment.part == Part::Sync {
                    found = Some((at, segment.ms));
                    break;
                }
                at += segment.ms;
            }
            let (offset, ms) = found.unwrap_or_else(|| panic!("{:?} has no sync", row.mode));
            assert!(
                (offset - row.sync_offset_ms).abs() < 1e-6,
                "{:?} sync at {offset} ms against a stated {} ms",
                row.mode,
                row.sync_offset_ms
            );
            assert!((ms - row.sync_ms).abs() < 1e-9, "{:?} sync width", row.mode);
        }
    }

    #[test]
    fn each_mode_scans_every_row_of_its_picture_exactly_once() {
        for row in TIMINGS {
            let (_, height) = row.size();
            assert_eq!(
                height % row.rows_per_line,
                0,
                "{:?} splits {height} rows into {} per line",
                row.mode,
                row.rows_per_line
            );
            let luma: Vec<u8> = row
                .segments
                .iter()
                .filter_map(|s| match s.part {
                    Part::Scan(Scan::Luma(index)) => Some(index),
                    _ => None,
                })
                .collect();
            if row.rows_per_line == 2 {
                assert_eq!(luma, vec![0, 1], "{:?} luma rows", row.mode);
            }
        }
    }

    #[test]
    fn transmission_times_match_the_published_mode_durations() {
        for (mode, seconds) in [
            (SstvMode::Robot36, 36.0),
            (SstvMode::Robot72, 72.0),
            (SstvMode::MartinM1, 114.3),
            (SstvMode::MartinM2, 58.1),
            (SstvMode::ScottieS1, 109.6),
            (SstvMode::ScottieS2, 71.1),
            (SstvMode::ScottieDx, 268.9),
            (SstvMode::Pd50, 49.7),
            (SstvMode::Pd90, 90.0),
            (SstvMode::Pd120, 126.1),
            (SstvMode::Pd180, 187.1),
            (SstvMode::Sc2180, 182.0),
        ] {
            let actual = timing(mode).seconds();
            assert!(
                (actual - seconds).abs() < 0.15,
                "{mode:?} runs {actual:.2} s against a published {seconds} s"
            );
        }
    }

    #[test]
    fn vis_codes_are_unique_and_round_trip() {
        for &mode in &SstvMode::ALL {
            assert_eq!(SstvMode::from_vis(mode.vis()), Some(mode));
        }
        assert_eq!(SstvMode::from_vis(0), None);
    }

    #[test]
    fn black_and_white_sit_at_the_ends_of_the_video_band() {
        assert_eq!(hz_to_level(BLACK_HZ as f32), 0);
        assert_eq!(hz_to_level(WHITE_HZ as f32), 255);
        assert!((level_to_hz(0) - BLACK_HZ).abs() < 1e-9);
        assert!((level_to_hz(255) - WHITE_HZ).abs() < 1e-9);
        for level in [0u8, 1, 64, 128, 200, 255] {
            assert_eq!(hz_to_level(level_to_hz(level) as f32), level);
        }
        assert_eq!(hz_to_level(900.0), 0);
        assert_eq!(hz_to_level(3_000.0), 255);
    }
}
