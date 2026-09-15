use sdrmm_device::DeviceError;

pub(crate) const DEFAULT_PORT: u16 = 30_431;

/// Every IIOD command line ends this way, and the server answers with a decimal line of its own.
const EOL: &str = "\r\n";

pub(crate) fn version() -> String {
    format!("VERSION{EOL}")
}

pub(crate) fn print() -> String {
    format!("PRINT{EOL}")
}

pub(crate) fn timeout(ms: u32) -> String {
    format!("TIMEOUT {ms}{EOL}")
}

pub(crate) fn open(device: &str, samples: usize, mask: &[u32]) -> String {
    let mut line = format!("OPEN {device} {samples} ");
    for word in mask.iter().rev() {
        line.push_str(&format!("{word:08x}"));
    }
    line.push_str(EOL);
    line
}

pub(crate) fn close(device: &str) -> String {
    format!("CLOSE {device}{EOL}")
}

pub(crate) fn read_buf(device: &str, bytes: usize) -> String {
    format!("READBUF {device} {bytes}{EOL}")
}

pub(crate) fn write_buf(device: &str, bytes: usize) -> String {
    format!("WRITEBUF {device} {bytes}{EOL}")
}

pub(crate) fn read_device_attr(device: &str, attr: &str) -> String {
    format!("READ {device} {attr}{EOL}")
}

pub(crate) fn write_device_attr(device: &str, attr: &str, bytes: usize) -> String {
    format!("WRITE {device} {attr} {bytes}{EOL}")
}

pub(crate) fn read_channel_attr(device: &str, output: bool, channel: &str, attr: &str) -> String {
    format!("READ {device} {} {channel} {attr}{EOL}", way(output))
}

pub(crate) fn write_channel_attr(
    device: &str,
    output: bool,
    channel: &str,
    attr: &str,
    bytes: usize,
) -> String {
    format!(
        "WRITE {device} {} {channel} {attr} {bytes}{EOL}",
        way(output)
    )
}

const fn way(output: bool) -> &'static str {
    if output { "OUTPUT" } else { "INPUT" }
}

/// One answer line: a byte count to follow, or the negated errno of a refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Response {
    Bytes(usize),
    Refused(i32),
}

impl Response {
    pub(crate) fn parse(line: &str) -> Option<Self> {
        let value: i32 = line.trim().parse().ok()?;
        Some(if value < 0 {
            Self::Refused(value)
        } else {
            Self::Bytes(value as usize)
        })
    }

    pub(crate) fn bytes(self, what: &str) -> Result<usize, DeviceError> {
        match self {
            Self::Bytes(bytes) => Ok(bytes),
            Self::Refused(errno) => Err(refusal(what, errno)),
        }
    }
}

/// The server answers a refusal with the errno its own call returned, which is the only place a
/// reason for it exists — so it is translated here rather than reported as a bare number.
pub(crate) fn refusal(what: &str, errno: i32) -> DeviceError {
    let text = format!("{what}: {}", errno_text(errno));
    match -errno {
        2 | 19 => DeviceError::NotFound(text),
        16 => DeviceError::InUse(text),
        22 | 25 | 38 | 95 => DeviceError::Unsupported(text),
        _ => DeviceError::Io(text),
    }
}

fn errno_text(errno: i32) -> String {
    let name = match -errno {
        1 => "operation not permitted",
        2 => "no such attribute or device",
        5 => "input/output error",
        9 => "the buffer is not open",
        11 => "the radio had nothing ready",
        13 => "permission denied",
        16 => "the radio is busy",
        19 => "no such device",
        22 => "the radio refused that value",
        25 => "not a valid operation on this device",
        32 => "the connection broke",
        38 | 95 => "this firmware does not implement that",
        110 => "the radio did not answer in time",
        _ => return format!("iiod returned errno {}", -errno),
    };
    name.to_string()
}

