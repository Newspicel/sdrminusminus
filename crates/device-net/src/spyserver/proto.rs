use sdrmm_device::DeviceError;

pub(crate) const DEFAULT_PORT: u16 = 5555;

pub(crate) const PROTOCOL_VERSION: u32 = (2 << 24) | 1700;
fn major(version: u32) -> u32 {
    version >> 24
}

const CMD_HELLO: u32 = 0;
const CMD_SET_SETTING: u32 = 2;

pub(crate) const MAX_BODY: u32 = 1 << 20;

pub(crate) const HEADER_LEN: usize = 20;
pub(crate) const DEVICE_INFO_LEN: usize = 48;
pub(crate) const CLIENT_SYNC_LEN: usize = 36;

const STREAM_MODE_IQ_ONLY: u32 = 0x01;
pub(crate) const STREAM_TYPE_IQ: u32 = 1;

pub(crate) const MSG_DEVICE_INFO: u16 = 0;
pub(crate) const MSG_CLIENT_SYNC: u16 = 1;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum IqFormat {
    Uint8,
    #[default]
    Int16,
    Float32,
}

impl IqFormat {
    pub(crate) fn code(self) -> u32 {
        match self {
            Self::Uint8 => 1,
            Self::Int16 => 2,
            Self::Float32 => 4,
        }
    }

    pub(crate) fn message_type(self) -> u16 {
        match self {
            Self::Uint8 => 100,
            Self::Int16 => 101,
            Self::Float32 => 103,
        }
    }

    pub(crate) fn from_message_type(kind: u16) -> Option<Self> {
        [Self::Uint8, Self::Int16, Self::Float32]
            .into_iter()
            .find(|format| format.message_type() == kind)
    }

