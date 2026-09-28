pub(crate) const PORT: u16 = 8073;

const MSG: &[u8] = b"MSG ";
const SND: &[u8] = b"SND";
const IQ_HEADER: usize = 20;
const IDENT: &str = "SDR--";

pub(crate) const ADC_OVERLOAD: u8 = 0x02;
const IQ: u8 = 0x08;
const COMPRESSED: u8 = 0x10;
const LITTLE_ENDIAN: u8 = 0x80;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    Busy(u32),
    Password(u32),
    Down,
    Redirect(String),
    Inactive,
    TimeLimit,
    Kicked,
}

impl Refusal {
    fn of(key: &str, value: &str) -> Option<Self> {
        match key {
            "too_busy" => Some(Self::Busy(value.parse().unwrap_or(0))),
            "badp" => match value.parse::<u32>() {
                Ok(0) => None,
                code => Some(Self::Password(code.unwrap_or(u32::MAX))),
            },
            "down" => Some(Self::Down),
            "redirect" => Some(Self::Redirect(value.to_string())),
            "inactivity_timeout" => Some(Self::Inactive),
            "ip_limit" => Some(Self::TimeLimit),
            "kiwi_kick" => Some(Self::Kicked),
            _ => None,
        }
    }

    pub(crate) fn reason(&self) -> String {
        match self {
            Self::Busy(channels) => format!("all {channels} KiwiSDR channels are busy"),
            Self::Password(1) => {
                "the KiwiSDR wants a password or has no free public channel".to_string()
            }
            Self::Password(5) => "the KiwiSDR allows one connection per address".to_string(),
            Self::Password(code) => format!("the KiwiSDR refused the login (badp={code})"),
            Self::Down => "the KiwiSDR is down".to_string(),
            Self::Redirect(to) => format!("the KiwiSDR redirects to {to}"),
            Self::Inactive => "the KiwiSDR inactivity limit ran out".to_string(),
            Self::TimeLimit => "the KiwiSDR per-user time limit ran out".to_string(),
            Self::Kicked => "the KiwiSDR admin ended the session".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Field {
    SampleRate(f64),
    AudioRate(u32),
    Span(f64),
    OffsetKhz(f64),
    Channels(u32),
    Refused(Refusal),
}

impl Field {
    fn of(key: &str, value: &str) -> Option<Self> {
        match key {
            "sample_rate" => value.parse().ok().map(Self::SampleRate),
            "audio_rate" => value.parse().ok().map(Self::AudioRate),
            "bandwidth" => value.parse().ok().map(Self::Span),
            "freq_offset" => value.parse().ok().map(Self::OffsetKhz),
            "rx_chans" => value.parse().ok().map(Self::Channels),
            _ => Refusal::of(key, value).map(Self::Refused),
        }
    }
}

pub(crate) fn fields(frame: &[u8]) -> Vec<Field> {
    let Some(line) = frame.strip_prefix(MSG) else {
        return Vec::new();
    };
    String::from_utf8_lossy(line)
        .split(' ')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            Field::of(key, &unescape(value))
        })
        .collect()
}

fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let escaped = (bytes[at] == b'%')
            .then(|| bytes.get(at + 1..at + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
                at += 3;
            }
            None => {
                out.push(bytes[at]);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn escape(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(byte).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Snd<'a> {
    pub flags: u8,
    pub seq: u32,
    pub iq: &'a [u8],
}

impl Snd<'_> {
    pub(crate) fn is_iq(&self) -> bool {
        self.flags & IQ != 0 && self.flags & COMPRESSED == 0
    }

    pub(crate) fn little_endian(&self) -> bool {
        self.flags & LITTLE_ENDIAN != 0
    }

    pub(crate) fn pairs(&self) -> usize {
        self.iq.len() / 4
    }
}

pub(crate) fn snd(frame: &[u8]) -> Option<Snd<'_>> {
    if !frame.starts_with(SND) || frame.len() < IQ_HEADER {
        return None;
    }
    Some(Snd {
        flags: frame[3],
        seq: u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]),
        iq: &frame[IQ_HEADER..],
    })
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Command {
    Login(String),
    AudioRate(u32),
    Unsquelch,
    Ident,
    Tune { khz: f64, half_width_hz: u32 },
    Gain { agc: bool, manual_db: u8 },
    Keepalive,
}

