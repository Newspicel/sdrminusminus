use crate::acars::codec;

pub fn parse(text: &str) -> Option<serde_json::Value> {
    let mut ptr = text;
    loop {
        let b = ptr.as_bytes();
        if b.len() >= 13 && b[0] == b'/' && b[8] == b'.' {
            ptr = ptr.get(9..)?;
        } else if b.len() >= 8 && b[0] == b'/' && b[3] == b'.' {
            ptr = ptr.get(4..)?;
        }
        if !(ptr.starts_with("OHMA") || ptr.starts_with("RYKO")) {
            return None;
        }
        let prefix_len = text.len() - ptr.len() + 4;
        let payload = ptr.get(4..)?;
        if let Some(pos) = payload.find(text.get(..prefix_len)?) {
            ptr = payload.get(pos..)?;
            continue;
        }
        let cleaned: Vec<u8> = payload
            .bytes()
            .filter(|c| !matches!(c, b'\r' | b'\n'))
            .collect();
        let bin = codec::base64_decode(&cleaned)?;
        let inflated = codec::inflate_zlib(&bin)?;
        return serde_json::from_slice(&inflated).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acars::codec::testing::{base64_encode, zlib};

    fn make_ohma(json: &str, prefix: &str) -> String {
        format!("{prefix}OHMA{}", base64_encode(&zlib(json.as_bytes())))
    }

    #[test]
    fn short_form_decodes() {
        let v = parse(&make_ohma(r#"{"version":1,"message":{"sysid":"APU"}}"#, "")).unwrap();
        assert_eq!(v["message"]["sysid"], "APU");
    }

    #[test]
    fn long_form_prefix_skipped() {
        let v = parse(&make_ohma(r#"{"a":2}"#, "/RTNBOCR.")).unwrap();
        assert_eq!(v["a"], 2);
    }

    #[test]
    fn uplink_prefix_skipped() {
        let v = parse(&make_ohma(r#"{"b":3}"#, "/O2.")).unwrap();
        assert_eq!(v["b"], 3);
    }

    #[test]
    fn line_breaks_inside_the_payload_are_ignored() {
        let text = make_ohma(r#"{"c":4}"#, "");
        let (head, tail) = text.split_at(10);
        let v = parse(&format!("{head}\r\n{tail}")).unwrap();
        assert_eq!(v["c"], 4);
    }

    #[test]
    fn non_ohma_rejected() {
        assert!(parse("#DFB engine report").is_none());
        assert!(parse("OHMAnot-base64!!").is_none());
    }
}
