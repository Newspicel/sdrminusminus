use sdrmm_device::DeviceError;

pub(crate) const DEFAULT_PORT: u16 = 1234;

pub(crate) const GREETING_LEN: usize = 12;
const MAGIC: &[u8] = b"RTL0";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Tuner {
    #[default]
    Unknown,
    E4000,
    Fc0012,
    Fc0013,
    Fc2580,
    R820T,
    R828D,
}

impl Tuner {
    fn from_code(code: u32) -> Self {
        match code {
            1 => Self::E4000,
            2 => Self::Fc0012,
            3 => Self::Fc0013,
            4 => Self::Fc2580,
            5 => Self::R820T,
            6 => Self::R828D,
            _ => Self::Unknown,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Unknown => "unknown tuner",
            Self::E4000 => "E4000",
            Self::Fc0012 => "FC0012",
            Self::Fc0013 => "FC0013",
            Self::Fc2580 => "FC2580",
            Self::R820T => "R820T",
            Self::R828D => "R828D",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Greeting {
    pub(crate) tuner: Tuner,
    pub(crate) gain_steps: u32,
}

impl Greeting {
    pub(crate) fn parse(bytes: &[u8; GREETING_LEN]) -> Result<Self, DeviceError> {
        if &bytes[..4] != MAGIC {
            return Err(DeviceError::Io(format!(
                "not an rtl_tcp server: expected the greeting {:?}, got {:?}",
                String::from_utf8_lossy(MAGIC),
                String::from_utf8_lossy(&bytes[..4])
            )));
        }
        Ok(Self {
            tuner: Tuner::from_code(u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]])),
            gain_steps: u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub(crate) enum Command {
    SampleRate = 0x02,
    CenterFreq = 0x01,
    FreqCorrection = 0x05,
    GainMode = 0x03,
    Gain = 0x04,
    RtlAgc = 0x08,
    BiasTee = 0x0e,
}

impl Command {
    fn rank(self) -> u8 {
        match self {
            Self::SampleRate => 0,
            Self::CenterFreq => 1,
            Self::FreqCorrection => 2,
            Self::GainMode => 3,
            Self::Gain => 4,
            Self::RtlAgc => 5,
            Self::BiasTee => 6,
        }
    }
}

pub(crate) fn frame(command: Command, param: u32) -> [u8; 5] {
    let [a, b, c, d] = param.to_be_bytes();
    [command as u8, a, b, c, d]
}

pub(crate) fn ordered(mut batch: Vec<(Command, u32)>) -> Vec<(Command, u32)> {
    batch.sort_by_key(|(command, _)| command.rank());
    batch
}

#[cfg(test)]
mod tests {
    use super::*;

    fn greeting(magic: &[u8; 4], tuner: u32, gains: u32) -> [u8; GREETING_LEN] {
        let mut bytes = [0u8; GREETING_LEN];
        bytes[..4].copy_from_slice(magic);
        bytes[4..8].copy_from_slice(&tuner.to_be_bytes());
        bytes[8..].copy_from_slice(&gains.to_be_bytes());
        bytes
    }

    #[test]
    fn the_greeting_carries_the_tuner_and_its_step_count() {
        let parsed = Greeting::parse(&greeting(b"RTL0", 5, 29)).expect("a real greeting");
        assert_eq!(parsed.tuner, Tuner::R820T);
        assert_eq!(parsed.gain_steps, 29);
    }

    #[test]
    fn a_greeting_without_the_magic_is_refused_by_name() {
        let error = Greeting::parse(&greeting(b"HTTP", 5, 29)).expect_err("not rtl_tcp");
        assert!(error.to_string().contains("RTL0"), "{error}");
    }

    #[test]
    fn an_unrecognised_tuner_code_is_unknown_rather_than_a_failure() {
        for code in [0, 7, 99] {
            let parsed = Greeting::parse(&greeting(b"RTL0", code, 0)).expect("still a greeting");
            assert_eq!(parsed.tuner, Tuner::Unknown);
        }
    }

    #[test]
    fn a_command_is_its_byte_then_the_parameter_big_endian() {
        assert_eq!(
            frame(Command::CenterFreq, 100_000_000),
            [0x01, 0x05, 0xF5, 0xE1, 0x00]
        );
        assert_eq!(frame(Command::BiasTee, 1), [0x0e, 0, 0, 0, 1]);
    }

    #[test]
    fn a_negative_correction_survives_as_twos_complement() {
        assert_eq!(
            frame(Command::FreqCorrection, -12i32 as u32),
            [0x05, 0xFF, 0xFF, 0xFF, 0xF4]
        );
    }

    #[test]
    fn a_batch_is_ordered_rate_before_centre_and_mode_before_gain() {
        let batch = ordered(vec![
            (Command::Gain, 240),
            (Command::CenterFreq, 100_000_000),
            (Command::GainMode, 1),
            (Command::SampleRate, 2_048_000),
        ]);
        assert_eq!(
            batch.iter().map(|(c, _)| *c).collect::<Vec<_>>(),
            vec![
                Command::SampleRate,
                Command::CenterFreq,
                Command::GainMode,
                Command::Gain,
            ]
        );
    }
}