impl Command {
    pub(crate) fn encode(&self) -> String {
        match self {
            Self::Login(password) => format!("SET auth t=kiwi p={}", escape(password)),
            Self::AudioRate(rate) => format!("SET AR OK in={rate} out={rate}"),
            Self::Unsquelch => "SET squelch=0 max=0".to_string(),
            Self::Ident => format!("SET ident_user={IDENT}"),
            Self::Tune { khz, half_width_hz } => {
                format!(
                    "SET mod=iq low_cut=-{half_width_hz} high_cut={half_width_hz} freq={khz:.3}"
                )
            }
            Self::Gain { agc, manual_db } => format!(
                "SET agc={} hang=0 thresh=-100 slope=6 decay=1000 manGain={manual_db}",
                u8::from(*agc)
            ),
            Self::Keepalive => "SET keepalive".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_line_yields_every_known_field() {
        assert_eq!(
            fields(b"MSG version_maj=1 version_min=902 freq_offset=0.000 rx_chans=8"),
            vec![Field::OffsetKhz(0.0), Field::Channels(8)]
        );
        assert_eq!(
            fields(b"MSG center_freq=15000000 bandwidth=30000000 adc_clk_nom=66666600"),
            vec![Field::Span(30e6)]
        );
        assert_eq!(
            fields(b"MSG sample_rate=11998.924226"),
            vec![Field::SampleRate(11_998.924_226)]
        );
        assert_eq!(
            fields(b"MSG audio_init=0 audio_rate=12000"),
            vec![Field::AudioRate(12_000)]
        );
    }

    #[test]
    fn only_msg_frames_carry_fields() {
        assert!(fields(b"SND\x08rest").is_empty());
        assert!(fields(b"MSGsample_rate=1").is_empty());
    }

    #[test]
    fn refusals_are_recognised() {
        let refused = |line: &str| fields(format!("MSG {line}").as_bytes());
        assert_eq!(
            refused("too_busy=4"),
            vec![Field::Refused(Refusal::Busy(4))]
        );
        assert_eq!(
            refused("badp=1"),
            vec![Field::Refused(Refusal::Password(1))]
        );
        assert!(refused("badp=0").is_empty());
        assert_eq!(refused("down"), vec![Field::Refused(Refusal::Down)]);
        assert_eq!(
            refused("redirect=http%3A%2F%2Fother%3A8073"),
            vec![Field::Refused(Refusal::Redirect(
                "http://other:8073".into()
            ))]
        );
        assert_eq!(
            refused("inactivity_timeout=15"),
            vec![Field::Refused(Refusal::Inactive)]
        );
        assert_eq!(
            refused("ip_limit=60,1.2.3.4"),
            vec![Field::Refused(Refusal::TimeLimit)]
        );
        assert_eq!(
            refused("kiwi_kick=1,bye"),
            vec![Field::Refused(Refusal::Kicked)]
        );
    }

    #[test]
    fn broken_escapes_pass_through() {
        assert_eq!(unescape("a%20b"), "a b");
        assert_eq!(unescape("100%"), "100%");
        assert_eq!(unescape("%zz%4"), "%zz%4");
    }

    fn frame(flags: u8, seq: u32, payload: &[u8]) -> Vec<u8> {
        let mut out = b"SND".to_vec();
        out.push(flags);
        out.extend_from_slice(&seq.to_le_bytes());
        out.extend_from_slice(&[0; 12]);
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn an_iq_frame_splits_into_header_and_samples() {
        let bytes = frame(0x0D, 7, &[0; 8]);
        let got = snd(&bytes).expect("a frame");
        assert_eq!(got.seq, 7);
        assert!(got.is_iq());
        assert!(!got.little_endian());
        assert_eq!(got.pairs(), 2);
        assert!(snd(&bytes[..19]).is_none());
        assert!(snd(b"MSG xxxxxxxxxxxxxxxxxxxxxx").is_none());
    }

    #[test]
    fn audio_and_compressed_frames_are_not_iq() {
        assert!(!snd(&frame(0x00, 0, &[])).expect("frame").is_iq());
        assert!(!snd(&frame(0x18, 0, &[])).expect("frame").is_iq());
        assert!(snd(&frame(0x88, 0, &[])).expect("frame").little_endian());
    }

    #[test]
    fn commands_encode_as_the_kiwi_expects() {
        assert_eq!(Command::Login(String::new()).encode(), "SET auth t=kiwi p=");
        assert_eq!(
            Command::Login("a b&c=d".to_string()).encode(),
            "SET auth t=kiwi p=a%20b%26c%3Dd"
        );
        assert_eq!(unescape(&escape("pä ss%")), "pä ss%");
        assert_eq!(
            Command::AudioRate(12_000).encode(),
            "SET AR OK in=12000 out=12000"
        );
        assert_eq!(
            Command::Tune {
                khz: 7_074.0,
                half_width_hz: 5_999
            }
            .encode(),
            "SET mod=iq low_cut=-5999 high_cut=5999 freq=7074.000"
        );
        assert_eq!(
            Command::Gain {
                agc: false,
                manual_db: 60
            }
            .encode(),
            "SET agc=0 hang=0 thresh=-100 slope=6 decay=1000 manGain=60"
        );
        assert_eq!(Command::Ident.encode(), "SET ident_user=SDR--");
    }
}
