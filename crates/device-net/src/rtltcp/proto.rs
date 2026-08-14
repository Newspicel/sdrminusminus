//! The rtl_tcp wire protocol, as osmocom's `rtl_tcp.c` defines it: a twelve-byte greeting, then
//! raw interleaved 8-bit IQ forever, with five-byte commands travelling the other way on the same
//! socket.
use sdrmm_device::DeviceError;

/// rtl_tcp's registered port, and what an operator who typed only a host means.
pub(crate) const DEFAULT_PORT: u16 = 1234;

/// The greeting: `"RTL0"`, the tuner type, and how many gain steps that tuner has.
pub(crate) const GREETING_LEN: usize = 12;
const MAGIC: &[u8] = b"RTL0";

/// Which tuner the remote dongle carries. The frequency range and gain table are the tuner's, so
/// this one byte of the greeting is the whole of what the protocol says about the hardware.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Tuner {
    /// A dongle whose tuner librtlsdr did not recognise — and, in practice, a server that is not
    /// librtlsdr at all: the rtl_tcp protocol is spoken by several SDR applications' remote
    /// sources, and they send zero here.
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
    /// `rtlsdr_tuner` as `rtlsdr_get_tuner_type` reports it.
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

/// What the server says about itself on connect.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Greeting {
    pub(crate) tuner: Tuner,
    /// How many entries the remote librtlsdr's gain table has. The values themselves are never
    /// sent, so this is only useful as a check on the table this backend holds for the tuner.
    pub(crate) gain_steps: u32,
}

impl Greeting {
    /// # Errors
    /// [`DeviceError::Io`] when the magic is absent — the port answered, but whatever is on it is
    /// not an rtl_tcp server, and the alternative is to read its output as IQ.
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
    /// Sent before a centre: librtlsdr re-runs the tuner setup from the new rate, so a centre
    /// written first would be overwritten.
    SampleRate = 0x02,
    CenterFreq = 0x01,
    FreqCorrection = 0x05,
    /// 0 selects the tuner's own AGC, 1 manual — and it has to precede a gain value, which the
    /// remote ignores while the AGC owns the tuner.
    GainMode = 0x03,
    /// Tenths of a dB. The remote snaps it to its own table and never says where it landed.
    Gain = 0x04,
    /// The RTL2832U's digital AGC, not the tuner's.
    RtlAgc = 0x08,
    BiasTee = 0x0e,
}

impl Command {
    /// Where this command sits in a batch. A retune has to follow the settings that would undo it,
    /// and a gain value has to follow the mode that lets it stick — the order is the point, so
    /// both the live path and the reconnect replay sort through here rather than each choosing for
    /// itself.
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

/// One command as it goes on the wire: the byte, then the parameter big-endian (`ntohl` on the
/// far side). Signed parameters — only the ppm correction — are sent as their two's complement,
/// which is what the server's `int` cast reads back.
pub(crate) fn frame(command: Command, param: u32) -> [u8; 5] {
    let [a, b, c, d] = param.to_be_bytes();
    [command as u8, a, b, c, d]
}

/// Put a batch into the order the radio has to receive it in.
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

    /// Reading a wrong port's output as IQ would show a plausible-looking noise floor forever.
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

    /// The server reads the parameter into an `int`, so a negative correction has to arrive as its
    /// two's complement rather than as a clamped zero.
    #[test]
    fn a_negative_correction_survives_as_twos_complement() {
        assert_eq!(
            frame(Command::FreqCorrection, -12i32 as u32),
            [0x05, 0xFF, 0xFF, 0xFF, 0xF4]
        );
    }

    /// The two orderings that are correctness, not taste: a rate change re-runs the tuner setup,
    /// and a gain value sent while the AGC owns the tuner is discarded.
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