/// The enabled-channel bitmask IIOD wants, one bit per scan-element index.
pub(crate) fn mask(indices: &[u32], total: usize) -> Vec<u32> {
    let words = total.div_ceil(32).max(1);
    let mut mask = vec![0u32; words];
    for index in indices {
        let word = (*index / 32) as usize;
        if let Some(slot) = mask.get_mut(word) {
            *slot |= 1 << (*index % 32);
        }
    }
    mask
}

/// How many characters the mask block that leads a refill occupies, hex words plus the newline.
pub(crate) const fn mask_len(words: usize) -> usize {
    words * 8 + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_line_ends_the_way_iiod_expects() {
        assert_eq!(version(), "VERSION\r\n");
        assert_eq!(close("cf-ad9361-lpc"), "CLOSE cf-ad9361-lpc\r\n");
        assert_eq!(timeout(5_000), "TIMEOUT 5000\r\n");
    }

    #[test]
    fn open_prints_the_mask_most_significant_word_first() {
        assert_eq!(
            open("cf-ad9361-lpc", 4096, &[0x3]),
            "OPEN cf-ad9361-lpc 4096 00000003\r\n"
        );
        assert_eq!(
            open("dev", 8, &[0x0000_000f, 0xdead_beef]),
            "OPEN dev 8 deadbeef0000000f\r\n"
        );
    }

    #[test]
    fn attribute_commands_name_the_channel_by_direction() {
        assert_eq!(
            read_channel_attr("ad9361-phy", false, "voltage0", "hardwaregain"),
            "READ ad9361-phy INPUT voltage0 hardwaregain\r\n"
        );
        assert_eq!(
            write_channel_attr("ad9361-phy", true, "altvoltage0", "frequency", 11),
            "WRITE ad9361-phy OUTPUT altvoltage0 frequency 11\r\n"
        );
        assert_eq!(
            read_device_attr("ad9361-phy", "ensm_mode"),
            "READ ad9361-phy ensm_mode\r\n"
        );
        assert_eq!(
            write_device_attr("ad9361-phy", "ensm_mode", 4),
            "WRITE ad9361-phy ensm_mode 4\r\n"
        );
    }

    #[test]
    fn a_response_line_is_a_length_or_an_errno() {
        assert_eq!(Response::parse("4096\n"), Some(Response::Bytes(4096)));
        assert_eq!(Response::parse(" 0 "), Some(Response::Bytes(0)));
        assert_eq!(Response::parse("-22"), Some(Response::Refused(-22)));
        assert_eq!(Response::parse("not a number"), None);
        assert_eq!(Response::parse(""), None);
    }

    #[test]
    fn a_refusal_carries_the_kind_the_errno_means() {
        assert!(matches!(
            Response::Refused(-22).bytes("set rate"),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(matches!(
            Response::Refused(-16).bytes("open"),
            Err(DeviceError::InUse(_))
        ));
        assert!(matches!(
            Response::Refused(-2).bytes("read attr"),
            Err(DeviceError::NotFound(_))
        ));
        assert!(matches!(
            Response::Refused(-5).bytes("refill"),
            Err(DeviceError::Io(_))
        ));
        let message = Response::Refused(-4242)
            .bytes("refill")
            .expect_err("a refusal")
            .to_string();
        assert!(message.contains("4242"), "{message}");
    }

    #[test]
    fn a_mask_sets_one_bit_per_enabled_scan_element() {
        assert_eq!(mask(&[0, 1], 2), vec![0b11]);
        assert_eq!(mask(&[2, 3], 4), vec![0b1100]);
        assert_eq!(mask(&[0, 33], 34), vec![0b1, 0b10]);
        assert_eq!(mask(&[], 0), vec![0]);
        assert_eq!(
            mask(&[99], 4),
            vec![0],
            "an index the device lacks is ignored"
        );
    }

    #[test]
    fn the_mask_block_is_eight_characters_a_word_and_a_newline() {
        assert_eq!(mask_len(1), 9);
        assert_eq!(mask_len(2), 17);
    }
}
