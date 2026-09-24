use serde::Serialize;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Oooi {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depa: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dsta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gtout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gtin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wloff: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wlin: Option<String>,
}

impl Oooi {
    fn is_empty(&self) -> bool {
        self.depa.is_none()
            && self.dsta.is_none()
            && self.eta.is_none()
            && self.gtout.is_none()
            && self.gtin.is_none()
            && self.wloff.is_none()
            && self.wlin.is_none()
    }
}

fn airport(txt: &[u8], at: usize) -> Option<String> {
    let s = txt.get(at..at + 4)?;
    if s.iter()
        .all(|&c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        Some(String::from_utf8_lossy(s).into_owned())
    } else {
        None
    }
}

fn time4(txt: &[u8], at: usize) -> Option<String> {
    let s = txt.get(at..at + 4)?;
    if !s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let hh = (s[0] - b'0') * 10 + (s[1] - b'0');
    let mm = (s[2] - b'0') * 10 + (s[3] - b'0');
    if hh > 23 || mm > 59 {
        return None;
    }
    Some(String::from_utf8_lossy(s).into_owned())
}

fn at_is(txt: &[u8], at: usize, c: u8) -> bool {
    txt.get(at) == Some(&c)
}

fn starts(txt: &[u8], prefix: &[u8]) -> bool {
    txt.len() >= prefix.len() && &txt[..prefix.len()] == prefix
}

pub fn decode(label: &str, text: &str) -> Option<Oooi> {
    let t = text.as_bytes();
    let mut o = Oooi::default();
    if label.starts_with('Q') {
        q_series(label, t, &mut o)?;
    } else {
        numbered(label, t, &mut o)?;
    }
    if o.is_empty() { None } else { Some(o) }
}

fn q_series(label: &str, t: &[u8], o: &mut Oooi) -> Option<()> {
    match label {
        "Q1" => {
            o.depa = airport(t, 0);
            o.gtout = time4(t, 4);
            o.wloff = time4(t, 8);
            o.wlin = time4(t, 12);
            o.gtin = time4(t, 16);
            o.dsta = airport(t, 24);
        }
        "Q2" => {
            o.depa = airport(t, 0);
            o.eta = time4(t, 4);
        }
        "QA" => {
            o.depa = airport(t, 0);
            o.gtout = time4(t, 4);
        }
        "QB" => {
            o.depa = airport(t, 0);
            o.wloff = time4(t, 4);
        }
        "QC" => {
            o.depa = airport(t, 0);
            o.wlin = time4(t, 4);
        }
        "QD" => {
            o.depa = airport(t, 0);
            o.gtin = time4(t, 4);
        }
        "QE" => {
            o.depa = airport(t, 0);
            o.gtout = time4(t, 4);
            o.dsta = airport(t, 8);
        }
        "QF" => {
            o.depa = airport(t, 0);
            o.wloff = time4(t, 4);
            o.dsta = airport(t, 8);
        }
        "QG" => {
            o.depa = airport(t, 0);
            o.gtout = time4(t, 4);
            o.gtin = time4(t, 8);
        }
        "QH" => {
            o.depa = airport(t, 0);
            o.gtout = time4(t, 4);
        }
        "QK" => {
            o.depa = airport(t, 0);
            o.wlin = time4(t, 4);
            o.dsta = airport(t, 8);
        }
        "QL" => {
            o.dsta = airport(t, 0);
            o.gtin = time4(t, 8);
            o.depa = airport(t, 13);
        }
        "QM" => {
            o.dsta = airport(t, 0);
            o.depa = airport(t, 8);
        }
        "QN" => {
            o.dsta = airport(t, 4);
            o.eta = time4(t, 8);
        }
        "QP" => {
            o.depa = airport(t, 0);
            o.dsta = airport(t, 4);
            o.gtout = time4(t, 8);
        }
        "QQ" => {
            o.depa = airport(t, 0);
            o.dsta = airport(t, 4);
            o.wloff = time4(t, 8);
        }
        "QR" => {
            o.depa = airport(t, 0);
            o.dsta = airport(t, 4);
            o.wlin = time4(t, 8);
        }
        "QS" => {
            o.depa = airport(t, 0);
            o.dsta = airport(t, 4);
            o.gtin = time4(t, 8);
        }
        "QT" => {
            o.depa = airport(t, 0);
            o.dsta = airport(t, 4);
            o.gtout = time4(t, 8);
            o.gtin = time4(t, 12);
        }
        _ => return None,
    }
    Some(())
}

