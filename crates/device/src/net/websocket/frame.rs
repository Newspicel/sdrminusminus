use std::hash::{BuildHasher, Hasher, RandomState};

pub(crate) const MAX_PAYLOAD: usize = 32 << 20;

pub(crate) const MAX_CONTROL_PAYLOAD: usize = 125;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Opcode {
    Continuation,
    Text,
    Binary,
    Close,
    Ping,
    Pong,
}

impl Opcode {
    pub(crate) fn from_bits(bits: u8) -> Option<Self> {
        match bits {
            0x0 => Some(Self::Continuation),
            0x1 => Some(Self::Text),
            0x2 => Some(Self::Binary),
            0x8 => Some(Self::Close),
            0x9 => Some(Self::Ping),
            0xA => Some(Self::Pong),
            _ => None,
        }
    }

    pub(crate) fn bits(self) -> u8 {
        match self {
            Self::Continuation => 0x0,
            Self::Text => 0x1,
            Self::Binary => 0x2,
            Self::Close => 0x8,
            Self::Ping => 0x9,
            Self::Pong => 0xA,
        }
    }

    pub(crate) fn is_control(self) -> bool {
        matches!(self, Self::Close | Self::Ping | Self::Pong)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Head {
    pub(crate) fin: bool,
    pub(crate) opcode: Opcode,
    masked: bool,
    marker: u8,
}

impl Head {
    pub(crate) fn parse(bytes: [u8; 2]) -> Result<Self, String> {
        if bytes[0] & 0x70 != 0 {
            return Err("a reserved frame bit is set, but no extension was negotiated".to_string());
        }
        let bits = bytes[0] & 0x0F;
        let opcode = Opcode::from_bits(bits)
            .ok_or_else(|| format!("frame opcode {bits:#x} is not one RFC 6455 defines"))?;
        let fin = bytes[0] & 0x80 != 0;
        if opcode.is_control() && !fin {
            return Err("a control frame may not be fragmented".to_string());
        }
        Ok(Self {
            fin,
            opcode,
            masked: bytes[1] & 0x80 != 0,
            marker: bytes[1] & 0x7F,
        })
    }

    /// How many bytes follow the two-byte head before the payload starts.
    pub(crate) fn extra(self) -> usize {
        let length = match self.marker {
            126 => 2,
            127 => 8,
            _ => 0,
        };
        length + if self.masked { 4 } else { 0 }
    }

    pub(crate) fn payload(self, extra: &[u8]) -> Result<(usize, Option<[u8; 4]>), String> {
        let (len, rest) = match self.marker {
            126 => (
                usize::from(u16::from_be_bytes([extra[0], extra[1]])),
                &extra[2..],
            ),
            127 => {
                let wide = u64::from_be_bytes([
                    extra[0], extra[1], extra[2], extra[3], extra[4], extra[5], extra[6], extra[7],
                ]);
                let len = usize::try_from(wide)
                    .map_err(|_| format!("frame payload of {wide} bytes does not fit in memory"))?;
                (len, &extra[8..])
            }
            marker => (usize::from(marker), extra),
        };
        if self.opcode.is_control() && len > MAX_CONTROL_PAYLOAD {
            return Err(format!(
                "control frame payload of {len} bytes exceeds the {MAX_CONTROL_PAYLOAD} RFC 6455 allows"
            ));
        }
        if len > MAX_PAYLOAD {
            return Err(format!(
                "frame payload of {len} bytes exceeds the {MAX_PAYLOAD} SDR-- will buffer"
            ));
        }
        let mask = self.masked.then(|| [rest[0], rest[1], rest[2], rest[3]]);
        Ok((len, mask))
    }
}

pub(crate) fn unmask(payload: &mut [u8], mask: [u8; 4]) {
    for (at, byte) in payload.iter_mut().enumerate() {
        *byte ^= mask[at % 4];
    }
}

pub(crate) fn encode(opcode: Opcode, payload: &[u8]) -> Vec<u8> {
    let mask = mask_key();
    let mut frame = Vec::with_capacity(payload.len() + 14);
    frame.push(0x80 | opcode.bits());
    match payload.len() {
        len if len < 126 => frame.push(0x80 | len as u8),
        len if len <= usize::from(u16::MAX) => {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(len as u16).to_be_bytes());
        }
        len => {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(len as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(&mask);
    frame.extend(
        payload
            .iter()
            .enumerate()
            .map(|(at, byte)| byte ^ mask[at % 4]),
    );
    frame
}

/// A value a proxy cannot predict, which is the only property RFC 6455 asks of a masking key.
pub(crate) fn entropy() -> u64 {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| u64::from(since.subsec_nanos())),
    );
    hasher.finish()
}

fn mask_key() -> [u8; 4] {
    let bytes = entropy().to_le_bytes();
    [bytes[0], bytes[1], bytes[2], bytes[3]]
}

pub(crate) fn close_reason(payload: &[u8]) -> String {
    let Some((code, text)) = payload.split_at_checked(2) else {
        return "the server closed the WebSocket".to_string();
    };
    let code = u16::from_be_bytes([code[0], code[1]]);
    let text = String::from_utf8_lossy(text);
    if text.is_empty() {
        format!("the server closed the WebSocket with status {code}")
    } else {
        format!("the server closed the WebSocket with status {code}: {text}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(bytes: [u8; 2]) -> Head {
        Head::parse(bytes).expect("a well-formed head")
    }

    #[test]
    fn a_head_carries_the_final_flag_and_the_opcode() {
        let text = head([0x81, 0x05]);
        assert!(text.fin);
        assert_eq!(text.opcode, Opcode::Text);
        assert_eq!(text.extra(), 0);
        assert_eq!(text.payload(&[]).expect("a short payload"), (5, None));

        let fragment = head([0x02, 0x00]);
        assert!(!fragment.fin);
        assert_eq!(fragment.opcode, Opcode::Binary);
    }

    #[test]
    fn extended_lengths_are_big_endian() {
        let medium = head([0x82, 126]);
        assert_eq!(medium.extra(), 2);
        assert_eq!(
            medium.payload(&[0x01, 0x00]).expect("a medium payload").0,
            256
        );

        let large = head([0x82, 127]);
        assert_eq!(large.extra(), 8);
        assert_eq!(
            large
                .payload(&[0, 0, 0, 0, 0, 0x01, 0x00, 0x00])
                .expect("a large payload")
                .0,
            65_536
        );
    }

    #[test]
    fn a_masked_server_frame_is_unmasked_with_the_key_it_carries() {
        let masked = head([0x81, 0x80 | 3]);
        assert_eq!(masked.extra(), 4);
        let (len, mask) = masked.payload(&[1, 2, 3, 4]).expect("a masked payload");
        assert_eq!(len, 3);
        let mask = mask.expect("the key");
        let mut payload = *b"abc";
        unmask(&mut payload, mask);
        assert_ne!(&payload, b"abc");
        unmask(&mut payload, mask);
        assert_eq!(&payload, b"abc", "unmasking twice is the identity");
    }

    #[test]
    fn frames_this_cannot_frame_are_refused_by_name() {
        assert!(
            Head::parse([0x81 | 0x40, 0x00]).is_err_and(|e| e.contains("reserved")),
            "a compressed frame must not be read as plain"
        );
        assert!(Head::parse([0x83, 0x00]).is_err_and(|e| e.contains("opcode")));
        assert!(Head::parse([0x09, 0x00]).is_err_and(|e| e.contains("fragmented")));
        assert!(
            head([0x89, 126])
                .payload(&[0x01, 0x00])
                .is_err_and(|e| e.contains("control frame")),
            "an oversized control frame is refused"
        );
        assert!(
            head([0x82, 127])
                .payload(&[0xFF, 0, 0, 0, 0, 0, 0, 0])
                .is_err(),
            "a payload larger than this will buffer is refused rather than allocated"
        );
    }

    #[test]
    fn a_client_frame_is_final_masked_and_round_trips() {
        for payload in [vec![0u8; 3], vec![7u8; 200], vec![9u8; 70_000]] {
            let frame = encode(Opcode::Binary, &payload);
            assert_eq!(frame[0], 0x82, "final binary frame");
            let head = Head::parse([frame[0], frame[1]]).expect("its own head");
            let (len, mask) = head.payload(&frame[2..]).expect("its own length");
            assert_eq!(len, payload.len());
            let mask = mask.expect("a client frame is always masked");
            let mut body = frame[2 + head.extra()..].to_vec();
            unmask(&mut body, mask);
            assert_eq!(body, payload);
        }
    }

    #[test]
    fn two_masking_keys_in_a_row_differ() {
        let keys: std::collections::BTreeSet<[u8; 4]> = (0..16).map(|_| mask_key()).collect();
        assert!(keys.len() > 1, "a constant mask key is not a mask key");
    }

    #[test]
    fn a_close_payload_is_reported_with_its_status_and_text() {
        assert!(close_reason(&[]).contains("closed the WebSocket"));
        assert!(close_reason(&1000u16.to_be_bytes()).contains("1000"));
        let mut payload = 1011u16.to_be_bytes().to_vec();
        payload.extend_from_slice(b"restarting");
        assert!(close_reason(&payload).contains("restarting"));
    }
}
