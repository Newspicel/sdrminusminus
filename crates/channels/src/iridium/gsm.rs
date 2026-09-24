use serde_json::{Map, Value, json};

use crate::datalink::hex;

fn major(tmaj: u8) -> Option<&'static str> {
    Some(match tmaj {
        0x03 => "CC",
        0x83 => "CC(dest)",
        0x05 => "MM",
        0x06 => "RR",
        0x08 => "GMM",
        0x0b => "SS",
        0x09 => "SMS",
        0x89 => "SMS(dest)",
        _ => return None,
    })
}

fn minor(tmin: u16) -> Option<&'static str> {
    Some(match tmin {
        0x0301 => "Alerting",
        0x0302 => "Call Proceeding",
        0x0303 => "Progress",
        0x0305 => "Setup",
        0x030f => "Connect Acknowledge",
        0x0325 => "Disconnect",
        0x032a => "Release Complete",
        0x032d => "Release",
        0x0502 => "Location Updating Accept",
        0x0504 => "Location Updating Reject",
        0x0508 => "Location Updating Request",
        0x0512 => "Authentication Request",
        0x0514 => "Authentication Response",
        0x0518 => "Identity request",
        0x0519 => "Identity response",
        0x051a => "TMSI Reallocation Command",
        0x0521 => "CM Service Accept",
        0x0522 => "CM Service Reject",
        0x0618 => "System Information Type 1",
        0x0619 => "System Information Type 2",
        0x061a => "System Information Type 3",
        0x061b => "System Information Type 4",
        0x061c => "System Information Type 5",
        0x061d => "System Information Type 6",
        0x0602 => "System Information Type 2ter",
        0x0607 => "System Information Type 2quater",
        0x0605 => "System Information Type 5bis",
        0x0606 => "System Information Type 5ter",
        0x061f => "System Information Type 2bis",
        0x0621 => "Paging Request Type 1",
        0x0622 => "Paging Request Type 2",
        0x0624 => "Paging Request Type 3",
        0x0627 => "Paging Response",
        0x063f => "Immediate Assignment",
        0x0639 => "Immediate Assignment Extended",
        0x063a => "Immediate Assignment Reject",
        0x063b => "Additional Assignment",
        0x0635 => "Channel Release",
        0x0805 => "GMM Detach Request",
        0x0b3b => "Register (SS)",
        0x0600 => "Register/SBD:uplink",
        0x0901 => "CP-DATA",
        0x0904 => "CP-ACK",
        0x0910 => "CP-ERROR",
        _ => return None,
    })
}

