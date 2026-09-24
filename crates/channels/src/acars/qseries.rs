use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QKind {
    LinkTest,
    OutReport,
    OffReport,
    OnReport,
    InReport,
    OooiReport,
    EtaReport,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QSeries {
    pub label: String,
    pub kind: QKind,
    pub description: &'static str,
}

pub fn classify(label: &str) -> Option<QSeries> {
    let b = label.as_bytes();
    if b.len() != 2 || b[0] != b'Q' {
        return None;
    }
    let c = b[1];
    let in_family = c.is_ascii_digit() && (b'0'..=b'7').contains(&c) || (b'A'..=b'X').contains(&c);
    if !in_family {
        return None;
    }

    let (kind, description) = match c {
        b'0' => (QKind::LinkTest, "ACARS Link Test"),
        b'1' => (QKind::OooiReport, "OOOI Report"),
        b'2' => (QKind::EtaReport, "ETA Report"),
        b'A' => (QKind::OutReport, "OUT Report"),
        b'B' => (QKind::OffReport, "OFF Report"),
        b'C' => (QKind::OnReport, "ON Report"),
        b'D' => (QKind::InReport, "IN Report"),
        b'E' => (QKind::OooiReport, "OUT Report (with destination)"),
        b'F' => (QKind::OffReport, "OFF Destination Report"),
        b'G' => (QKind::OooiReport, "OUT/IN Report"),
        b'H' => (QKind::OutReport, "OUT Report"),
        b'K' => (QKind::OnReport, "ON Destination Report"),
        b'L' => (QKind::InReport, "IN Report"),
        b'M' => (QKind::Other, "Destination Report"),
        b'N' => (QKind::EtaReport, "ETA Report"),
        b'P' => (QKind::OutReport, "OUT Report"),
        b'Q' => (QKind::OffReport, "OFF Report"),
        b'R' => (QKind::OnReport, "ON Report"),
        b'S' => (QKind::InReport, "IN Report"),
        b'T' => (QKind::OooiReport, "OUT/IN Report"),
        _ => (QKind::Other, "Link control / squitter"),
    };

    Some(QSeries {
        label: label.to_owned(),
        kind,
        description,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_labels_match_airframes_wording() {
        assert_eq!(classify("Q0").unwrap().description, "ACARS Link Test");
        assert_eq!(classify("Q2").unwrap().description, "ETA Report");
        assert_eq!(
            classify("QF").unwrap().description,
            "OFF Destination Report"
        );
        assert_eq!(classify("QP").unwrap().description, "OUT Report");
        assert_eq!(classify("QQ").unwrap().description, "OFF Report");
        assert_eq!(classify("QR").unwrap().description, "ON Report");
        assert_eq!(classify("QS").unwrap().description, "IN Report");
    }

    #[test]
    fn link_test_is_classified() {
        let q = classify("Q0").unwrap();
        assert_eq!(q.kind, QKind::LinkTest);
        assert_eq!(q.label, "Q0");
    }

    #[test]
    fn oooi_event_labels_classified_from_acarsdec_table() {
        assert_eq!(classify("QA").unwrap().kind, QKind::OutReport);
        assert_eq!(classify("QB").unwrap().kind, QKind::OffReport);
        assert_eq!(classify("QC").unwrap().kind, QKind::OnReport);
        assert_eq!(classify("QD").unwrap().kind, QKind::InReport);
    }

    #[test]
    fn family_bounds() {
        assert!(classify("Q7").is_some());
        assert!(classify("QX").is_some());
        assert!(classify("Q8").is_none());
        assert!(classify("Q9").is_none());
        assert!(classify("QY").is_none());
        assert!(classify("QZ").is_none());
        assert!(classify("H1").is_none());
        assert!(classify("Q").is_none());
        assert!(classify("Q0X").is_none());
    }
}
