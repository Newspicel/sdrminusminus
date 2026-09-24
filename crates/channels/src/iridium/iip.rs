use serde_json::{Value, json};

use super::rs::{bytes_of_bits, checksum_16, first_39, iip_crc24, rs8_correct};
use crate::datalink::hex;

fn iip_type_name(t: u8) -> &'static str {
    match t {
        0x01 => "ack-idle",
        0x04 => "data",
        _ => "unknown",
    }
}

fn base64_decode(s: &[u8]) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let s: Vec<u8> = s.iter().copied().take_while(|&c| c != b'=').collect();
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &c in &s {
        acc = (acc << 6) | val(c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

fn scan_credentials(bytes: &[u8]) -> Vec<Value> {
    let mut found = Vec::new();
    scan_ppp_pap(bytes, &mut found);
    scan_http_basic(bytes, &mut found);
    found
}

fn scan_ppp_pap(bytes: &[u8], out: &mut Vec<Value>) {
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == 0xC0 && bytes[i + 1] == 0x23 {
            let p = &bytes[i + 2..];
            if p.len() >= 5 && p[0] == 0x01 {
                let length = u16::from_be_bytes([p[2], p[3]]) as usize;
                let pkt = if length >= 4 && length <= p.len() { &p[..length] } else { p };
                if pkt.len() >= 5 {
                    let peer_len = pkt[4] as usize;
                    if 5 + peer_len < pkt.len() {
                        let peer = &pkt[5..5 + peer_len];
                        let pw_off = 5 + peer_len;
                        let pw_len = pkt[pw_off] as usize;
                        if pw_off + 1 + pw_len <= pkt.len() {
                            let pw = &pkt[pw_off + 1..pw_off + 1 + pw_len];
                            out.push(json!({
                                "kind": "ppp-pap",
                                "username": String::from_utf8_lossy(peer),
                                "password": String::from_utf8_lossy(pw),
                            }));
                        }
                    }
                }
            }
        }
        i += 1;
    }
}

fn scan_http_basic(bytes: &[u8], out: &mut Vec<Value>) {
    const NEEDLE: &[u8] = b"Authorization:";
    let mut from = 0usize;
    while let Some(rel) = find_ci(&bytes[from..], NEEDLE) {
        let mut j = from + rel + NEEDLE.len();
        while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
            j += 1;
        }
        if matches_ci(&bytes[j..], b"Basic") {
            j += 5;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            let start = j;
            while j < bytes.len() && is_b64(bytes[j]) {
                j += 1;
            }
            if j > start {
                if let Some(dec) = base64_decode(&bytes[start..j]) {
                    if let Some(colon) = dec.iter().position(|&b| b == b':') {
                        out.push(json!({
                            "kind": "http-basic",
                            "username": String::from_utf8_lossy(&dec[..colon]),
                            "password": String::from_utf8_lossy(&dec[colon + 1..]),
                        }));
                    }
                }
            }
        }
        from += rel + NEEDLE.len();
    }
}

fn is_b64(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'='
}

fn matches_ci(hay: &[u8], needle: &[u8]) -> bool {
    hay.len() >= needle.len()
        && hay[..needle.len()]
            .iter()
            .zip(needle)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
}

fn find_ci(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| matches_ci(&hay[i..], needle))
}

pub fn parse_iip_frame(payload_r: &[u8]) -> Value {
    let hdr = payload_r[0];
    let seq = payload_r[1];
    let ack = payload_r[2];
    let cs = payload_r[3];
    let sum = hdr as u32 + seq as u32 + ack as u32 + cs as u32;
    let cs_ok = sum > 0 && sum % 255 == 0;
    let mut out = json!({
        "ip_type": iip_type_name(hdr),
        "ip_type_code": hdr,
        "seq": seq,
        "ack": ack,
        "header_cs_ok": cs_ok,
    });
    let data = &payload_r[4..payload_r.len().saturating_sub(3).max(4)];
    match hdr {
        0x04 if !data.is_empty() => {
            let len = (data[0] as usize).min(data.len() - 1);
            let payload = &data[1..1 + len];
            out["data_hex"] = json!(hex(payload));
            let text: String = payload
                .iter()
                .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
                .collect();
            out["data_ascii"] = json!(text);
            let creds = scan_credentials(payload);
            if !creds.is_empty() {
                out["credentials"] = json!(creds);
            }
        }
        0x01 => {
            let end = data.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
            if end > 0 {
                out["data_hex"] = json!(hex(&data[..end]));
            }
        }
        _ => {
            out["data_hex"] = json!(hex(data));
        }
    }
    out
}