fn numbered(label: &str, t: &[u8], o: &mut Oooi) -> Option<()> {
    match label {
        "10" => {
            if !starts(t, b"ARR01") {
                return None;
            }
            o.dsta = airport(t, 12);
            o.eta = time4(t, 16);
        }
        "11" => {
            if t.get(13..17) != Some(b"/DS ") {
                return None;
            }
            o.dsta = airport(t, 17);
            if t.get(21..26) != Some(b"/ETA ") {
                return None;
            }
            o.eta = time4(t, 26);
        }
        "12" | "1G" | "83" => {
            if !at_is(t, 4, b',') {
                return None;
            }
            o.depa = airport(t, 0);
            o.dsta = airport(t, 5);
        }
        "15" => {
            if !starts(t, b"FST01") {
                return None;
            }
            o.depa = airport(t, 5);
            o.dsta = airport(t, 9);
        }
        "17" => {
            if !starts(t, b"ETA ") {
                return None;
            }
            o.eta = time4(t, 4);
            if !at_is(t, 8, b',') {
                return None;
            }
            o.depa = airport(t, 9);
            if !at_is(t, 13, b',') {
                return None;
            }
            o.dsta = airport(t, 14);
        }
        "20" => {
            if !starts(t, b"RST") {
                return None;
            }
            o.depa = airport(t, 22);
            o.dsta = airport(t, 26);
        }
        "21" => {
            if !at_is(t, 6, b',') {
                return None;
            }
            o.depa = airport(t, 7);
            if !at_is(t, 11, b',') {
                return None;
            }
            o.dsta = airport(t, 12);
        }
        "2N" => {
            if !starts(t, b"TKO01") || !at_is(t, 11, b'/') {
                return None;
            }
            o.depa = airport(t, 20);
            o.dsta = airport(t, 24);
        }
        "2Z" => {
            o.dsta = airport(t, 0);
        }
        "33" => {
            if !at_is(t, 0, b',') || !at_is(t, 20, b',') {
                return None;
            }
            o.depa = airport(t, 21);
            if !at_is(t, 25, b',') {
                return None;
            }
            o.dsta = airport(t, 26);
        }
        "39" => {
            if !starts(t, b"GTA01") || !at_is(t, 15, b'/') {
                return None;
            }
            o.depa = airport(t, 24);
            o.dsta = airport(t, 28);
        }
        "45" => {
            if !at_is(t, 0, b'A') {
                return None;
            }
            o.dsta = airport(t, 1);
        }
        "80" => {
            if t.get(6..11) != Some(b"/DEST") || !at_is(t, 11, b'/') {
                return None;
            }
            o.dsta = airport(t, 12);
        }
        "8D" => {
            if !at_is(t, 4, b',') || !at_is(t, 35, b',') {
                return None;
            }
            o.depa = airport(t, 36);
            if !at_is(t, 40, b',') {
                return None;
            }
            o.dsta = airport(t, 41);
        }
        "8E" | "8S" => {
            if !at_is(t, 4, b',') {
                return None;
            }
            o.dsta = airport(t, 0);
            o.eta = time4(t, 5);
        }
        _ => return None,
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qf_off_destination_report() {
        let o = decode("QF", "KEWR2210KATL").unwrap();
        assert_eq!(o.depa.as_deref(), Some("KEWR"));
        assert_eq!(o.wloff.as_deref(), Some("2210"));
        assert_eq!(o.dsta.as_deref(), Some("KATL"));
    }

    #[test]
    fn qf_three_char_faa_codes_rejected() {
        let o = decode("QF", "EWR2210ATL");
        if let Some(o) = o {
            assert!(o.wloff.is_none(), "must not emit garbage OFF time");
            assert!(o.dsta.is_none(), "must not emit truncated dest");
        }
    }

    #[test]
    fn qq_off_report() {
        let o = decode("QQ", "KEWRKSWF20041942").unwrap();
        assert_eq!(o.depa.as_deref(), Some("KEWR"));
        assert_eq!(o.dsta.as_deref(), Some("KSWF"));
        assert_eq!(o.wloff.as_deref(), Some("2004"));
    }

    #[test]
    fn qq_off_report_with_destination_only() {
        let o = decode("QQ", "KEWRKDFW1829OS KDFW /FUL0306/MO 1816/APH 0000000").unwrap();
        assert_eq!(o.depa.as_deref(), Some("KEWR"));
        assert_eq!(o.dsta.as_deref(), Some("KDFW"));
        assert_eq!(o.wloff.as_deref(), Some("1829"));
    }

    #[test]
    fn q2_eta_report() {
        let o = decode("Q2", "KSFO0830").unwrap();
        assert_eq!(o.depa.as_deref(), Some("KSFO"));
        assert_eq!(o.eta.as_deref(), Some("0830"));
    }

    #[test]
    fn qp_out_report_full() {
        let o = decode("QP", "KLAXKJFK1305").unwrap();
        assert_eq!(o.depa.as_deref(), Some("KLAX"));
        assert_eq!(o.dsta.as_deref(), Some("KJFK"));
        assert_eq!(o.gtout.as_deref(), Some("1305"));
    }

    #[test]
    fn qs_in_report_full() {
        let o = decode("QS", "KLAXKJFK2247").unwrap();
        assert_eq!(o.depa.as_deref(), Some("KLAX"));
        assert_eq!(o.dsta.as_deref(), Some("KJFK"));
        assert_eq!(o.gtin.as_deref(), Some("2247"));
    }

    #[test]
    fn label_20_rst() {
        let txt = "RST0000000000000000000KORDKSFO";
        let o = decode("20", txt).unwrap();
        assert_eq!(o.depa.as_deref(), Some("KORD"));
        assert_eq!(o.dsta.as_deref(), Some("KSFO"));
    }

    #[test]
    fn non_oooi_label_is_none() {
        assert!(decode("H1", "#DFB engine data").is_none());
        assert!(decode("SA", "0EV121314V").is_none());
        assert!(decode("QF", "??").is_none());
    }

    #[test]
    fn empty_link_test_yields_nothing() {
        assert!(decode("Q0", "").is_none());
    }
}
