#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Datatype {
    width: Width,
    signed: bool,
    float: bool,
    big_endian: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Width {
    Eight,
    Sixteen,
    ThirtyTwo,
    SixtyFour,
}

impl Width {
    const fn bytes(self) -> usize {
        match self {
            Self::Eight => 1,
            Self::Sixteen => 2,
            Self::ThirtyTwo => 4,
            Self::SixtyFour => 8,
        }
    }
}

impl Datatype {
    #[must_use]
    pub fn parse(datatype: &str) -> Option<Self> {
        let rest = datatype.strip_prefix('c')?;
        let (kind, rest) = rest.split_at_checked(1)?;
        let (float, signed) = match kind {
            "f" => (true, true),
            "i" => (false, true),
            "u" => (false, false),
            _ => return None,
        };
        let (bits, endian) = match rest.find('_') {
            Some(at) => (&rest[..at], &rest[at..]),
            None => (rest, ""),
        };
        let width = match bits {
            "8" => Width::Eight,
            "16" => Width::Sixteen,
            "32" => Width::ThirtyTwo,
            "64" => Width::SixtyFour,
            _ => return None,
        };
        let big_endian = match endian {
            "" | "_le" => false,
            "_be" => true,
            _ => return None,
        };
        if float && matches!(width, Width::Eight | Width::Sixteen) {
            return None;
        }
        if width == Width::SixtyFour && !float {
            return None;
        }
        if width == Width::Eight && big_endian {
            return None;
        }
        Some(Self {
            width,
            signed,
            float,
            big_endian,
        })
    }

    #[must_use]
    pub const fn bytes_per_sample(self) -> usize {
        self.width.bytes() * 2
    }

    #[must_use]
    pub fn sample(self, bytes: &[u8]) -> (f32, f32) {
        let half = self.width.bytes();
        (self.scalar(&bytes[..half]), self.scalar(&bytes[half..]))
    }

    fn scalar(self, bytes: &[u8]) -> f32 {
        match (self.width, self.float, self.signed) {
            (Width::Eight, _, true) => f32::from(bytes[0] as i8) / 128.0,
            (Width::Eight, _, false) => (f32::from(bytes[0]) - 128.0) / 128.0,
            (Width::Sixteen, _, true) => f32::from(self.i16(bytes)) / 32_768.0,
            (Width::Sixteen, _, false) => (f32::from(self.u16(bytes)) - 32_768.0) / 32_768.0,
            (Width::ThirtyTwo, true, _) => f32::from_bits(self.u32(bytes)),
            (Width::ThirtyTwo, false, true) => self.u32(bytes) as i32 as f32 / 2_147_483_648.0,
            (Width::ThirtyTwo, false, false) => {
                (f64::from(self.u32(bytes)) - 2_147_483_648.0) as f32 / 2_147_483_648.0
            }
            (Width::SixtyFour, _, _) => f64::from_bits(self.u64(bytes)) as f32,
        }
    }

    fn u16(self, bytes: &[u8]) -> u16 {
        let raw = [bytes[0], bytes[1]];
        if self.big_endian {
            u16::from_be_bytes(raw)
        } else {
            u16::from_le_bytes(raw)
        }
    }

    fn i16(self, bytes: &[u8]) -> i16 {
        self.u16(bytes) as i16
    }

    fn u32(self, bytes: &[u8]) -> u32 {
        let raw = [bytes[0], bytes[1], bytes[2], bytes[3]];
        if self.big_endian {
            u32::from_be_bytes(raw)
        } else {
            u32::from_le_bytes(raw)
        }
    }

    fn u64(self, bytes: &[u8]) -> u64 {
        let raw = [
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ];
        if self.big_endian {
            u64::from_be_bytes(raw)
        } else {
            u64::from_le_bytes(raw)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_datatypes_sigmf_recordings_arrive_in_are_understood() {
        for (name, stride) in [
            ("cf32_le", 8),
            ("cf32_be", 8),
            ("cf64_le", 16),
            ("ci16_le", 4),
            ("ci16_be", 4),
            ("cu16_le", 4),
            ("ci32_le", 8),
            ("ci8", 2),
            ("cu8", 2),
        ] {
            let datatype = Datatype::parse(name).unwrap_or_else(|| panic!("{name} must parse"));
            assert_eq!(datatype.bytes_per_sample(), stride, "{name}");
        }
    }

    #[test]
    fn real_and_nonsense_datatypes_are_refused() {
        for name in [
            "rf32_le", "cf16_le", "cf8", "ci64_le", "cq32_le", "c32_le", "", "cf32_mid", "ci8_be",
        ] {
            assert!(Datatype::parse(name).is_none(), "{name} must be refused");
        }
    }

    #[test]
    fn full_scale_maps_to_full_scale_in_every_integer_width() {
        let cases: [(&str, Vec<u8>, f32); 4] = [
            ("ci8", vec![0x7F, 0x81], 127.0 / 128.0),
            ("cu8", vec![0xFF, 0x01], 127.0 / 128.0),
            ("ci16_le", vec![0xFF, 0x7F, 0x00, 0x80], 32_767.0 / 32_768.0),
            ("ci16_be", vec![0x7F, 0xFF, 0x80, 0x00], 32_767.0 / 32_768.0),
        ];
        for (name, bytes, want_re) in cases {
            let datatype = Datatype::parse(name).unwrap_or_else(|| panic!("{name} must parse"));
            let (re, im) = datatype.sample(&bytes);
            assert!((re - want_re).abs() < 1e-4, "{name} re {re}");
            assert!(im < -0.9, "{name} im {im}");
        }
    }

    #[test]
    fn floats_cross_over_unchanged_in_either_byte_order() {
        let le = Datatype::parse("cf32_le").expect("cf32_le");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0.25f32.to_le_bytes());
        bytes.extend_from_slice(&(-0.5f32).to_le_bytes());
        assert_eq!(le.sample(&bytes), (0.25, -0.5));

        let be = Datatype::parse("cf32_be").expect("cf32_be");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0.25f32.to_be_bytes());
        bytes.extend_from_slice(&(-0.5f32).to_be_bytes());
        assert_eq!(be.sample(&bytes), (0.25, -0.5));

        let wide = Datatype::parse("cf64_le").expect("cf64_le");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0.25f64.to_le_bytes());
        bytes.extend_from_slice(&(-0.5f64).to_le_bytes());
        assert_eq!(wide.sample(&bytes), (0.25, -0.5));
    }
}
