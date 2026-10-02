use sdrmm_device::DeviceError;
use serde::Deserialize;

pub(crate) const PROTOCOL: u32 = 6;
pub(crate) const MIN_SAMPLES: u32 = 256;
pub(crate) const FULL_SCALE: f32 = 512.0;
const RATE_INDEX: [u32; 7] = [
    80_000_000, 40_000_000, 20_000_000, 10_000_000, 8_000_000, 4_000_000, 16_000_000,
];
const LEGACY_ESP32_RATES: [u32; 3] = [80_000_000, 40_000_000, 16_000_000];
const LEGACY_RANGE_MHZ: (u32, u32) = (2412, 2484);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) family: String,
    pub(crate) max_samples: u32,
}

impl Identity {
    pub(crate) fn parse(line: &str) -> Result<Self, DeviceError> {
        let unsupported = || DeviceError::Unsupported(format!("not ESP-SDR firmware: {line:?}"));
        let mut words = line.split(' ');
        let family = words
            .next()
            .and_then(|word| word.strip_suffix("SDR"))
            .filter(|family| !family.is_empty())
            .ok_or_else(unsupported)?;
        let protocol: u32 = words
            .next()
            .and_then(|w| w.parse().ok())
            .ok_or_else(unsupported)?;
        if protocol != PROTOCOL {
            return Err(DeviceError::Unsupported(format!(
                "ESP-SDR protocol {protocol}, this build speaks {PROTOCOL}"
            )));
        }
        if words.next() != Some("burst") {
            return Err(unsupported());
        }
        let max_samples: u32 = words
            .next()
            .and_then(|w| w.parse().ok())
            .ok_or_else(unsupported)?;
        if max_samples < MIN_SAMPLES || words.next().is_some() {
            return Err(unsupported());
        }
        Ok(Self {
            family: family.to_string(),
            max_samples,
        })
    }