    pub(crate) fn sample_bytes(self) -> usize {
        match self {
            Self::Uint8 => 2,
            Self::Int16 => 4,
            Self::Float32 => 8,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Uint8 => "uint8",
            Self::Int16 => "int16",
            Self::Float32 => "float32",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        [Self::Uint8, Self::Int16, Self::Float32]
            .into_iter()
            .find(|format| format.name() == name)
    }

    pub(crate) fn forced(code: u32) -> Result<Option<Self>, DeviceError> {
        match code {
            0 => Ok(None),
            1 => Ok(Some(Self::Uint8)),
            2 => Ok(Some(Self::Int16)),
            4 => Ok(Some(Self::Float32)),
            other => Err(DeviceError::Unsupported(format!(
                "this server forces IQ format {other}, which sdr-- cannot decode"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Setting {
    StreamingMode,
    Gain,
    IqFormat,
    IqFrequency,
    IqDecimation,
    IqDigitalGain,
    StreamingEnabled,
}

impl Setting {
    fn code(self) -> u32 {
        match self {
            Self::StreamingMode => 0,
            Self::StreamingEnabled => 1,
            Self::Gain => 2,
            Self::IqFormat => 100,
            Self::IqFrequency => 101,
            Self::IqDecimation => 102,
            Self::IqDigitalGain => 103,
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::IqFormat => 0,
            Self::IqDecimation => 1,
            Self::IqFrequency => 2,
            Self::StreamingMode => 3,
            Self::Gain => 4,
            Self::IqDigitalGain => 5,
            Self::StreamingEnabled => 6,
        }
    }
}

pub(crate) fn ordered(mut batch: Vec<(Setting, u32)>) -> Vec<(Setting, u32)> {
    batch.sort_by_key(|(setting, _)| setting.rank());
    batch
}

pub(crate) fn hello(client: &str) -> Vec<u8> {
    let body_size = 4 + client.len();
    let mut frame = Vec::with_capacity(8 + body_size);
    frame.extend_from_slice(&CMD_HELLO.to_le_bytes());
    frame.extend_from_slice(&(body_size as u32).to_le_bytes());
    frame.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    frame.extend_from_slice(client.as_bytes());
    frame
}

pub(crate) fn setting(setting: Setting, value: u32) -> [u8; 16] {
    let mut frame = [0u8; 16];
    frame[..4].copy_from_slice(&CMD_SET_SETTING.to_le_bytes());
    frame[4..8].copy_from_slice(&8u32.to_le_bytes());
    frame[8..12].copy_from_slice(&setting.code().to_le_bytes());
    frame[12..].copy_from_slice(&value.to_le_bytes());
    frame
}

pub(crate) const fn iq_only() -> (Setting, u32) {
    (Setting::StreamingMode, STREAM_MODE_IQ_ONLY)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MessageHeader {
    pub(crate) kind: u16,
    pub(crate) flags: u16,
    pub(crate) stream_type: u32,
    pub(crate) body_size: u32,
}

impl MessageHeader {
    pub(crate) fn parse(bytes: &[u8; HEADER_LEN]) -> Result<Self, DeviceError> {
        let word = |at: usize| {
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        let protocol = word(0);
        if major(protocol) != major(PROTOCOL_VERSION) {
            return Err(DeviceError::Io(format!(
                "SpyServer protocol {}.{}.{} is not one sdr-- speaks (expected major {})",
                major(protocol),
                (protocol >> 16) & 0xFF,
                protocol & 0xFFFF,
                major(PROTOCOL_VERSION)
            )));
        }
        let message_type = word(4);
        let body_size = word(16);
        if body_size > MAX_BODY {
            return Err(DeviceError::Io(format!(
                "SpyServer message body of {body_size} bytes exceeds the protocol's {MAX_BODY}"
            )));
        }
        Ok(Self {
            kind: (message_type & 0xFFFF) as u16,
            flags: (message_type >> 16) as u16,
            stream_type: word(8),
            body_size,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DeviceInfo {
    pub(crate) device_type: u32,
    pub(crate) serial: u32,
    pub(crate) max_sample_rate: u32,
    pub(crate) decimation_stages: u32,
    pub(crate) max_gain_index: u32,
    pub(crate) min_frequency: u32,
    pub(crate) max_frequency: u32,
    pub(crate) min_decimation: u32,
    pub(crate) forced_iq_format: u32,
}

impl DeviceInfo {
    pub(crate) fn model(self) -> &'static str {
        match self.device_type {
            1 => "Airspy",
            2 => "Airspy HF+",
            3 => "RTL-SDR",
            _ => "receiver",
        }
    }

    pub(crate) fn airspy_one(self) -> bool {
        self.device_type == 1
    }

    pub(crate) fn parse(body: &[u8]) -> Result<Self, DeviceError> {
        if body.len() < DEVICE_INFO_LEN {
            return Err(DeviceError::Io(format!(
                "SpyServer device info is {} bytes, expected at least {DEVICE_INFO_LEN}",
                body.len()
            )));
        }
        let word = |n: usize| {
            let at = n * 4;
            u32::from_le_bytes([body[at], body[at + 1], body[at + 2], body[at + 3]])
        };
        Ok(Self {
            device_type: word(0),
            serial: word(1),
            max_sample_rate: word(2),
            decimation_stages: word(4),
            max_gain_index: word(6),
            min_frequency: word(7),
            max_frequency: word(8),
            min_decimation: word(10),
            forced_iq_format: word(11),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ClientSync {
    pub(crate) can_control: bool,
    pub(crate) gain: u32,
    pub(crate) iq_center_hz: u32,
    pub(crate) min_iq_center_hz: u32,
    pub(crate) max_iq_center_hz: u32,
}

impl ClientSync {
    pub(crate) fn parse(body: &[u8]) -> Result<Self, DeviceError> {
        if body.len() < CLIENT_SYNC_LEN {
            return Err(DeviceError::Io(format!(
                "SpyServer client sync is {} bytes, expected at least {CLIENT_SYNC_LEN}",
                body.len()
            )));
        }
        let word = |n: usize| {
            let at = n * 4;
            u32::from_le_bytes([body[at], body[at + 1], body[at + 2], body[at + 3]])
        };
        Ok(Self {
            can_control: word(0) != 0,
            gain: word(1),
            iq_center_hz: word(3),
            min_iq_center_hz: word(5),
            max_iq_center_hz: word(6),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn header_bytes(
        protocol: u32,
        kind: u16,
        flags: u16,
        stream_type: u32,
        sequence: u32,
        body_size: u32,
    ) -> [u8; HEADER_LEN] {
        let mut bytes = [0u8; HEADER_LEN];
        bytes[..4].copy_from_slice(&protocol.to_le_bytes());
        bytes[4..8].copy_from_slice(&(u32::from(kind) | (u32::from(flags) << 16)).to_le_bytes());
        bytes[8..12].copy_from_slice(&stream_type.to_le_bytes());
        bytes[12..16].copy_from_slice(&sequence.to_le_bytes());
        bytes[16..].copy_from_slice(&body_size.to_le_bytes());
        bytes
    }

    #[test]
    fn a_hello_carries_the_version_then_the_client_name() {
        let frame = hello("sdr--");
        assert_eq!(&frame[..4], &0u32.to_le_bytes(), "CMD_HELLO");
        assert_eq!(
            &frame[4..8],
            &9u32.to_le_bytes(),
            "four bytes plus the name"
        );
        assert_eq!(&frame[8..12], &PROTOCOL_VERSION.to_le_bytes());
        assert_eq!(&frame[12..], b"sdr--");
    }

    #[test]
    fn a_setting_is_its_id_and_value_little_endian() {
        let frame = setting(Setting::IqFrequency, 100_000_000);
        assert_eq!(&frame[..4], &2u32.to_le_bytes(), "CMD_SET_SETTING");
        assert_eq!(&frame[4..8], &8u32.to_le_bytes());
        assert_eq!(&frame[8..12], &101u32.to_le_bytes());
        assert_eq!(&frame[12..], &100_000_000u32.to_le_bytes());
    }

    #[test]
    fn a_batch_sets_the_stream_up_before_it_turns_it_on() {
        let batch = ordered(vec![
            (Setting::StreamingEnabled, 1),
            (Setting::IqFrequency, 100_000_000),
            (Setting::IqDigitalGain, 6),
            (Setting::IqFormat, 2),
            (Setting::IqDecimation, 1),
        ]);
        assert_eq!(
            batch.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            vec![
                Setting::IqFormat,
                Setting::IqDecimation,
                Setting::IqFrequency,
                Setting::IqDigitalGain,
                Setting::StreamingEnabled,
            ]
        );
    }

    #[test]
    fn a_header_splits_the_type_word_into_kind_and_gain_flags() {
        let header = MessageHeader::parse(&header_bytes(
            PROTOCOL_VERSION,
            101,
            6,
            STREAM_TYPE_IQ,
            7,
            64,
        ))
        .expect("a real header");
        assert_eq!(header.kind, 101);
        assert_eq!(
            header.flags, 6,
            "the digital gain the server applied, in dB"
        );
        assert_eq!(header.stream_type, STREAM_TYPE_IQ);
        assert_eq!(header.body_size, 64);
    }

    #[test]
    fn a_header_from_another_protocol_or_with_an_impossible_body_is_refused() {
        let wrong_version = MessageHeader::parse(&header_bytes(3 << 24, 101, 0, 1, 0, 64));
        assert!(
            wrong_version.is_err_and(|e| e.to_string().contains("is not one sdr-- speaks")),
            "a major version this cannot frame must be named"
        );
        let huge =
            MessageHeader::parse(&header_bytes(PROTOCOL_VERSION, 101, 0, 1, 0, MAX_BODY + 1));
        assert!(huge.is_err_and(|e| e.to_string().contains("exceeds")));
    }

    fn words(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    #[test]
    fn device_info_reads_the_fields_this_backend_acts_on() {
        let info = DeviceInfo::parse(&words(&[
            3,
            0xDEAD,
            10_000_000,
            8_000_000,
            4,
            2,
            28,
            24_000_000,
            1_766_000_000,
            8,
            1,
            0,
        ]))
        .expect("device info");
        assert_eq!(info.model(), "RTL-SDR");
        assert_eq!(info.max_sample_rate, 10_000_000);
        assert_eq!(info.decimation_stages, 4);
        assert_eq!(info.max_gain_index, 28);
        assert_eq!(info.min_frequency, 24_000_000);
        assert_eq!(info.max_frequency, 1_766_000_000);
        assert_eq!(info.min_decimation, 1);
        assert!(!info.airspy_one());
    }

    #[test]
    fn client_sync_reads_the_control_flag_and_the_iq_window() {
        let sync = ClientSync::parse(&words(&[
            0,
            12,
            100_000_000,
            99_000_000,
            0,
            90_000_000,
            110_000_000,
            0,
            0,
        ]))
        .expect("client sync");
        assert!(!sync.can_control, "a locked server");
        assert_eq!(sync.gain, 12);
        assert_eq!(sync.iq_center_hz, 99_000_000);
        assert_eq!(sync.min_iq_center_hz, 90_000_000);
        assert_eq!(sync.max_iq_center_hz, 110_000_000);
    }

    #[test]
    fn a_truncated_body_is_refused_rather_than_read_short() {
        assert!(DeviceInfo::parse(&words(&[3, 0, 0])).is_err());
        assert!(ClientSync::parse(&words(&[1, 0])).is_err());
    }

    #[test]
    fn a_forced_format_is_honoured_and_an_undecodable_one_is_refused_at_open() {
        assert_eq!(IqFormat::forced(0).expect("nothing forced"), None);
        assert_eq!(IqFormat::forced(2).expect("int16"), Some(IqFormat::Int16));
        for code in [3, 5] {
            assert!(IqFormat::forced(code).is_err(), "format {code}");
        }
    }

    #[test]
    fn formats_round_trip_through_the_name_they_are_offered_under() {
        for format in [IqFormat::Uint8, IqFormat::Int16, IqFormat::Float32] {
            assert_eq!(IqFormat::from_name(format.name()), Some(format));
        }
        assert_eq!(IqFormat::from_name("int24"), None);
    }
}
