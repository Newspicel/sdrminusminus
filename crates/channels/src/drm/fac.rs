use super::{
    bits::crc,
    coding::{Qam, R1_2, R1_3, R1_4, R2_3, Rate},
    mode::Robustness,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FacService {
    pub id: u32,
    pub short_id: u8,
    pub audio_ca: bool,
    pub language: u8,
    pub data: bool,
    pub descriptor: u8,
    pub data_ca: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fac {
    pub enhancement: bool,
    pub identity: u8,
    pub plus: bool,
    pub occupancy: u8,
    pub short_interleave: bool,
    pub msc: Qam,
    pub sdc_robust: bool,
    pub services: u8,
    pub reconfiguration: u8,
    pub toggle: bool,
    pub service: [Option<FacService>; 2],
}

fn take(bits: &[bool], at: &mut usize, width: usize) -> u32 {
    let value = bits[*at..*at + width]
        .iter()
        .fold(0u32, |value, &bit| value << 1 | u32::from(bit));
    *at += width;
    value
}

fn put(bits: &mut Vec<bool>, value: u32, width: usize) {
    bits.extend((0..width).rev().map(|shift| value >> shift & 1 == 1));
}

impl FacService {
    fn read(bits: &[bool], at: &mut usize) -> Self {
        let id = take(bits, at, 24);
        let short_id = take(bits, at, 2) as u8;
        let audio_ca = take(bits, at, 1) == 1;
        let language = take(bits, at, 4) as u8;
        let data = take(bits, at, 1) == 1;
        let descriptor = take(bits, at, 5) as u8;
        let data_ca = take(bits, at, 1) == 1;
        *at += 6;
        Self {
            id,
            short_id,
            audio_ca,
            language,
            data,
            descriptor,
            data_ca,
        }
    }

    fn write(&self, bits: &mut Vec<bool>) {
        put(bits, self.id, 24);
        put(bits, u32::from(self.short_id), 2);
        put(bits, u32::from(self.audio_ca), 1);
        put(bits, u32::from(self.language), 4);
        put(bits, u32::from(self.data), 1);
        put(bits, u32::from(self.descriptor), 5);
        put(bits, u32::from(self.data_ca), 1);
        put(bits, 0, 6);
    }
}

#[must_use]
pub const fn fac_bits(plus: bool) -> usize {
    if plus { 116 } else { 72 }
}

#[must_use]
pub const fn fac_rate(plus: bool) -> Rate {
    if plus { R1_4 } else { super::coding::R3_5 }
}

fn check(bits: &[bool], plus: bool) -> u32 {
    let data = fac_bits(plus) - 8;
    let padding = if plus { 4 } else { 0 };
    crc(
        0x1D,
        8,
        bits[..data]
            .iter()
            .copied()
            .chain(std::iter::repeat_n(false, padding)),
    )
}

impl Fac {
    #[must_use]
    pub fn parse(bits: &[bool], plus: bool) -> Option<Self> {
        if bits.len() != fac_bits(plus) {
            return None;
        }
        let data = fac_bits(plus) - 8;
        let mut at = data;
        if take(bits, &mut at, 8) != check(bits, plus) {
            return None;
        }
        let mut at = 0;
        let enhancement = take(bits, &mut at, 1) == 1;
        let identity = take(bits, &mut at, 2) as u8;
        let rm = take(bits, &mut at, 1) == 1;
        if rm != plus {
            return None;
        }
        let occupancy = take(bits, &mut at, 3) as u8;
        let short_interleave = take(bits, &mut at, 1) == 1;
        let msc = match (plus, take(bits, &mut at, 2)) {
            (false, 0) => Qam::Q64,
            (false, 3) | (true, 0) => Qam::Q16,
            (true, 3) => Qam::Q4,
            _ => return None,
        };
        let sdc_robust = take(bits, &mut at, 1) == 1;
        let services = take(bits, &mut at, 4) as u8;
        let reconfiguration = take(bits, &mut at, 3) as u8;
        let toggle = take(bits, &mut at, 1) == 1;
        at += 1;
        let first = FacService::read(bits, &mut at);
        let second = plus.then(|| FacService::read(bits, &mut at));
        Some(Self {
            enhancement,
            identity,
            plus,
            occupancy,
            short_interleave,
            msc,
            sdc_robust,
            services,
            reconfiguration,
            toggle,
            service: [Some(first), second],
        })
    }

    #[must_use]
    pub fn encode(&self) -> Vec<bool> {
        let mut bits = Vec::with_capacity(fac_bits(self.plus));
        put(&mut bits, u32::from(self.enhancement), 1);
        put(&mut bits, u32::from(self.identity), 2);
        put(&mut bits, u32::from(self.plus), 1);
        put(&mut bits, u32::from(self.occupancy), 3);
        put(&mut bits, u32::from(self.short_interleave), 1);
        let msc = match (self.plus, self.msc) {
            (false, Qam::Q64) | (true, Qam::Q16) => 0,
            _ => 3,
        };
        put(&mut bits, msc, 2);
        put(&mut bits, u32::from(self.sdc_robust), 1);
        put(&mut bits, u32::from(self.services), 4);
        put(&mut bits, u32::from(self.reconfiguration), 3);
        put(&mut bits, u32::from(self.toggle), 1);
        put(&mut bits, 0, 1);
        for service in self.service.iter().take(if self.plus { 2 } else { 1 }) {
            service.unwrap_or_default().write(&mut bits);
        }
        bits.resize(fac_bits(self.plus), false);
        let value = check(&bits, self.plus);
        let data = fac_bits(self.plus) - 8;
        bits.truncate(data);
        put(&mut bits, value, 8);
        bits
    }

    #[must_use]
    pub const fn frame(&self) -> usize {
        match (self.plus, self.identity, self.toggle) {
            (_, 0 | 3, _) => 0,
            (false, 1, _) | (true, 1, true) => 1,
            (true, 1, false) | (false, 2, _) => 2,
            _ => 3,
        }
    }

    #[must_use]
    pub const fn identity_for(plus: bool, frame: usize) -> (u8, bool) {
        match (plus, frame) {
            (_, 0) => (0, false),
            (false, 1) => (1, false),
            (false, _) => (2, false),
            (true, 1) => (1, true),
            (true, 2) => (1, false),
            (true, _) => (2, true),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub const fn service_counts(&self) -> (u8, u8) {
        match self.services {
            0b0000 => (4, 0),
            0b0001 => (0, 1),
            0b0010 => (0, 2),
            0b0011 => (0, 3),
            0b0100 => (1, 0),
            0b0101 => (1, 1),
            0b0110 => (1, 2),
            0b0111 => (1, 3),
            0b1000 => (2, 0),
            0b1001 => (2, 1),
            0b1010 => (2, 2),
            0b1100 => (3, 0),
            0b1101 => (3, 1),
            0b1111 => (0, 4),
            _ => (0, 0),
        }
    }

    #[must_use]
    pub fn sdc_coding(&self) -> (Qam, &'static [Rate]) {
        match (self.plus, self.sdc_robust) {
            (false, false) => (Qam::Q16, &[R1_3, R2_3]),
            (false, true) | (true, false) => (Qam::Q4, &[R1_2]),
            (true, true) => (Qam::Q4, &[R1_4]),
        }
    }

    #[must_use]
    pub fn interleave_depth(&self, mode: Robustness) -> usize {
        if self.short_interleave && !self.plus {
            1
        } else {
            mode.interleave_depth()
        }
    }
}

#[must_use]
pub fn language(code: u8) -> &'static str {
    match code {
        1 => "Arabic",
        2 => "Bengali",
        3 => "Chinese",
        4 => "Dutch",
        5 => "English",
        6 => "French",
        7 => "German",
        8 => "Hindi",
        9 => "Japanese",
        10 => "Javanese",
        11 => "Korean",
        12 => "Portuguese",
        13 => "Russian",
        14 => "Spanish",
        15 => "Other",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(plus: bool) -> Fac {
        let (identity, toggle) = Fac::identity_for(plus, 2);
        Fac {
            enhancement: false,
            identity,
            plus,
            occupancy: if plus { 0 } else { 3 },
            short_interleave: !plus,
            msc: Qam::Q16,
            sdc_robust: true,
            services: 0b0101,
            reconfiguration: 0,
            toggle,
            service: [
                Some(FacService {
                    id: 0xABCDEF,
                    short_id: 1,
                    audio_ca: false,
                    language: 5,
                    data: false,
                    descriptor: 10,
                    data_ca: false,
                }),
                plus.then_some(FacService {
                    id: 0x123456,
                    short_id: 2,
                    data: true,
                    descriptor: 3,
                    ..FacService::default()
                }),
            ],
        }
    }

    #[test]
    fn fac_blocks_round_trip_and_reject_damage() {
        for plus in [false, true] {
            let fac = sample(plus);
            let mut bits = fac.encode();
            assert_eq!(bits.len(), fac_bits(plus));
            assert_eq!(Fac::parse(&bits, plus), Some(fac));
            assert_eq!(fac.frame(), 2);
            assert_eq!(fac.service_counts(), (1, 1));
            bits[30] = !bits[30];
            assert_eq!(Fac::parse(&bits, plus), None);
        }
    }

    #[test]
    fn identities_name_every_frame() {
        for plus in [false, true] {
            for frame in 0..if plus { 4 } else { 3 } {
                let (identity, toggle) = Fac::identity_for(plus, frame);
                let fac = Fac {
                    identity,
                    toggle,
                    ..sample(plus)
                };
                assert_eq!(fac.frame(), frame);
            }
        }
    }
}