    pub(crate) fn chip(&self) -> String {
        if self.family == "ESP32" {
            self.family.clone()
        } else {
            format!("ESP32-{}", self.family)
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Features(Vec<String>);

impl Features {
    pub(crate) fn parse(line: &str) -> Result<Self, DeviceError> {
        let rest = line
            .strip_prefix("CAPS")
            .ok_or_else(|| DeviceError::Io(format!("unexpected CAPS reply {line:?}")))?;
        Ok(Self(rest.split_whitespace().map(str::to_string).collect()))
    }

    pub(crate) fn has(&self, name: &str) -> bool {
        self.0.iter().any(|feature| feature == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Limits {
    pub(crate) gain_max: u32,
    pub(crate) bandwidth_mhz: Option<(u32, u32)>,
    pub(crate) rates: Vec<u32>,
    pub(crate) bits: Vec<Bits>,
}

#[derive(Deserialize)]
struct LimitsWire {
    gain: [u32; 3],
    bandwidth: Option<[u32; 4]>,
    rates: Vec<u32>,
    bits: Vec<u32>,
}

impl Limits {
    pub(crate) fn parse(line: &str) -> Result<Self, DeviceError> {
        let bad = |why: String| DeviceError::Io(format!("bad LIMITS reply {line:?}: {why}"));
        let json = line
            .strip_prefix("LIMITS ")
            .ok_or_else(|| bad("no LIMITS prefix".into()))?;
        let wire: LimitsWire = serde_json::from_str(json).map_err(|e| bad(e.to_string()))?;
        let rates: Vec<u32> = wire
            .rates
            .into_iter()
            .filter(|rate| rate_index(*rate).is_some())
            .collect();
        let bits: Vec<Bits> = wire.bits.into_iter().filter_map(Bits::from_width).collect();
        if rates.is_empty() || bits.is_empty() {
            return Err(bad("no usable rate or bit depth".into()));
        }
        Ok(Self {
            gain_max: wire.gain[1],
            bandwidth_mhz: wire.bandwidth.map(|[min, max, _, _]| (min, max)),
            rates,
            bits,
        })
    }

    pub(crate) fn legacy(gain_max: u32) -> Self {
        Self {
            gain_max,
            bandwidth_mhz: None,
            rates: LEGACY_ESP32_RATES.to_vec(),
            bits: vec![Bits::Eight, Bits::Ten],
        }
    }
}

pub(crate) fn parse_range(line: &str) -> Result<(u32, u32), DeviceError> {
    let bad = || DeviceError::Io(format!("bad RANGE reply {line:?}"));
    let mut words = line.strip_prefix("RANGE ").ok_or_else(bad)?.split(' ');
    let mut number = || {
        words
            .next()
            .and_then(|w| w.parse::<u32>().ok())
            .ok_or_else(bad)
    };
    let (min, max) = (number()?, number()?);
    if min >= max {
        return Err(bad());
    }
    Ok((min, max))
}

pub(crate) const fn legacy_range() -> (u32, u32) {
    LEGACY_RANGE_MHZ
}

pub(crate) fn parse_gain_max(line: &str) -> Result<u32, DeviceError> {
    line.strip_prefix("GAIN ")
        .and_then(|rest| rest.split(' ').nth(3))
        .and_then(|word| word.parse().ok())
        .ok_or_else(|| DeviceError::Io(format!("bad GAIN reply {line:?}")))
}

pub(crate) fn rate_index(rate: u32) -> Option<usize> {
    RATE_INDEX.iter().position(|known| *known == rate)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Bits {
    Eight,
    Ten,
}

impl Bits {
    pub(crate) fn from_width(width: u32) -> Option<Self> {
        match width {
            8 => Some(Self::Eight),
            10 => Some(Self::Ten),
            _ => None,
        }
    }

    pub(crate) const fn width(self) -> u32 {
        match self {
            Self::Eight => 8,
            Self::Ten => 10,
        }
    }

    pub(crate) const fn command(self) -> &'static str {
        match self {
            Self::Eight => "CAP16",
            Self::Ten => "CAP20",
        }
    }

    pub(crate) const fn payload_bytes(self, samples: u32) -> usize {
        (samples as usize * self.width() as usize * 2).div_ceil(8)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DataHeader {
    pub(crate) samples: u32,
    pub(crate) crc: u32,
    pub(crate) capture_us: u32,
}

impl DataHeader {
    pub(crate) fn parse(line: &str) -> Result<Self, String> {
        let mut words = line
            .strip_prefix("DATA ")
            .ok_or_else(|| format!("expected DATA, got {line:?}"))?
            .split(' ');
        let bad = || format!("bad DATA header {line:?}");
        let samples = words.next().and_then(|w| w.parse().ok()).ok_or_else(bad)?;
        let crc = words
            .next()
            .and_then(|w| u32::from_str_radix(w, 16).ok())
            .ok_or_else(bad)?;
        let capture_us = words.next().and_then(|w| w.parse().ok()).ok_or_else(bad)?;
        Ok(Self {
            samples,
            crc,
            capture_us,
        })
    }
}

const CRC_TABLE: [u32; 256] = crc_table();

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 == 1 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
}

pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(u32::MAX, |crc, byte| {
        CRC_TABLE[((crc ^ u32::from(*byte)) & 0xFF) as usize] ^ (crc >> 8)
    })
}

pub(crate) fn unpack(bits: Bits, payload: &[u8], out: &mut [u8]) -> usize {
    match bits {
        Bits::Eight => unpack_eight(payload, out),
        Bits::Ten => unpack_ten(payload, out),
    }
}

fn store(out: &mut [u8], index: usize, i: i16, q: i16) {
    let at = index * 4;
    out[at..at + 2].copy_from_slice(&i.to_le_bytes());
    out[at + 2..at + 4].copy_from_slice(&q.wrapping_neg().to_le_bytes());
}

fn unpack_eight(payload: &[u8], out: &mut [u8]) -> usize {
    let pairs = payload.as_chunks::<2>().0;
    let count = pairs.len().min(out.len() / 4);
    for (index, [i, q]) in pairs.iter().take(count).enumerate() {
        store(
            out,
            index,
            i16::from(i.cast_signed()) * 4,
            i16::from(q.cast_signed()) * 4,
        );
    }
    count
}

fn ten_bit(word: u32) -> i16 {
    (((word & 0x3FF) as i16) << 6) >> 6
}

fn unpack_ten(payload: &[u8], out: &mut [u8]) -> usize {
    let count = (payload.len() * 8 / 20).min(out.len() / 4);
    for index in 0..count {
        let bit = index * 20;
        let byte = bit / 8;
        let mut word = 0u32;
        for (shift, value) in payload[byte..payload.len().min(byte + 4)]
            .iter()
            .enumerate()
        {
            word |= u32::from(*value) << (8 * shift);
        }
        let word = word >> (bit % 8);
        store(out, index, ten_bit(word), ten_bit(word >> 10));
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_names_the_chip_and_burst_size() {
        let id = Identity::parse("ESP32SDR 6 burst 16380").expect("valid");
        assert_eq!(id.max_samples, 16380);
        assert_eq!(id.chip(), "ESP32");
        let c6 = Identity::parse("C6SDR 6 burst 8192").expect("valid");
        assert_eq!(c6.chip(), "ESP32-C6");
    }

    #[test]
    fn other_firmware_is_refused_by_name() {
        assert!(Identity::parse("NanoVNA").is_err());
        assert!(Identity::parse("ESP32SDR 5 burst 16380").is_err());
        assert!(Identity::parse("ESP32SDR 6 stream 16380").is_err());
        assert!(Identity::parse("ESP32SDR 6 burst 12").is_err());
    }

    #[test]
    fn caps_are_a_word_list() {
        let caps = Features::parse("CAPS SPEC GAIN HWAGC TUNEEXT").expect("valid");
        assert!(caps.has("HWAGC"));
        assert!(!caps.has("SPECN"));
        assert!(Features::parse("ERR busy").is_err());
    }

    #[test]
    fn limits_parse_the_esp32_reply() {
        let limits = Limits::parse(
            r#"LIMITS {"gain":[0,72,1],"bandwidth":[12,67,1,0],"rates":[80000000,40000000,16000000],"bits":[8,10]}"#,
        )
        .expect("valid");
        assert_eq!(limits.gain_max, 72);
        assert_eq!(limits.bandwidth_mhz, Some((12, 67)));
        assert_eq!(limits.rates, [80_000_000, 40_000_000, 16_000_000]);
        assert_eq!(limits.bits, [Bits::Eight, Bits::Ten]);
    }

    #[test]
    fn limits_without_a_filter_or_with_unknown_rates_still_parse() {
        let limits = Limits::parse(
            r#"LIMITS {"gain":[0,60,1],"bandwidth":null,"rates":[80000000,1234],"bits":[8,12]}"#,
        )
        .expect("valid");
        assert_eq!(limits.bandwidth_mhz, None);
        assert_eq!(limits.rates, [80_000_000]);
        assert_eq!(limits.bits, [Bits::Eight]);
        assert!(
            Limits::parse(r#"LIMITS {"gain":[0,60,1],"bandwidth":null,"rates":[1],"bits":[8]}"#)
                .is_err()
        );
    }

    #[test]
    fn range_and_gain_replies_parse() {
        assert_eq!(parse_range("RANGE 100 6000 1").expect("valid"), (100, 6000));
        assert!(parse_range("RANGE 6000 100 1").is_err());
        assert_eq!(
            parse_gain_max("GAIN HARDWARE -1 0 72 0").expect("valid"),
            72
        );
    }

    #[test]
    fn rate_indices_follow_the_firmware_table() {
        assert_eq!(rate_index(80_000_000), Some(0));
        assert_eq!(rate_index(40_000_000), Some(1));
        assert_eq!(rate_index(16_000_000), Some(6));
        assert_eq!(rate_index(2_000_000), None);
    }

    #[test]
    fn a_data_header_carries_count_crc_and_duration() {
        let header = DataHeader::parse("DATA 16380 83499450 214").expect("valid");
        assert_eq!(header.samples, 16380);
        assert_eq!(header.crc, 0x8349_9450);
        assert_eq!(header.capture_us, 214);
        assert!(DataHeader::parse("ERR capture_timeout").is_err());
        assert!(DataHeader::parse("DATA 12 zz 3").is_err());
    }

    #[test]
    fn crc_matches_the_ieee_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(&[]), 0);
    }

    #[test]
    fn payload_sizes_round_up_to_whole_bytes() {
        assert_eq!(Bits::Eight.payload_bytes(16380), 32760);
        assert_eq!(Bits::Ten.payload_bytes(16380), 40950);
        assert_eq!(Bits::Ten.payload_bytes(3), 8);
    }

    fn decoded(out: &[u8], index: usize) -> (i16, i16) {
        let at = index * 4;
        (
            i16::from_le_bytes([out[at], out[at + 1]]),
            i16::from_le_bytes([out[at + 2], out[at + 3]]),
        )
    }

    #[test]
    fn eight_bit_pairs_scale_to_ten_bits_with_q_mirrored() {
        let mut out = [0u8; 8];
        assert_eq!(unpack(Bits::Eight, &[0x7F, 0x80, 0xFF, 0x01], &mut out), 2);
        assert_eq!(decoded(&out, 0), (508, 512));
        assert_eq!(decoded(&out, 1), (-4, -4));
    }

    fn pack_ten(samples: &[(i16, i16)]) -> Vec<u8> {
        let mut bits = 0u128;
        for (n, (i, q)) in samples.iter().enumerate() {
            let word = (*i as u128 & 0x3FF) | ((*q as u128 & 0x3FF) << 10);
            bits |= word << (20 * n);
        }
        bits.to_le_bytes()[..Bits::Ten.payload_bytes(samples.len() as u32)].to_vec()
    }

    #[test]
    fn ten_bit_words_unpack_across_byte_boundaries() {
        let samples = [(511, -512), (-1, 1), (100, -100)];
        let payload = pack_ten(&samples);
        let mut out = [0u8; 12];
        assert_eq!(unpack(Bits::Ten, &payload, &mut out), 3);
        for (index, (i, q)) in samples.iter().enumerate() {
            assert_eq!(decoded(&out, index), (*i, -*q));
        }
    }
}
