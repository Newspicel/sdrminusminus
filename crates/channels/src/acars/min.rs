use serde::Serialize;

pub fn is_downlink_block(block_id: char) -> bool {
    block_id.is_ascii_digit()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DownlinkMin {
    pub msg_num: String,
    pub msg_num_seq: char,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<u8>,
}

pub fn split_downlink(raw_min: &str) -> Option<DownlinkMin> {
    let bytes = raw_min.as_bytes();
    if bytes.len() < 4 {
        return None;
    }
    let msg_num: String = raw_min.chars().take(3).collect();
    let msg_num_seq = bytes[3] as char;
    let seq = if msg_num_seq.is_ascii_uppercase() {
        Some(msg_num_seq as u8 - b'A')
    } else {
        None
    };
    Some(DownlinkMin {
        msg_num,
        msg_num_seq,
        seq,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_id_class_matches_libacars_macro() {
        for c in '0'..='9' {
            assert!(is_downlink_block(c), "{c} should be downlink");
        }
        for c in 'A'..='Z' {
            assert!(!is_downlink_block(c), "{c} should be uplink");
        }
    }

    #[test]
    fn splits_min_like_libacars() {
        let m = split_downlink("M01A").unwrap();
        assert_eq!(m.msg_num, "M01");
        assert_eq!(m.msg_num_seq, 'A');
        assert_eq!(m.seq, Some(0));

        let m = split_downlink("M07C").unwrap();
        assert_eq!(m.msg_num, "M07");
        assert_eq!(m.msg_num_seq, 'C');
        assert_eq!(m.seq, Some(2));
    }

    #[test]
    fn fourth_char_edge_cases() {
        let m = split_downlink("D5R2").unwrap();
        assert_eq!(m.msg_num, "D5R");
        assert_eq!(m.msg_num_seq, '2');
        assert_eq!(m.seq, None);

        let m = split_downlink("S01.").unwrap();
        assert_eq!(m.msg_num, "S01");
        assert_eq!(m.msg_num_seq, '.');
        assert_eq!(m.seq, None);
    }

    #[test]
    fn rejects_short_min() {
        assert!(split_downlink("M0").is_none());
        assert!(split_downlink("").is_none());
    }

    #[test]
    fn seq_indices_span_the_alphabet() {
        assert_eq!(split_downlink("AAAA").unwrap().seq, Some(0));
        assert_eq!(split_downlink("AAAZ").unwrap().seq, Some(25));
    }
}