pub fn parse_ip_payload(payload_bits: &[u8]) -> Option<Value> {
    if payload_bits.len() < 312 {
        return None;
    }
    let (straight, reversed) = bytes_of_bits(payload_bits);
    if iip_crc24(&reversed) == 0 {
        let mut frame = parse_iip_frame(&reversed);
        frame["ip_frame"] = json!("IIP");
        return Some(frame);
    }
    let Some(msg) = first_39(&straight).and_then(|sent| rs8_correct(&sent)) else {
        return Some(json!({ "ip_frame": "IIU" }));
    };
    if checksum_16(&msg) == 0 {
        let mut frame = json!({
            "ip_frame": "IIR",
            "data_hex": hex(&msg[..29]),
        });
        let credentials = scan_credentials(&msg[..29]);
        if !credentials.is_empty() {
            frame["credentials"] = json!(credentials);
        }
        return Some(frame);
    }
    let value = u16::from_le_bytes([msg[0], msg[1]]);
    Some(json!({
        "ip_frame": "IIQ",
        "flags": value & 7,
        "counter": value >> 3,
        "data_hex": hex(&msg[2..]),
    }))
}

#[cfg(test)]
mod tests {
    use super::super::encode::bits_of_hex;
    use super::*;

    const IIP_ACK: &str =
        "805488c300000000000000000000000000000000000000000000000000000000000000004fd4e0";
    const IIP_DATA: &str =
        "20e0c689a012a23232f200000000000000000000000000000000000000000000000000002d6aa5";
    const IIQ: &str =
        "1d09e7eee7615ef35f30e49b482e15cae75007201e12617b0feda7e1647796da2d5bcc9793d179";
    const IIR: &str =
        "ff022bea8ed02a82a175930f2337cd3794c52208006d6b1af0c0cbd625f2de9780ccb7e209fece";
    const IIU: &str =
        "658aac2c9faa07d13c447e33051eeef95a60e56143d6c43bcad76c008a9b0a6b5fc933154a6de2";

    #[test]
    fn iip_ack_frame() {
        let v = parse_ip_payload(&bits_of_hex(IIP_ACK)).expect("frame");
        assert_eq!(v["ip_frame"], "IIP");
        assert_eq!(v["ip_type"], "ack-idle");
        assert_eq!(v["seq"], 42);
        assert_eq!(v["ack"], 17);
        assert_eq!(v["header_cs_ok"], true);
    }

    #[test]
    fn iip_data_frame_extracts_payload() {
        let v = parse_ip_payload(&bits_of_hex(IIP_DATA)).expect("frame");
        assert_eq!(v["ip_frame"], "IIP");
        assert_eq!(v["ip_type"], "data");
        assert_eq!(v["seq"], 7);
        assert_eq!(v["ack"], 99);
        assert_eq!(v["header_cs_ok"], true);
        assert_eq!(v["data_ascii"], "HELLO");
    }

    #[test]
    fn iiq_frame() {
        let v = parse_ip_payload(&bits_of_hex(IIQ)).expect("frame");
        assert_eq!(v["ip_frame"], "IIQ");
        assert_eq!(v["flags"], 5);
        assert_eq!(v["counter"], 0x123);
        assert_eq!(
            v["data_hex"],
            "e7eee7615ef35f30e49b482e15cae75007201e12617b0feda7e1647796"
        );
    }

