use sdrmm_dsp::crc16_msb;
use serde::Serialize;

use crate::acars::{AcarsApp, adsc, cpdlc};

const ARINC_CRC_POLY: u16 = 0x1021;
const ARINC_CRC_INIT: u16 = 0xFFFF;
const ARINC_CRC_GOOD: u16 = 0x1D0F;
const IMI_LEN: usize = 3;
const AIR_REG_LEN: usize = 7;
const CRC_HEX_LEN: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Imi {
    At1,
    Cr1,
    Cc1,
    Dr1,
    Ads,
    Dis,
}

impl Imi {
    pub fn as_str(&self) -> &'static str {
        match self {
            Imi::At1 => "AT1",
            Imi::Cr1 => "CR1",
            Imi::Cc1 => "CC1",
            Imi::Dr1 => "DR1",
            Imi::Ads => "ADS",
            Imi::Dis => "DIS",
        }
    }
}

const IMI_TABLE: [(&str, Imi); 6] = [
    (".AT1", Imi::At1),
    (".CR1", Imi::Cr1),
    (".CC1", Imi::Cc1),
    (".DR1", Imi::Dr1),
    (".ADS", Imi::Ads),
    (".DIS", Imi::Dis),
];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Envelope {
    pub gs_addr: String,
    pub air_reg: String,
    pub imi: Imi,
    pub crc_ok: bool,
}

fn arinc_crc(parts: &[&[u8]]) -> u16 {
    parts.iter().fold(ARINC_CRC_INIT, |crc, part| {
        crc16_msb(ARINC_CRC_POLY, crc, part)
    })
}

fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'A'..=b'F' => Some(10 + c - b'A'),
        b'a'..=b'f' => Some(10 + c - b'a'),
        _ => None,
    }
}

fn decode_hex(s: &str) -> Vec<u8> {
    let nibbles: Vec<u8> = s.bytes().map_while(hex_nibble).collect();
    nibbles
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| (pair[0] << 4) | pair[1])
        .collect()
}

pub fn parse(text: &str, downlink: bool) -> Option<AcarsApp> {
    let txt = text.strip_prefix('/').unwrap_or(text);

    let (pos, imi) = IMI_TABLE
        .iter()
        .find_map(|(pat, imi)| txt.find(pat).map(|p| (p, *imi)))?;
    if pos != 7 && pos != 4 {
        return None;
    }
    let gs_addr = txt.get(..pos)?;
    if !gs_addr
        .bytes()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return None;
    }

    let payload = txt.get(pos + 1..)?;
    if payload.len() < IMI_LEN + AIR_REG_LEN + CRC_HEX_LEN {
        return None;
    }
    let imi_str = payload.get(..IMI_LEN)?;
    let air_reg = payload.get(IMI_LEN..IMI_LEN + AIR_REG_LEN)?;
    let bytes = decode_hex(payload.get(IMI_LEN + AIR_REG_LEN..)?);
    let body_len = bytes.len().checked_sub(2)?;

    let crc_ok = arinc_crc(&[imi_str.as_bytes(), air_reg.as_bytes(), &bytes]) == ARINC_CRC_GOOD;
    let envelope = Envelope {
        gs_addr: gs_addr.to_owned(),
        air_reg: air_reg.to_owned(),
        imi,
        crc_ok,
    };
    let body = &bytes[..body_len];

    Some(match imi {
        Imi::Ads | Imi::Dis => AcarsApp::Adsc {
            message: adsc::parse(body, downlink, imi == Imi::Dis),
            envelope,
        },
        Imi::At1 | Imi::Cr1 | Imi::Cc1 | Imi::Dr1 => AcarsApp::Cpdlc {
            message: if imi == Imi::At1 {
                cpdlc::decode(body, downlink)
            } else {
                None
            },
            envelope,
            payload_hex: body.iter().map(|b| format!("{b:02x}")).collect(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_and_crc_on_real_message() {
        let app = parse(
            "/BOMASAI.ADS.VT-ANB072501A070A988CA73248F0E5DC10200000F5EE1ABC000102B885E0A19F5",
            true,
        )
        .expect("should parse");
        let AcarsApp::Adsc { envelope, .. } = &app else {
            panic!("expected ADS-C");
        };
        assert_eq!(envelope.gs_addr, "BOMASAI");
        assert_eq!(envelope.air_reg, ".VT-ANB");
        assert_eq!(envelope.imi, Imi::Ads);
        assert!(envelope.crc_ok, "real off-air message must pass CRC");
    }

    #[test]
    fn cpdlc_wilco_end_to_end() {
        let body = [0x02u8, 0x80, 0x00];
        let crc_bytes = (0..=u16::MAX)
            .map(u16::to_be_bytes)
            .find(|x| arinc_crc(&[b"AT1", b".N123AB", &body, x]) == ARINC_CRC_GOOD)
            .expect("a valid CRC trailer exists");
        let hex: String = body
            .iter()
            .chain(&crc_bytes)
            .map(|b| format!("{b:02X}"))
            .collect();
        let app = parse(&format!("/MSTEC7X.AT1.N123AB{hex}"), true).expect("parses");
        let AcarsApp::Cpdlc {
            envelope, message, ..
        } = &app
        else {
            panic!("expected CPDLC");
        };
        assert!(envelope.crc_ok);
        assert_eq!(envelope.imi, Imi::At1);
        let m = message.as_ref().expect("CPDLC body decodes");
        assert_eq!(m.msg_id, 5);
        assert_eq!(m.element, "dM0NULL");
        assert_eq!(m.text, "WILCO");
        assert!(!m.more_elements);
    }

    #[test]
    fn corrupted_payload_fails_crc_but_parses() {
        let app = parse(
            "/BOMASAI.ADS.VT-ANB072501A070A988CA73248F0E5DC10200000F5EE1ABC000102B885E0A19F4",
            true,
        )
        .expect("should still parse");
        let AcarsApp::Adsc { envelope, .. } = &app else {
            panic!("expected ADS-C");
        };
        assert!(!envelope.crc_ok);
    }

    #[test]
    fn rejects_non_arinc_text() {
        assert!(parse("POSN 4737.2N 12218.1W", true).is_none());
        assert!(parse("/SHORTX.ADS.A", true).is_none());
    }
}
