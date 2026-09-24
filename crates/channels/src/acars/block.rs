use sdrmm_dsp::crc16_ccitt;
use serde::Serialize;

use crate::acars::AcarsCore;

const SOH: u8 = 0x01;
const STX: u8 = 0x02;
const ETX: u8 = 0x03;
const NAK: u8 = 0x15;
const ETB: u8 = 0x17;
const DEL: u8 = 0x7F;
const HEADER_LEN: usize = 12;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AcarsBlock {
    pub core: AcarsCore,
    pub downlink: bool,
    pub crc_ok: bool,
    pub parity_errors: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<crate::acars::min::DownlinkMin>,
}

pub fn parse(octets: &[u8]) -> Option<AcarsBlock> {
    if octets.len() < 1 + HEADER_LEN + 1 + 2 || octets[0] != SOH {
        return None;
    }
    let body = &octets[1..];

    let suffix_idx = if body[HEADER_LEN] & 0x7F == ETX || body[HEADER_LEN] & 0x7F == ETB {
        HEADER_LEN
    } else if body[HEADER_LEN] & 0x7F == STX {
        (HEADER_LEN + 1..body.len().saturating_sub(2))
            .find(|&i| matches!(body[i] & 0x7F, ETX | ETB))?
    } else {
        return None;
    };
    if body.len() < suffix_idx + 3 {
        return None;
    }

    let crc_ok = crc16_ccitt(&body[..suffix_idx + 3]) == 0;
    let parity_errors = body[..=suffix_idx]
        .iter()
        .filter(|&&c| c.count_ones() % 2 == 0)
        .count() as u32;

    let ch: Vec<u8> = body[..=suffix_idx].iter().map(|c| c & 0x7F).collect();
    let mode = ch[0] as char;
    let addr = &ch[1..8];
    let tail = if addr.iter().all(|&c| c == 0) {
        None
    } else {
        Some(
            addr.iter()
                .map(|&c| c as char)
                .skip_while(|&c| c == '.')
                .collect::<String>(),
        )
    };
    let ack = match ch[8] {
        NAK => None,
        c => Some(c as char),
    };
    let label: String = ch[9..11]
        .iter()
        .map(|&c| if c == DEL { 'd' } else { c as char })
        .collect();
    let block_id = match ch[11] {
        0 => None,
        c => Some(c as char),
    };
    let downlink = ch[11].is_ascii_digit();

    let mut msg_num = None;
    let mut flight = None;
    let mut text = String::new();
    if suffix_idx > HEADER_LEN {
        let mut payload = &ch[HEADER_LEN + 1..suffix_idx];
        if downlink && payload.len() >= 10 {
            msg_num = Some(payload[..4].iter().map(|&c| c as char).collect());
            flight = Some(payload[4..10].iter().map(|&c| c as char).collect());
            payload = &payload[10..];
        }
        text = payload.iter().map(|&c| c as char).collect();
    }

    let appdec = crate::acars::decode(&label, &text, downlink);
    let min = msg_num
        .as_deref()
        .and_then(crate::acars::min::split_downlink);
    Some(AcarsBlock {
        min,
        core: AcarsCore {
            mode,
            tail,
            label,
            sublabel: appdec.sublabel,
            mfi: appdec.mfi,
            block_id,
            ack,
            flight,
            msg_num,
            text,
            more_to_come: ch[suffix_idx] == ETB,
            reassembled: false,
            assstat: None,
            app: appdec.app.and_then(|a| serde_json::to_value(a).ok()),
            vdl2_link: None,
        },
        downlink,
        crc_ok,
        parity_errors,
    })
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub fn build(
    mode: char,
    tail: &str,
    ack: Option<char>,
    label: &str,
    block_id: char,
    msg_num: Option<&str>,
    flight: Option<&str>,
    text: &str,
    etb: bool,
) -> Vec<u8> {
    fn parity(c: u8) -> u8 {
        if c.count_ones().is_multiple_of(2) {
            c | 0x80
        } else {
            c
        }
    }
    let mut chars: Vec<u8> = Vec::new();
    chars.push(mode as u8);
    chars.extend(format!("{tail:.>7}").bytes());
    chars.push(ack.map(|c| c as u8).unwrap_or(NAK));
    chars.extend(label.bytes());
    chars.push(block_id as u8);
    let has_text = !text.is_empty() || msg_num.is_some();
    if has_text {
        chars.push(STX);
        if let Some(m) = msg_num {
            chars.extend(m.bytes());
        }
        if let Some(f) = flight {
            chars.extend(f.bytes());
        }
        chars.extend(text.bytes());
    }
    chars.push(if etb { ETB } else { ETX });
    let mut out: Vec<u8> = vec![SOH];
    out.extend(chars.into_iter().map(parity));
    let crc = crc16_ccitt(&out[1..]);
    out.push((crc & 0xFF) as u8);
    out.push((crc >> 8) as u8);
    out.push(DEL);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_downlink_with_adsc() {
        let octets = build(
            '2',
            "VT-ANB",
            None,
            "B6",
            '4',
            Some("M11A"),
            Some("AI0142"),
            "/BOMASAI.ADS.VT-ANB072501A070A988CA73248F0E5DC10200000F5EE1ABC000102B885E0A19F5",
            false,
        );
        let b = parse(&octets).expect("must parse");
        assert!(b.crc_ok);
        assert_eq!(b.parity_errors, 0);
        assert!(b.downlink);
        assert_eq!(b.core.tail.as_deref(), Some("VT-ANB"));
        assert_eq!(b.core.label, "B6");
        assert_eq!(b.core.flight.as_deref(), Some("AI0142"));
        let app = b.core.app.expect("ADS-C app should decode");
        assert_eq!(app["app"], "adsc");
        assert_eq!(app["crc_ok"], true);
    }

    #[test]
    fn downlink_block_surfaces_split_min() {
        let octets = build(
            '2',
            "N12345",
            None,
            "H1",
            '4',
            Some("M07C"),
            Some("UA1234"),
            "HELLO",
            false,
        );
        let b = parse(&octets).expect("must parse");
        assert!(b.crc_ok);
        assert_eq!(b.core.msg_num.as_deref(), Some("M07C"));
        let min = b.min.expect("downlink block has a split MIN");
        assert_eq!(min.msg_num, "M07");
        assert_eq!(min.msg_num_seq, 'C');
        assert_eq!(min.seq, Some(2));
    }

    #[test]
    fn uplink_block_has_no_min() {
        let octets = build('2', "N12345", Some('3'), "H1", 'A', None, None, "HI", false);
        let b = parse(&octets).expect("must parse");
        assert!(!b.downlink);
        assert!(b.min.is_none());
    }

    #[test]
    fn detects_bad_bcs() {
        let mut octets = build('2', "N123AB", Some('3'), "Q0", 'A', None, None, "", false);
        let n = octets.len();
        octets[n - 2] ^= 0x01;
        let b = parse(&octets).unwrap();
        assert!(!b.crc_ok);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse(&[0x55; 30]).is_none());
        assert!(parse(&[0x01, 0x32]).is_none());
    }
}
