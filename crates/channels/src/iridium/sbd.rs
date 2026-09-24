use super::lcw::DaFrame;
use serde_json::json;

const FREQ_TOL_HZ: f64 = 2000.0;
const FRAG_GAP_S: f64 = 0.28;
const EXPIRE_S: f64 = 1.0;
const SBD_MULTI_EXPIRE_S: f64 = 5.0;

struct Pending {
    freq: f64,
    ul: bool,
    next_ctr: u8,
    data: Vec<u8>,
    last_time: f64,
}

struct MultiSbd {
    msgno: u8,
    msgcnt: u8,
    typ: u16,
    hdr: serde_json::Map<String, serde_json::Value>,
    ul: bool,
    body: Vec<u8>,
    last_time: f64,
}

type SbdParts = (
    u16,
    serde_json::Map<String, serde_json::Value>,
    u8,
    i16,
    Vec<u8>,
);

pub struct SbdReassembler {
    buf: Vec<Pending>,
    multi: Vec<MultiSbd>,
}

#[derive(Debug, Clone)]
pub struct SbdMessage {
    pub kind: &'static str,
    pub details: serde_json::Value,
    pub acars: Option<super::acars::AcarsBlock>,
}

impl SbdReassembler {
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            multi: Vec::new(),
        }
    }

    pub fn push(&mut self, f: &DaFrame, time: f64, freq: f64, ul: bool) -> Option<SbdMessage> {
        if !f.crc_ok {
            return None;
        }
        self.buf.retain(|p| time - p.last_time < EXPIRE_S);
        let bytes = &f.data[..(f.len as usize).min(20)];

        let m = self.buf.iter().position(|p| {
            (p.freq - freq).abs() < FREQ_TOL_HZ
                && p.ul == ul
                && p.next_ctr == f.ctr
                && time >= p.last_time
                && time <= p.last_time + FRAG_GAP_S
        });
        if let Some(i) = m {
            self.buf[i].data.extend_from_slice(bytes);
            self.buf[i].last_time = time;
            if f.continuation {
                self.buf[i].next_ctr = (f.ctr + 1) % 8;
                return None;
            }
            let p = self.buf.remove(i);
            return self.parse_l2(&p.data, p.ul, time);
        }

        match (f.ctr, f.continuation) {
            (0, false) => {
                let bytes = bytes.to_vec();
                self.parse_l2(&bytes, ul, time)
            }
            (0, true) => {
                self.buf.push(Pending {
                    freq,
                    ul,
                    next_ctr: 1,
                    data: bytes.to_vec(),
                    last_time: time,
                });
                None
            }
            _ => None,
        }
    }

    fn parse_l2(&mut self, data: &[u8], ul: bool, time: f64) -> Option<SbdMessage> {
        if data.len() < 5 {
            return None;
        }
        if let Some(pos) = super::mtpos::extract(data, ul) {
            return Some(SbdMessage {
                kind: "mt-position",
                details: pos,
                acars: None,
            });
        }
        if let Some(mut g) = super::gsm::decode(data) {
            g["raw_l2_hex"] = json!(data.iter().map(|b| format!("{b:02x}")).collect::<String>());
            g["ul"] = json!(ul);
            return Some(SbdMessage {
                kind: "gsm",
                details: g,
                acars: None,
            });
        }
        let (typ, hdr, msgno, msgcnt, body) = Self::sbd_parts(data, ul)?;

        self.multi
            .retain(|m| time - m.last_time < SBD_MULTI_EXPIRE_S);

        if msgno == 0 || (msgcnt <= 1 && msgno == 1) {
            return Self::parse_acars(typ, &body, hdr);
        }
        if msgcnt > 1 && msgno == 1 {
            self.multi.push(MultiSbd {
                msgno,
                msgcnt: msgcnt as u8,
                typ,
                hdr,
                ul,
                body,
                last_time: time,
            });
            return None;
        }
        if msgno > 1 {
            if let Some(i) = self
                .multi
                .iter()
                .position(|m| m.ul == ul && msgno == m.msgno + 1 && msgno <= m.msgcnt)
            {
                self.multi[i].body.extend_from_slice(&body);
                self.multi[i].msgno = msgno;
                self.multi[i].last_time = time;
                if msgno == self.multi[i].msgcnt {
                    let mut m = self.multi.remove(i);
                    m.hdr.insert("multi_packets".into(), json!(m.msgcnt));
                    return Self::parse_acars(m.typ, &m.body, m.hdr);
                }
                return None;
            }
            return None;
        }
        Self::parse_acars(typ, &body, hdr)
    }

    fn sbd_parts(data: &[u8], ul: bool) -> Option<SbdParts> {
        let (typ, mut rest): (u16, &[u8]) = match (data[0], data[1]) {
            (0x76, t) if t != 5 => (u16::from_be_bytes([data[0], data[1]]), &data[2..]),
            (0x06, 0x00) => (0x0600, &data[2..]),
            _ => return None,
        };
        let mut hdr = serde_json::Map::new();
        let mut msgno: u8 = 0;
        let mut msgcnt: i16 = -1;
        match typ {
            0x0600 => {
                if rest.len() < 29 {
                    return None;
                }
                let h = &rest[..29];
                if h[0] == 0x20 {
                    if let Some(imei) = imei_bcd(&h[5..13]) {
                        hdr.insert("imei".into(), json!(imei));
                    }
                    hdr.insert("momsn".into(), json!(u16::from_be_bytes([h[13], h[14]])));
                }
                hdr.insert("msg_count".into(), json!(h[15]));
                let it = u32::from_be_bytes([h[25], h[26], h[27], h[28]]);
                if it != 0 {
                    hdr.insert("time_unix".into(), json!(super::ira::iri_time_unix(it)));
                }
                msgcnt = h[15] as i16;
                msgno = if h[15] == 0 { 0 } else { 1 };
                return Some((typ, hdr, msgno, msgcnt, rest[29..].to_vec()));
            }
            t if t >> 8 == 0x76 && (t & 0xff) == 0x08 => match rest.first() {
                Some(0x26) if rest.len() >= 7 => {
                    hdr.insert(
                        "mtmsn".into(),
                        json!(u16::from_be_bytes([rest[1], rest[2]])),
                    );
                    hdr.insert("packets".into(), json!(rest[3]));
                    hdr.insert("backlog".into(), json!(rest[4]));
                    msgcnt = rest[3] as i16;
                    rest = &rest[7..];
                }
                Some(0x20) if rest.len() >= 5 => {
                    msgcnt = rest[3] as i16;
                    rest = &rest[5..];
                }
                _ => {}
            },
            _ => {}
        }
        if ul && rest.len() >= 3 && (rest[0] == 0x50 || rest[0] == 0x51) {
            rest = &rest[3..];
        }
        if rest.len() > 3 && rest[0] == 0x10 {
            let len = rest[1] as usize;
            msgno = rest[2];
            rest = &rest[3..];
            if rest.len() > len {
                rest = &rest[..len];
            }
        }
        Some((typ, hdr, msgno, msgcnt, rest.to_vec()))
    }

    fn parse_acars(
        typ: u16,
        payload: &[u8],
        mut hdr: serde_json::Map<String, serde_json::Value>,
    ) -> Option<SbdMessage> {
        hdr.insert("type".into(), json!(format!("{typ:04x}")));
        if payload.first() != Some(&0x01) || payload.len() < 16 {
            hdr.insert(
                "payload_hex".into(),
                json!(
                    payload
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                ),
            );
            let printable = payload
                .iter()
                .filter(|&&b| (0x20..0x7f).contains(&b))
                .count();
            if printable * 2 >= payload.len() {
                let text: String = payload
                    .iter()
                    .map(|&b| {
                        if (0x20..0x7f).contains(&b) {
                            b as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                hdr.insert("payload_text".into(), json!(text));
            }
            return Some(SbdMessage {
                kind: "sbd",
                details: serde_json::Value::Object(hdr),
                acars: None,
            });
        }
        let body = if payload.get(1) == Some(&0x03) && payload.len() > 9 {
            let mut b = vec![0x01];
            b.extend_from_slice(&payload[9..]);
            b
        } else {
            payload.to_vec()
        };
        let acars = super::acars::parse(&body);
        hdr.insert("acars_ok".into(), json!(acars.as_ref().map(|a| a.crc_ok)));
        Some(SbdMessage {
            kind: "sbd",
            details: serde_json::Value::Object(hdr),
            acars,
        })
    }
}

fn imei_bcd(b: &[u8]) -> Option<String> {
    if b.len() < 8 {
        return None;
    }
    let mut digits = Vec::with_capacity(16);
    for &x in &b[..8] {
        digits.push(x & 0xf);
        digits.push(x >> 4);
    }
    if digits.iter().any(|&d| d > 9) {
        return None;
    }
    Some(digits[1..16].iter().map(|d| char::from(b'0' + d)).collect())
}

impl Default for SbdReassembler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn imei_bcd_decodes_15_digits() {
        let bytes = [0x33, 0x00, 0x32, 0x04, 0x23, 0x91, 0x27, 0x01];
        assert_eq!(imei_bcd(&bytes).as_deref(), Some("300234032197210"));
        let bad = [0xa3, 0x00, 0x32, 0x04, 0x23, 0x91, 0x27, 0x01];
        assert_eq!(imei_bcd(&bad), None);
        assert_eq!(imei_bcd(&[0u8; 4]), None);
    }

    #[test]
    fn sbd_0600_registration_exposes_imei_momsn_count() {
        let mut data = vec![0x06, 0x00];
        let mut h = vec![0u8; 29];
        h[0] = 0x20;
        h[5..13].copy_from_slice(&[0x33, 0x00, 0x32, 0x04, 0x23, 0x91, 0x27, 0x01]);
        h[13] = 0x01;
        h[14] = 0x2c;
        h[15] = 1;
        data.extend_from_slice(&h);
        data.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        let m = SbdReassembler::new()
            .parse_l2(&data, true, 0.0)
            .expect("sbd message");
        assert_eq!(m.kind, "sbd");
        assert_eq!(m.details["type"], json!("0600"));
        assert_eq!(m.details["imei"], json!("300234032197210"));
        assert_eq!(m.details["momsn"], json!(300));
        assert_eq!(m.details["msg_count"], json!(1));
        assert_eq!(m.details["payload_hex"], json!("deadbeef"));
        assert!(m.details.get("time_unix").is_none());
    }

    #[test]
    fn sbd_0600_non_0x20_subtype_omits_imei() {
        let mut data = vec![0x06, 0x00];
        let mut h = vec![0u8; 29];
        h[0] = 0x10;
        h[5..13].copy_from_slice(&[0x33, 0x00, 0x32, 0x04, 0x23, 0x91, 0x27, 0x01]);
        h[13] = 0x01;
        h[14] = 0x2c;
        h[15] = 1;
        data.extend_from_slice(&h);
        data.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        let m = SbdReassembler::new()
            .parse_l2(&data, false, 0.0)
            .expect("sbd message");
        assert_eq!(m.details["type"], json!("0600"));
        assert!(m.details.get("imei").is_none(), "imei must be 0x20-only");
        assert!(m.details.get("momsn").is_none(), "momsn must be 0x20-only");
        assert_eq!(m.details["msg_count"], json!(1));
    }

    #[test]
    fn sbd_text_payload_is_rendered() {
        let mut data = vec![0x76, 0x08, 0x26, 0, 0, 0, 0, 0, 0];
        data.extend_from_slice(b"ST_TXT:ID:01");
        let m = SbdReassembler::new()
            .parse_l2(&data, false, 0.0)
            .expect("sbd message");
        assert_eq!(m.details["type"], json!("7608"));
        assert_eq!(m.details["payload_text"], json!("ST_TXT:ID:01"));
        assert!(m.acars.is_none(), "not ACARS (no 0x01 SOH)");
    }

    #[test]
    fn sbd_7608_transfer_exposes_mtmsn_packets_backlog() {
        let data = [
            0x76, 0x08, 0x26, 0x00, 0x07, 0x01, 0x02, 0x00, 0x00, 0xca, 0xfe,
        ];
        let m = SbdReassembler::new()
            .parse_l2(&data, false, 0.0)
            .expect("sbd message");
        assert_eq!(m.details["type"], json!("7608"));
        assert_eq!(m.details["mtmsn"], json!(7));
        assert_eq!(m.details["packets"], json!(1));
        assert_eq!(m.details["backlog"], json!(2));
        assert_eq!(m.details["payload_hex"], json!("cafe"));
    }

    #[test]
    fn sbd_multi_packet_reassembles() {
        let mut r = SbdReassembler::new();
        let p1 = [
            0x76, 0x08, 0x26, 0x00, 0x05, 0x02, 0x00, 0x00, 0x00, 0x10, 0x02, 0x01, 0xde, 0xad,
        ];
        assert!(
            r.parse_l2(&p1, false, 0.0).is_none(),
            "first packet must buffer"
        );
        let p2 = [0x76, 0x0a, 0x10, 0x02, 0x02, 0xbe, 0xef];
        let m = r
            .parse_l2(&p2, false, 0.1)
            .expect("multi-packet message completes");
        assert_eq!(
            m.details["type"],
            json!("7608"),
            "carries first packet's type"
        );
        assert_eq!(
            m.details["payload_hex"],
            json!("deadbeef"),
            "bodies concatenated"
        );
        assert_eq!(
            m.details["multi_packets"],
            json!(2),
            "marked as 2-packet reassembly"
        );
        assert!(
            r.parse_l2(&p2, false, 0.2).is_none(),
            "orphan continuation dropped"
        );
    }

    #[test]
    fn sbd_multi_packet_expires() {
        let mut r = SbdReassembler::new();
        let p1 = [
            0x76, 0x08, 0x26, 0x00, 0x05, 0x02, 0x00, 0x00, 0x00, 0x10, 0x02, 0x01, 0xde, 0xad,
        ];
        assert!(r.parse_l2(&p1, false, 0.0).is_none());
        let p2 = [0x76, 0x0a, 0x10, 0x02, 0x02, 0xbe, 0xef];
        assert!(
            r.parse_l2(&p2, false, 6.0).is_none(),
            "late continuation expired"
        );
    }
}