fn p_mi_iei(d: &[u8]) -> Option<(Value, usize)> {
    if d.len() < 2 {
        return None;
    }
    let iei_len = d[0] as usize;
    let iei_dig = d[1] >> 4;
    let iei_odd = (d[1] >> 3) & 1;
    let iei_typ = d[1] & 7;
    match iei_typ {
        1 | 2 => {
            if iei_odd == 1 && iei_len == 8 && d.len() >= 9 {
                let mut s = format!("{iei_dig:x}");
                for &b in &d[2..9] {
                    s.push_str(&format!("{:x}{:x}", b & 0xf, b >> 4));
                }
                let label = if iei_typ == 1 { "imsi" } else { "imei" };
                Some((json!({ "type": label, "value": s }), 9))
            } else {
                None
            }
        }
        4 => {
            if iei_odd == 0 && iei_len == 5 && iei_dig == 0xf && d.len() >= 6 {
                Some((
                    json!({ "type": "tmsi", "value": format!("{:02x}{:02x}{:02x}{:02x}", d[2], d[3], d[4], d[5]) }),
                    6,
                ))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn p_lai(d: &[u8]) -> Option<(Value, usize)> {
    if d.len() < 5 || d[1] >> 4 != 0xf {
        return None;
    }
    let mcc = format!("{}{}{}", d[0] & 0xf, d[0] >> 4, d[1] & 0xf);
    let mnc = format!("{}{}", d[2] >> 4, d[2] & 0xf);
    let lac = format!("{:02x}{:02x}", d[3], d[4]);
    Some((json!({ "mcc": mcc, "mnc": mnc, "lac": lac }), 5))
}

fn p_disc(d: &[u8]) -> Option<(Value, usize)> {
    if d.len() < 3 || d[0] < 2 || d[1] >> 4 != 0xe {
        return None;
    }
    let net = d[1] & 0xf;
    let cause = d[2] & 0x7f;
    let location = match net {
        0 => "user".to_string(),
        2 => "local".to_string(),
        3 => "transit".to_string(),
        4 => "remote".to_string(),
        n => format!("net:{n}"),
    };
    let cause_text = match cause {
        1 => "Unassigned number",
        16 => "Normal call clearing",
        17 => "User busy",
        31 => "Normal, unspecified",
        34 => "No channel available",
        41 => "Temporary failure",
        57 => "Bearer cap. not authorized",
        127 => "Interworking, unspecified",
        _ => "",
    };
    let consumed = if (d[2] >> 7) == 1 && d[0] == 3 && d.len() >= 4 && d[3] == 0x88 {
        4
    } else {
        3
    };
    Some((
        json!({ "location": location, "cause": cause, "cause_text": cause_text }),
        consumed,
    ))
}

pub fn decode(data: &[u8]) -> Option<Value> {
    if data.len() <= 2 {
        return None;
    }
    let tmaj = data[0];
    let maj = major(tmaj)?;
    if tmaj == 0x76 || (tmaj == 0x06 && data[1] == 0x00) {
        return None;
    }
    let b0 = if tmaj == 0x83 || tmaj == 0x89 {
        tmaj & 0x7f
    } else {
        tmaj
    };
    let tmin = ((b0 as u16) << 8) | data[1] as u16;
    let body = &data[2..];

    let mut obj = Map::new();
    obj.insert("type".into(), json!("gsm"));
    obj.insert("protocol".into(), json!(maj));
    obj.insert("message".into(), json!(minor(tmin).unwrap_or("?")));
    obj.insert("tmin".into(), json!(format!("{tmin:04x}")));

    match tmin {
        0x032d | 0x032a => {
            if body.len() == 4
                && body[0] == 8
                && let Some((v, _)) = p_disc(&body[1..])
            {
                obj.insert("disconnect".into(), v);
            }
        }
        0x0325 => {
            if let Some((v, _)) = p_disc(body) {
                obj.insert("disconnect".into(), v);
            }
        }
        0x0502 => {
            if let Some((lai, n)) = p_lai(body) {
                obj.insert("lai".into(), lai);
                let mut rest = &body[n..];
                if rest.first() == Some(&0x17) {
                    if let Some((mi, _)) = p_mi_iei(&rest[1..]) {
                        obj.insert("mobile_id".into(), mi);
                    }
                    rest = &rest[1..];
                }
                if rest.first() == Some(&0xa1) {
                    obj.insert("follow_on".into(), json!(true));
                }
            }
        }
        0x0508 => {
            if body.len() >= 7 && body[0] & 0xf == 0 && body[6] == 0x28 {
                let key = body[0] >> 4;
                obj.insert(
                    "key_seq".into(),
                    if key == 7 { json!("none") } else { json!(key) },
                );
                if let Some((lai, _)) = p_lai(&body[1..]) {
                    obj.insert("lai".into(), lai);
                }
                if body.len() > 7
                    && let Some((mi, _)) = p_mi_iei(&body[7..])
                {
                    obj.insert("mobile_id".into(), mi);
                }
            }
        }
        0x051a => {
            if let Some((lai, n)) = p_lai(body) {
                obj.insert("lai".into(), lai);
                if let Some((mi, _)) = p_mi_iei(&body[n..]) {
                    obj.insert("mobile_id".into(), mi);
                }
            }
        }
        0x0504 => {
            if body.first() == Some(&2) {
                obj.insert("reject".into(), json!("IMSI unknown in HLR"));
            }
        }
        0x0518 => match body.first() {
            Some(2) => {
                obj.insert("requested".into(), json!("IMEI"));
            }
            Some(1) => {
                obj.insert("requested".into(), json!("IMSI"));
            }
            _ => {}
        },
        0x0519 => {
            if let Some((mi, _)) = p_mi_iei(body) {
                obj.insert("mobile_id".into(), mi);
            }
        }
        _ => {}
    }

    if !body.is_empty() {
        obj.insert("body_hex".into(), json!(hex(body)));
    }
    Some(Value::Object(obj))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_gsm_major_is_none() {
        assert!(decode(&[0x76, 0x08, 0, 0, 0]).is_none());
        assert!(decode(&[0x06, 0x00, 0, 0, 0]).is_none());
    }

    #[test]
    fn identity_request_imei() {
        let v = decode(&[0x05, 0x18, 0x02]).expect("decodes");
        assert_eq!(v["protocol"], "MM");
        assert_eq!(v["message"], "Identity request");
        assert_eq!(v["requested"], "IMEI");
    }

    #[test]
    fn rr_messages_are_labelled() {
        for (mt, name) in [
            (0x3a_u8, "Immediate Assignment Reject"),
            (0x3b, "Additional Assignment"),
            (0x05, "System Information Type 5bis"),
            (0x07, "System Information Type 2quater"),
        ] {
            let v = decode(&[0x06, mt, 0x00, 0x00, 0x00]).expect("RR decodes");
            assert_eq!(v["protocol"], "RR", "0x06{mt:02x}");
            assert_eq!(v["message"], name, "0x06{mt:02x}");
        }
        for (mt, name) in [
            (0x3f_u8, "Immediate Assignment"),
            (0x21, "Paging Request Type 1"),
            (0x27, "Paging Response"),
            (0x18, "System Information Type 1"),
            (0x1a, "System Information Type 3"),
            (0x35, "Channel Release"),
        ] {
            let v = decode(&[0x06, mt, 0x00, 0x00, 0x00]).expect("RR decodes");
            assert_eq!(v["protocol"], "RR", "0x06{mt:02x}");
            assert_eq!(v["message"], name, "0x06{mt:02x}");
        }
    }

    #[test]
    fn rr_does_not_steal_sbd_hello() {
        assert!(decode(&[0x06, 0x00, 0x00, 0x00, 0x00]).is_none());
    }

    #[test]
    fn body_bytes_are_surfaced_as_hex() {
        let v = decode(&[0x09, 0x01, 0x01, 0x02, 0x03, 0xde, 0xad]).expect("SMS decodes");
        assert_eq!(v["protocol"], "SMS");
        assert_eq!(v["message"], "CP-DATA");
        assert_eq!(v["body_hex"], "010203dead");
        let v = decode(&[0x05, 0x18, 0x02]).expect("decodes");
        assert_eq!(v["requested"], "IMEI");
        assert_eq!(v["body_hex"], "02");
    }

    #[test]
    fn gmm_and_ss_are_labelled() {
        let v = decode(&[0x08, 0x05, 0x00, 0x00, 0x00]).expect("GMM decodes");
        assert_eq!(v["protocol"], "GMM");
        assert_eq!(v["message"], "GMM Detach Request");
        let v = decode(&[0x0b, 0x3b, 0x00, 0x00, 0x00]).expect("SS decodes");
        assert_eq!(v["protocol"], "SS");
        assert_eq!(v["message"], "Register (SS)");
    }
}