    #[test]
    fn iir_frame() {
        let v = parse_ip_payload(&bits_of_hex(IIR)).expect("frame");
        assert_eq!(v["ip_frame"], "IIR");
        assert_eq!(
            v["data_hex"],
            "ff022bea8ed02a82a175930f2337cd3794c52208006d6b1af0c0cbd625"
        );
    }

    #[test]
    fn iiu_fallback() {
        let v = parse_ip_payload(&bits_of_hex(IIU)).expect("frame");
        assert_eq!(v["ip_frame"], "IIU");
    }

    #[test]
    fn base64_rfc4648_vectors() {
        assert_eq!(base64_decode(b"Zm9v"), Some(b"foo".to_vec()));
        assert_eq!(base64_decode(b"Zm9vYg=="), Some(b"foob".to_vec()));
        assert_eq!(base64_decode(b"Zm9vYmFy"), Some(b"foobar".to_vec()));
        assert_eq!(
            base64_decode(b"QWxhZGRpbjpvcGVuIHNlc2FtZQ=="),
            Some(b"Aladdin:open sesame".to_vec())
        );
        assert_eq!(base64_decode(b"not base64!"), None);
    }

    #[test]
    fn ppp_pap_authenticate_request() {
        let user = b"sat-user";
        let pass = b"hunter2";
        let mut pkt = vec![0xC0, 0x23];
        let mut body = vec![0x01, 0x42];
        let length = 4 + 1 + user.len() + 1 + pass.len();
        body.extend_from_slice(&(length as u16).to_be_bytes());
        body.push(user.len() as u8);
        body.extend_from_slice(user);
        body.push(pass.len() as u8);
        body.extend_from_slice(pass);
        pkt.extend_from_slice(&body);

        let creds = scan_credentials(&pkt);
        assert_eq!(creds.len(), 1, "one credential expected");
        assert_eq!(creds[0]["kind"], "ppp-pap");
        assert_eq!(creds[0]["username"], "sat-user");
        assert_eq!(creds[0]["password"], "hunter2");
    }

    #[test]
    fn http_basic_auth_header() {
        let payload = b"GET / HTTP/1.1\r\nAuthorization: Basic YWRtaW46czNjcjN0\r\n\r\n";
        let creds = scan_credentials(payload);
        assert_eq!(creds.len(), 1);
        assert_eq!(creds[0]["kind"], "http-basic");
        assert_eq!(creds[0]["username"], "admin");
        assert_eq!(creds[0]["password"], "s3cr3t");

        let p2 = b"authorization: basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==";
        let c2 = scan_credentials(p2);
        assert_eq!(c2[0]["username"], "Aladdin");
        assert_eq!(c2[0]["password"], "open sesame");
    }

    #[test]
    fn no_credentials_in_plain_payload() {
        assert!(scan_credentials(b"GET /index.html HTTP/1.1\r\nHost: x\r\n").is_empty());
        assert!(scan_credentials(&[0u8; 29]).is_empty());
        assert!(scan_credentials(&[0xC0, 0x23, 0x02, 0x01, 0x00, 0x04]).is_empty());
    }

    #[test]
    fn iip_data_frame_surfaces_pap_credentials() {
        let mut payload = vec![0xC0, 0x23, 0x01, 0x01];
        let length = 4 + 1 + 1 + 1 + 1;
        payload.extend_from_slice(&(length as u16).to_be_bytes());
        payload.push(1);
        payload.push(b'u');
        payload.push(1);
        payload.push(b'p');
        let mut frame = vec![0x04, 0x00, 0x00, 0x00, payload.len() as u8];
        frame.extend_from_slice(&payload);
        frame.extend_from_slice(&[0u8, 0u8, 0u8]);
        let v = parse_iip_frame(&frame);
        let creds = v["credentials"].as_array().expect("credentials present");
        assert_eq!(creds[0]["kind"], "ppp-pap");
        assert_eq!(creds[0]["username"], "u");
        assert_eq!(creds[0]["password"], "p");
    }
}
