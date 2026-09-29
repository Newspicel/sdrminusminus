use crate::dongle::error::PllFault;

const VCO_MIN_KHZ: u64 = 1_770_000;
const VCO_MAX_KHZ: u64 = 3_540_000;
const DIVIDERS: [u32; 6] = [2, 4, 8, 16, 32, 64];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Divider {
    pub(crate) ratio: u32,
    pub(crate) code: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Synth {
    pub(crate) nint: u8,
    pub(crate) sdm: u16,
}

pub(crate) const fn lo_khz(lo_hz: u64) -> u64 {
    (lo_hz + 500) / 1000
}

pub(crate) fn divider(lo_hz: u64) -> Result<Divider, PllFault> {
    let khz = lo_khz(lo_hz);
    DIVIDERS
        .iter()
        .zip(0u8..)
        .find(|(ratio, _)| (VCO_MIN_KHZ..VCO_MAX_KHZ).contains(&(khz * u64::from(**ratio))))
        .map(|(ratio, code)| Divider {
            ratio: *ratio,
            code,
        })
        .ok_or(PllFault::NoDivider)
}

pub(crate) const fn tuned_code(code: u8, fine: u8, vpr: u8) -> u8 {
    if fine > vpr {
        code.wrapping_sub(1)
    } else if fine < vpr {
        code.wrapping_add(1)
    } else {
        code
    }
}

pub(crate) fn synth(
    lo_hz: u64,
    divider: Divider,
    reference_hz: u32,
    vpr: u8,
) -> Result<Synth, PllFault> {
    let reference = u64::from(reference_hz.max(1));
    let vco_hz = lo_hz * u64::from(divider.ratio);
    let vco_div = (reference + 65_536 * vco_hz) / (2 * reference);
    let nint = (vco_div / 65_536) as u8;
    let sdm = (vco_div % 65_536) as u16;
    if u32::from(nint) > 128 / u32::from(vpr.max(1)) - 1 {
        return Err(PllFault::NintTooLarge);
    }
    Ok(Synth { nint, sdm })
}

pub(crate) const fn nint_reg(nint: u8) -> u8 {
    let ni = nint.wrapping_sub(13) / 4;
    let si = nint.wrapping_sub(ni.wrapping_mul(4)).wrapping_sub(13);
    ni.wrapping_add(si << 6)
}

pub(crate) const fn modulator_bits(sdm: u16, dither: bool) -> u8 {
    let exact = if sdm == 0 { 0x08 } else { 0x00 };
    let undithered = if dither { 0x00 } else { 0x10 };
    exact | undithered
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Row {
        lo: u64,
        ratio: u32,
        code: u8,
        nint: u8,
        sdm: u16,
        reg14: u8,
    }

    const fn row(lo: u64, ratio: u32, code: u8, nint: u8, sdm: u16, reg14: u8) -> Row {
        Row {
            lo,
            ratio,
            code,
            nint,
            sdm,
            reg14,
        }
    }

    fn check(reference: u32, vpr: u8, rows: &[Row]) {
        for r in rows {
            let div = divider(r.lo).unwrap();
            assert_eq!((div.ratio, div.code), (r.ratio, r.code), "{}", r.lo);
            let got = synth(r.lo, div, reference, vpr).unwrap();
            assert_eq!(
                got,
                Synth {
                    nint: r.nint,
                    sdm: r.sdm
                },
                "{}",
                r.lo
            );
            assert_eq!(nint_reg(got.nint), r.reg14, "{}", r.lo);
        }
    }

    #[test]
    fn a_28_8_mhz_reference_matches_the_vectors() {
        let rows = [
            row(56_000_000, 32, 4, 31, 0x1c72, 0x84),
            row(39_470_000, 64, 5, 43, 0xdb06, 0x87),
            row(103_570_000, 32, 4, 57, 0x89f5, 0x0b),
            row(146_300_000, 16, 3, 40, 0xa38e, 0xc6),
            row(437_490_000, 8, 2, 60, 0xc333, 0xcb),
            row(1_093_570_000, 2, 0, 37, 0xf89f, 0x06),
            row(1_769_570_000, 2, 0, 61, 0x7183, 0x0c),
        ];
        check(28_800_000, 2, &rows);
        check(28_800_000, 1, &rows);
    }

    #[test]
    fn a_16_mhz_reference_matches_the_vectors() {
        check(
            16_000_000,
            1,
            &[
                row(56_000_000, 32, 4, 56, 0x0000, 0xca),
                row(39_470_000, 64, 5, 78, 0xf0a4, 0x50),
                row(103_570_000, 32, 4, 103, 0x91ec, 0x96),
                row(146_300_000, 16, 3, 73, 0x2666, 0x0f),
                row(437_490_000, 8, 2, 109, 0x5f5c, 0x18),
                row(1_093_570_000, 2, 0, 68, 0x591f, 0xcd),
                row(1_769_570_000, 2, 0, 110, 0x991f, 0x58),
            ],
        );
    }

    #[test]
    fn an_lo_without_a_divider_is_a_pll_error() {
        assert_eq!(divider(27_570_000), Err(PllFault::NoDivider));
        assert_eq!(divider(1_800_000_000), Err(PllFault::NoDivider));
    }

    #[test]
    fn nint_is_capped_by_the_vco_power_reference() {
        let div = divider(1_769_570_000).unwrap();
        assert_eq!(
            synth(1_769_570_000, div, 16_000_000, 2),
            Err(PllFault::NintTooLarge)
        );
        assert!(synth(1_769_570_000, div, 16_000_000, 1).is_ok());
    }

    #[test]
    fn fine_tune_moves_the_divider_code_toward_the_reference() {
        assert_eq!(tuned_code(4, 3, 2), 3);
        assert_eq!(tuned_code(4, 1, 2), 5);
        assert_eq!(tuned_code(4, 2, 2), 4);
        assert_eq!(tuned_code(0, 3, 2), 0xff);
    }

    #[test]
    fn modulator_bits_cover_every_case() {
        assert_eq!(modulator_bits(0, true), 0x08);
        assert_eq!(modulator_bits(5, true), 0x00);
        assert_eq!(modulator_bits(0, false), 0x18);
        assert_eq!(modulator_bits(5, false), 0x10);
    }
}
