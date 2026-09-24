pub fn extract(text: &str, downlink: bool) -> (Option<String>, Option<String>, &str) {
    let b = text.as_bytes();
    let marked = if downlink {
        (b.len() >= 4 && b[0] == b'#' && b[3] == b'B').then_some((1..3, 4))
    } else {
        (b.len() >= 5 && text.starts_with("- #")).then_some((3..5, 5))
    };
    let (sublabel, mut consumed) = marked
        .and_then(|(range, end)| Some((text.get(range)?.to_owned(), end)))
        .map_or((None, 0), |(sublabel, end)| (Some(sublabel), end));

    let mut mfi = None;
    if sublabel.is_some() {
        let rest = &b[consumed..];
        if rest.len() >= 4 && rest[0] == b'/' && rest[3] == b' ' {
            mfi = text.get(consumed + 1..consumed + 3).map(str::to_owned);
            consumed += 4;
        }
    }
    (sublabel, mfi, text.get(consumed..).unwrap_or(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downlink_sublabel_and_mfi() {
        let (s, m, rest) = extract("#DFB/M1 POSRPT", true);
        assert_eq!(s.as_deref(), Some("DF"));
        assert_eq!(m.as_deref(), Some("M1"));
        assert_eq!(rest, "POSRPT");
    }

    #[test]
    fn downlink_sublabel_only() {
        let (s, m, rest) = extract("#M1BPOSRPT", true);
        assert_eq!(s.as_deref(), Some("M1"));
        assert_eq!(m, None);
        assert_eq!(rest, "POSRPT");
    }

    #[test]
    fn uplink_sublabel() {
        let (s, m, rest) = extract("- #MDTEXT", false);
        assert_eq!(s.as_deref(), Some("MD"));
        assert_eq!(m, None);
        assert_eq!(rest, "TEXT");
    }

    #[test]
    fn no_sublabel_passthrough() {
        let (s, m, rest) = extract("PLAIN TEXT", true);
        assert_eq!(s, None);
        assert_eq!(m, None);
        assert_eq!(rest, "PLAIN TEXT");
    }
}
