use super::{
    chip::{Block, Chip, Value},
    error::{Error, Invalid, Result},
    usb::Transport,
};

pub(crate) const CRYSTAL_HZ: u32 = 28_800_000;
pub(crate) const DIRECT_MAX_HZ: u32 = CRYSTAL_HZ / 2;
pub(crate) const PPM_LIMIT: i32 = 488;

const SYSCTL: u16 = 0x2000;
const EPA_CTL: u16 = 0x2148;
const EPA_MAXPKT: u16 = 0x2158;
const DEMOD_CTL: u16 = 0x3000;
const DEMOD_CTL_1: u16 = 0x300b;

const ENDPOINT_HELD: u16 = 0x1002;
const ENDPOINT_RUNNING: u16 = 0x0000;

const IF_MIN: i64 = -(1 << 21);
const PPM_WORD_MAX: i64 = 0x1fff;
const RATE_WINDOWS: [(u32, u32); 2] = [(225_001, 300_000), (900_001, 3_200_000)];

const FIR: [i16; 16] = [
    -54, -36, -41, -40, -32, -14, 14, 53, 101, 156, 215, 273, 327, 372, 404, 421,
];
const FIR_START: u8 = 0x1c;

const SOFT_RESET: u8 = 0x14;
const SOFT_RUN: u8 = 0x10;
const SDR_MODE: u8 = 0x05;
#[cfg(test)]
const COUNTER_MODE: u8 = 0x03;
const ZERO_IF_OFF: u8 = 0x1a;
const IN_PHASE_ONLY: u8 = 0x4d;
const ADC_I: u8 = 0x80;
const ADC_Q: u8 = 0x90;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Branch {
    I,
    Q,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Resampler {
    pub(crate) ratio: u32,
    pub(crate) actual: u32,
}

const fn crystal_scaled() -> u64 {
    (CRYSTAL_HZ as u64) << 22
}

pub(crate) fn resampler(rate: u32) -> Result<Resampler> {
    if !RATE_WINDOWS
        .iter()
        .any(|(low, high)| (*low..=*high).contains(&rate))
    {
        return Err(Error::SampleRate(rate));
    }
    let ratio = (crystal_scaled() / u64::from(rate)) as u32 & 0x0fff_fffc;
    let real = ratio | ((ratio & 0x0800_0000) << 1);
    let actual = (crystal_scaled() / u64::from(real)) as u32;
    Ok(Resampler { ratio, actual })
}

pub(crate) fn if_word(if_hz: u32, crystal_hz: u32) -> Option<i32> {
    let word = -((i64::from(if_hz) << 22) / i64::from(crystal_hz.max(1)));
    (word >= IF_MIN).then_some(word as i32)
}

pub(crate) const fn if_bytes(word: i32) -> [u8; 3] {
    [
        ((word >> 16) & 0x3f) as u8,
        ((word >> 8) & 0xff) as u8,
        (word & 0xff) as u8,
    ]
}

pub(crate) fn ppm_word(ppm: i32) -> Result<i32> {
    if !(-PPM_LIMIT..=PPM_LIMIT).contains(&ppm) {
        return Err(Invalid::Ppm(ppm).into());
    }
    let word = -(i64::from(ppm) << 24) / 1_000_000;
    if word.abs() > PPM_WORD_MAX {
        return Err(Invalid::Ppm(ppm).into());
    }
    Ok(word as i32)
}

pub(crate) const fn ppm_bytes(word: i32) -> [u8; 2] {
    [(word & 0xff) as u8, ((word >> 8) & 0x3f) as u8]
}

pub(crate) fn corrected(nominal_hz: u32, ppm: i32) -> u32 {
    let scaled = i128::from(nominal_hz) * i128::from(1_000_000 + i64::from(ppm));
    let rounded = if scaled >= 0 {
        (scaled + 500_000) / 1_000_000
    } else {
        (scaled - 500_000) / 1_000_000
    };
    rounded.clamp(0, i128::from(u32::MAX)) as u32
}

pub(crate) fn fir_bytes(taps: &[i16; 16]) -> [u8; 20] {
    let mut bytes = [0u8; 20];
    for (byte, tap) in bytes.iter_mut().zip(&taps[..8]) {
        *byte = *tap as u8;
    }
    let (pairs, _) = taps[8..].as_chunks::<2>();
    let (outs, _) = bytes[8..].as_chunks_mut::<3>();
    for ([a, b], out) in pairs.iter().zip(outs) {
        let (a, b) = (*a as u16, *b as u16);
        *out = [
            (a >> 4) as u8,
            (((a & 0x0f) << 4) | ((b >> 8) & 0x0f)) as u8,
            (b & 0xff) as u8,
        ];
    }
    bytes
}

pub(crate) fn reset_usb<T: Transport>(chip: &Chip<T>) -> Result<()> {
    chip.write_block(Block::Usb, SYSCTL, Value::Byte(0x09))
}

pub(crate) fn init_baseband<T: Transport>(chip: &Chip<T>) -> Result<()> {
    reset_usb(chip)?;
    chip.write_block(Block::Usb, EPA_MAXPKT, Value::Word(0x0002))?;
    hold_endpoint(chip)?;
    chip.write_block(Block::Sys, DEMOD_CTL_1, Value::Byte(0x22))?;
    chip.write_block(Block::Sys, DEMOD_CTL, Value::Byte(0xe8))?;
    soft_reset(chip)?;
    chip.demod(1, 0x15, 0x00)?;
    chip.write_demod(1, 0x16, Value::Word(0x0000))?;
    for reg in 0x16..=0x1a {
        chip.demod(1, reg, 0x00)?;
    }
    write_fir(chip)?;
    chip.demod(0, 0x19, SDR_MODE)?;
    chip.demod(1, 0x93, 0xf0)?;
    chip.demod(1, 0x94, 0x0f)?;
    chip.demod(1, 0x11, 0x00)?;
    chip.demod(1, 0x04, 0x00)?;
    chip.demod(0, 0x61, 0x60)?;
    chip.demod(0, 0x06, ADC_I)?;
    chip.demod(1, 0xb1, 0x1b)?;
    chip.demod(0, 0x0d, 0x83)
}

fn write_fir<T: Transport>(chip: &Chip<T>) -> Result<()> {
    for (reg, byte) in (FIR_START..).zip(fir_bytes(&FIR)) {
        chip.demod(1, reg, byte)?;
    }
    Ok(())
}

pub(crate) fn soft_reset<T: Transport>(chip: &Chip<T>) -> Result<()> {
    chip.demod(1, 0x01, SOFT_RESET)?;
    chip.demod(1, 0x01, SOFT_RUN)
}

pub(crate) fn tuner_path<T: Transport>(chip: &Chip<T>, if_hz: u32, crystal_hz: u32) -> Result<()> {
    chip.demod(1, 0xb1, ZERO_IF_OFF)?;
    chip.demod(0, 0x08, IN_PHASE_ONLY)?;
    write_if(chip, if_hz, crystal_hz)?;
    chip.demod(1, 0x15, 0x01)
}

pub(crate) fn direct_path<T: Transport>(chip: &Chip<T>, branch: Branch) -> Result<()> {
    chip.demod(1, 0xb1, ZERO_IF_OFF)?;
    chip.demod(1, 0x15, 0x00)?;
    chip.demod(0, 0x08, IN_PHASE_ONLY)?;
    select_adc(chip, branch)
}

pub(crate) fn select_adc<T: Transport>(chip: &Chip<T>, branch: Branch) -> Result<()> {
    let value = match branch {
        Branch::I => ADC_I,
        Branch::Q => ADC_Q,
    };
    chip.demod(0, 0x06, value)
}

pub(crate) fn write_if<T: Transport>(chip: &Chip<T>, if_hz: u32, crystal_hz: u32) -> Result<()> {
    let word = if_word(if_hz, crystal_hz).ok_or(Invalid::DirectCenter(if_hz))?;
    for (reg, byte) in (0x19..).zip(if_bytes(word)) {
        chip.demod(1, reg, byte)?;
    }
    Ok(())
}

pub(crate) fn write_ratio<T: Transport>(chip: &Chip<T>, ratio: u32) -> Result<()> {
    chip.write_demod(1, 0x9f, Value::Word((ratio >> 16) as u16))?;
    chip.write_demod(1, 0xa1, Value::Word((ratio & 0xffff) as u16))
}

pub(crate) fn write_ppm<T: Transport>(chip: &Chip<T>, ppm: i32) -> Result<()> {
    let [low, high] = ppm_bytes(ppm_word(ppm)?);
    chip.demod(1, 0x3f, low)?;
    chip.demod(1, 0x3e, high)
}

pub(crate) fn hold_endpoint<T: Transport>(chip: &Chip<T>) -> Result<()> {
    chip.write_block(Block::Usb, EPA_CTL, Value::Word(ENDPOINT_HELD))
}

pub(crate) fn release_endpoint<T: Transport>(chip: &Chip<T>) -> Result<()> {
    chip.write_block(Block::Usb, EPA_CTL, Value::Word(ENDPOINT_RUNNING))
}

#[cfg(test)]
pub(crate) fn set_counter<T: Transport>(chip: &Chip<T>, on: bool) -> Result<()> {
    chip.demod(0, 0x19, if on { COUNTER_MODE } else { SDR_MODE })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dongle::fake::Fake;

    #[test]
    fn the_default_fir_packs_to_the_known_bytes() {
        assert_eq!(
            fir_bytes(&FIR),
            [
                0xca, 0xdc, 0xd7, 0xd8, 0xe0, 0xf2, 0x0e, 0x35, 0x06, 0x50, 0x9c, 0x0d, 0x71, 0x11,
                0x14, 0x71, 0x74, 0x19, 0x41, 0xa5
            ]
        );
    }

    #[test]
    fn if_registers_match_the_vectors() {
        for (hz, word, bytes) in [
            (0, 0, [0x00, 0x00, 0x00]),
            (1_000_000, -145_635, [0x3d, 0xc7, 0x1d]),
            (1_625_000, -236_657, [0x3c, 0x63, 0x8f]),
            (1_815_000, -264_328, [0x3b, 0xf7, 0x78]),
            (2_300_000, -334_961, [0x3a, 0xe3, 0x8f]),
            (3_570_000, -519_918, [0x38, 0x11, 0x12]),
            (4_570_000, -665_554, [0x35, 0xd8, 0x2e]),
            (7_100_000, -1_034_012, [0x30, 0x38, 0xe4]),
            (7_200_000, -1_048_576, [0x30, 0x00, 0x00]),
            (14_400_000, -2_097_152, [0x20, 0x00, 0x00]),
        ] {
            assert_eq!(if_word(hz, CRYSTAL_HZ), Some(word), "{hz}");
            assert_eq!(if_bytes(word), bytes, "{hz}");
        }
    }

    #[test]
    fn past_half_the_crystal_the_if_register_overflows() {
        assert_eq!(if_word(14_400_100, CRYSTAL_HZ), None);
    }

    #[test]
    fn the_if_register_follows_the_corrected_crystal() {
        let crystal = corrected(CRYSTAL_HZ, 100);
        assert_eq!(if_word(7_100_000, crystal), Some(-1_033_909));
    }

    #[test]
    fn resampler_matches_the_vectors() {
        for (rate, ratio) in [
            (225_001, 0x0fff_f6ac),
            (250_000, 0x0ccc_cccc),
            (300_000, 0x0800_0000),
            (900_001, 0x07ff_ff68),
            (1_000_000, 0x0733_3330),
            (1_024_000, 0x0708_0000),
            (1_536_000, 0x04b0_0000),
            (1_920_000, 0x03c0_0000),
            (2_000_000, 0x0399_9998),
            (2_048_000, 0x0384_0000),
            (2_400_000, 0x0300_0000),
            (2_560_000, 0x02d0_0000),
            (2_880_000, 0x0280_0000),
            (3_000_000, 0x0266_6664),
            (3_200_000, 0x0240_0000),
        ] {
            assert_eq!(
                resampler(rate).unwrap(),
                Resampler {
                    ratio,
                    actual: rate
                },
                "{rate}"
            );
        }
    }

    #[test]
    fn rates_outside_the_windows_are_refused() {
        for rate in [0, 225_000, 300_001, 600_000, 900_000, 3_200_001] {
            assert!(
                matches!(resampler(rate), Err(Error::SampleRate(r)) if r == rate),
                "{rate}"
            );
        }
    }

    #[test]
    fn ppm_registers_and_crystals_match_the_vectors() {
        for (ppm, word, bytes, rtl, tuner) in [
            (0, 0, [0x00, 0x00], 28_800_000, 16_000_000),
            (1, -16, [0xf0, 0x3f], 28_800_029, 16_000_016),
            (-1, 16, [0x10, 0x00], 28_799_971, 15_999_984),
            (50, -838, [0xba, 0x3c], 28_801_440, 16_000_800),
            (-50, 838, [0x46, 0x03], 28_798_560, 15_999_200),
            (100, -1677, [0x73, 0x39], 28_802_880, 16_001_600),
            (-100, 1677, [0x8d, 0x06], 28_797_120, 15_998_400),
            (200, -3355, [0xe5, 0x32], 28_805_760, 16_003_200),
            (-200, 3355, [0x1b, 0x0d], 28_794_240, 15_996_800),
            (488, -8187, [0x05, 0x20], 28_814_054, 16_007_808),
            (-488, 8187, [0xfb, 0x1f], 28_785_946, 15_992_192),
        ] {
            assert_eq!(ppm_word(ppm).unwrap(), word, "{ppm}");
            assert_eq!(ppm_bytes(word), bytes, "{ppm}");
            assert_eq!(corrected(CRYSTAL_HZ, ppm), rtl, "{ppm}");
            assert_eq!(corrected(16_000_000, ppm), tuner, "{ppm}");
        }
    }

    #[test]
    fn ppm_past_the_register_is_refused() {
        for ppm in [489, -489, 1000] {
            assert!(matches!(
                ppm_word(ppm),
                Err(Error::Invalid(Invalid::Ppm(p))) if p == ppm
            ));
        }
    }

    #[test]
    fn baseband_init_writes_in_the_documented_order() {
        let fake = Fake::default();
        init_baseband(&Chip::new(fake.clone())).unwrap();
        assert_eq!(
            fake.block_writes(),
            [
                (0x2000, vec![0x09]),
                (0x2158, vec![0x00, 0x02]),
                (0x2148, vec![0x10, 0x02]),
                (0x300b, vec![0x22]),
                (0x3000, vec![0xe8]),
            ]
        );
        let mut expected: Vec<(u8, u8, Vec<u8>)> = vec![
            (1, 0x01, vec![0x14]),
            (1, 0x01, vec![0x10]),
            (1, 0x15, vec![0x00]),
            (1, 0x16, vec![0x00, 0x00]),
        ];
        expected.extend((0x16..=0x1a).map(|reg| (1, reg, vec![0x00])));
        expected.extend(
            (0x1c..)
                .zip(fir_bytes(&FIR))
                .map(|(reg, byte)| (1, reg, vec![byte])),
        );
        expected.extend([
            (0, 0x19, vec![0x05]),
            (1, 0x93, vec![0xf0]),
            (1, 0x94, vec![0x0f]),
            (1, 0x11, vec![0x00]),
            (1, 0x04, vec![0x00]),
            (0, 0x61, vec![0x60]),
            (0, 0x06, vec![0x80]),
            (1, 0xb1, vec![0x1b]),
            (0, 0x0d, vec![0x83]),
        ]);
        assert_eq!(fake.demod_writes(), expected);
    }

    #[test]
    fn the_counter_mode_toggles_page_zero_register_0x19() {
        let fake = Fake::default();
        let chip = Chip::new(fake.clone());
        set_counter(&chip, true).unwrap();
        set_counter(&chip, false).unwrap();
        assert_eq!(
            fake.demod_writes(),
            [(0, 0x19, vec![0x03]), (0, 0x19, vec![0x05])]
        );
    }
}
