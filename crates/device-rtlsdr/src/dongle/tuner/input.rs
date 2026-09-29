use super::TunerKind;
use crate::dongle::board::Board;

pub(crate) const UPCONVERTER_HZ: u32 = 28_800_000;
const VHF_TOP_HZ: u32 = 250_000_000;
const R828D_SPLIT_HZ: u32 = 345_000_000;
const NOTCH_OFF_HZ: [(u32, u32); 3] = [
    (0, 2_200_000),
    (85_000_000, 112_000_000),
    (172_000_000, 242_000_000),
];
const UPCONVERTER_PIN: u8 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Band {
    Hf,
    Vhf,
    Uhf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Reg { reg: u8, value: u8, mask: u8 },
    Pin { pin: u8, high: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Switching {
    pub(crate) steps: Vec<Step>,
    pub(crate) band: Option<Band>,
}

const fn reg(reg: u8, value: u8, mask: u8) -> Step {
    Step::Reg { reg, value, mask }
}

pub(crate) fn rf_hz(board: Board, requested_hz: u32) -> u64 {
    let shift = board.has_upconverter() && requested_hz < UPCONVERTER_HZ;
    u64::from(requested_hz) + if shift { u64::from(UPCONVERTER_HZ) } else { 0 }
}

pub(crate) const fn v4_band(hz: u32) -> Band {
    if hz <= UPCONVERTER_HZ {
        Band::Hf
    } else if hz < VHF_TOP_HZ {
        Band::Vhf
    } else {
        Band::Uhf
    }
}

pub(crate) const fn lite_band(hz: u32) -> Band {
    if hz <= UPCONVERTER_HZ {
        Band::Hf
    } else {
        Band::Uhf
    }
}

pub(crate) fn v4_notch(hz: u32) -> u8 {
    let off = NOTCH_OFF_HZ
        .iter()
        .any(|(low, high)| (*low..=*high).contains(&hz));
    if off { 0x00 } else { 0x08 }
}

const fn v4_input(band: Band) -> u8 {
    match band {
        Band::Hf => 0x20,
        Band::Vhf => 0x60,
        Band::Uhf => 0x00,
    }
}

const fn lite_input(band: Band) -> u8 {
    match band {
        Band::Hf => 0x60,
        Band::Vhf | Band::Uhf => 0x00,
    }
}

const fn upconverter(band: Band) -> Step {
    Step::Pin {
        pin: UPCONVERTER_PIN,
        high: !matches!(band, Band::Hf),
    }
}

const HF_BYPASS: [Step; 2] = [reg(0x1a, 0x40, 0xc3), reg(0x1b, 0x00, 0xff)];

pub(crate) fn switching(board: Board, kind: TunerKind, hz: u32, cached: Option<Band>) -> Switching {
    match (board, kind) {
        (Board::BlogV4, _) => v4(hz, cached),
        (Board::BlogV4Lite, _) => lite(hz, cached),
        (Board::Generic, TunerKind::R828D) => Switching {
            steps: vec![reg(0x05, r828d_input(hz), 0x60)],
            band: cached,
        },
        (Board::Generic, TunerKind::R820T) => Switching {
            steps: Vec::new(),
            band: cached,
        },
    }
}

const fn r828d_input(hz: u32) -> u8 {
    if hz <= R828D_SPLIT_HZ { 0x60 } else { 0x00 }
}

fn v4(hz: u32, cached: Option<Band>) -> Switching {
    let band = v4_band(hz);
    let mut steps = vec![reg(0x17, v4_notch(hz), 0x08)];
    if band == Band::Hf {
        steps.extend(HF_BYPASS);
    }
    if cached != Some(band) {
        let cable2 = if band == Band::Hf { 0x08 } else { 0x00 };
        steps.extend([
            reg(0x06, cable2, 0x08),
            upconverter(band),
            reg(0x05, v4_input(band), 0x60),
        ]);
    }
    Switching {
        steps,
        band: Some(band),
    }
}

fn lite(hz: u32, cached: Option<Band>) -> Switching {
    let band = lite_band(hz);
    let mut steps = Vec::new();
    if band == Band::Hf {
        steps.extend(HF_BYPASS);
    }
    if cached != Some(band) {
        steps.extend([upconverter(band), reg(0x05, lite_input(band), 0x60)]);
    }
    Switching {
        steps,
        band: Some(band),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HF: Step = Step::Pin {
        pin: 5,
        high: false,
    };
    const NOT_HF: Step = Step::Pin { pin: 5, high: true };

    #[test]
    fn the_v4_family_upconverts_below_28_8_mhz() {
        assert_eq!(rf_hz(Board::BlogV4, 7_100_000), 35_900_000);
        assert_eq!(rf_hz(Board::BlogV4Lite, 500_000), 29_300_000);
        assert_eq!(rf_hz(Board::BlogV4, 28_800_000), 28_800_000);
        assert_eq!(rf_hz(Board::Generic, 7_100_000), 7_100_000);
    }

    #[test]
    fn v4_bands_split_at_28_8_and_250_mhz() {
        assert_eq!(v4_band(28_800_000), Band::Hf);
        assert_eq!(v4_band(28_800_001), Band::Vhf);
        assert_eq!(v4_band(249_999_999), Band::Vhf);
        assert_eq!(v4_band(250_000_000), Band::Uhf);
        assert_eq!(lite_band(100_000_000), Band::Uhf);
        assert_eq!(lite_band(28_800_000), Band::Hf);
    }

    #[test]
    fn the_v4_notch_is_off_in_its_three_windows() {
        for hz in [
            0,
            2_200_000,
            85_000_000,
            100_000_000,
            112_000_000,
            172_000_000,
            242_000_000,
        ] {
            assert_eq!(v4_notch(hz), 0x00, "{hz}");
        }
        for hz in [
            2_200_001,
            84_999_999,
            112_000_001,
            171_999_999,
            242_000_001,
            433_920_000,
        ] {
            assert_eq!(v4_notch(hz), 0x08, "{hz}");
        }
    }

    #[test]
    fn a_v4_entering_hf_switches_input_upconverter_and_bypass() {
        let plan = switching(Board::BlogV4, TunerKind::R828D, 7_100_000, None);
        assert_eq!(
            plan.steps,
            [
                reg(0x17, 0x08, 0x08),
                reg(0x1a, 0x40, 0xc3),
                reg(0x1b, 0x00, 0xff),
                reg(0x06, 0x08, 0x08),
                HF,
                reg(0x05, 0x20, 0x60),
            ]
        );
        assert_eq!(plan.band, Some(Band::Hf));
    }

    #[test]
    fn a_v4_staying_on_hf_still_bypasses_the_tracking_filter() {
        let plan = switching(Board::BlogV4, TunerKind::R828D, 1_000_000, Some(Band::Hf));
        assert_eq!(
            plan.steps,
            [
                reg(0x17, 0x00, 0x08),
                reg(0x1a, 0x40, 0xc3),
                reg(0x1b, 0x00, 0xff)
            ]
        );
    }

    #[test]
    fn a_v4_on_vhf_and_uhf_picks_its_inputs() {
        let vhf = switching(Board::BlogV4, TunerKind::R828D, 100_000_000, Some(Band::Hf));
        assert_eq!(
            vhf.steps,
            [
                reg(0x17, 0x00, 0x08),
                reg(0x06, 0x00, 0x08),
                NOT_HF,
                reg(0x05, 0x60, 0x60)
            ]
        );
        let uhf = switching(Board::BlogV4, TunerKind::R828D, 433_920_000, vhf.band);
        assert_eq!(
            uhf.steps,
            [
                reg(0x17, 0x08, 0x08),
                reg(0x06, 0x00, 0x08),
                NOT_HF,
                reg(0x05, 0x00, 0x60)
            ]
        );
        let again = switching(Board::BlogV4, TunerKind::R828D, 434_000_000, uhf.band);
        assert_eq!(again.steps, [reg(0x17, 0x08, 0x08)]);
    }

    #[test]
    fn a_v4_lite_has_no_notch_and_no_vhf() {
        let hf = switching(Board::BlogV4Lite, TunerKind::R820T, 7_100_000, None);
        assert_eq!(
            hf.steps,
            [
                reg(0x1a, 0x40, 0xc3),
                reg(0x1b, 0x00, 0xff),
                HF,
                reg(0x05, 0x60, 0x60)
            ]
        );
        let uhf = switching(Board::BlogV4Lite, TunerKind::R820T, 100_000_000, hf.band);
        assert_eq!(uhf.steps, [NOT_HF, reg(0x05, 0x00, 0x60)]);
        assert_eq!(uhf.band, Some(Band::Uhf));
    }

    #[test]
    fn a_generic_r828d_splits_at_345_mhz_on_every_tune() {
        let low = switching(Board::Generic, TunerKind::R828D, 345_000_000, None);
        assert_eq!(low.steps, [reg(0x05, 0x60, 0x60)]);
        let high = switching(Board::Generic, TunerKind::R828D, 345_000_001, None);
        assert_eq!(high.steps, [reg(0x05, 0x00, 0x60)]);
    }

    #[test]
    fn a_generic_r820t_switches_nothing() {
        let plan = switching(Board::Generic, TunerKind::R820T, 100_000_000, None);
        assert!(plan.steps.is_empty());
    }
}
