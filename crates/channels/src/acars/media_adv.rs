use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MediaAdvisory {
    pub established: bool,
    pub current_link: char,
    pub time: String,
    pub available: Vec<char>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

fn is_valid_link(c: char) -> bool {
    "VSHGC2XI".contains(c)
}

pub fn parse(text: &str) -> Option<MediaAdvisory> {
    let b: Vec<char> = text.chars().collect();
    if b.len() < 10 || b[0] != '0' {
        return None;
    }
    let established = match b[1] {
        'E' => true,
        'L' => false,
        _ => return None,
    };
    let current_link = b[2];
    if !is_valid_link(current_link) {
        return None;
    }
    let digits: String = b[3..9].iter().collect();
    if !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (hh, mm, ss) = (&digits[0..2], &digits[2..4], &digits[4..6]);
    if hh.parse::<u8>().ok()? > 23 || mm.parse::<u8>().ok()? > 59 || ss.parse::<u8>().ok()? > 59 {
        return None;
    }

    let mut available = Vec::new();
    let mut i = 9;
    while i < b.len() && b[i] != '/' {
        if !is_valid_link(b[i]) {
            return None;
        }
        available.push(b[i]);
        i += 1;
    }
    let free_text = if i < b.len() && b[i] == '/' {
        let t: String = b[i + 1..].iter().collect();
        if t.is_empty() { None } else { Some(t) }
    } else {
        None
    };

    Some(MediaAdvisory {
        established,
        current_link,
        time: format!("{hh}:{mm}:{ss}"),
        available,
        text: free_text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_established_report() {
        let m = parse("0EV121314VS/EXTRA").unwrap();
        assert!(m.established);
        assert_eq!(m.current_link, 'V');
        assert_eq!(m.time, "12:13:14");
        assert_eq!(m.available, vec!['V', 'S']);
        assert_eq!(m.text.as_deref(), Some("EXTRA"));
    }

    #[test]
    fn parses_lost_report_no_text() {
        let m = parse("0L2235959V").unwrap();
        assert!(!m.established);
        assert_eq!(m.current_link, '2');
        assert_eq!(m.time, "23:59:59");
        assert_eq!(m.available, vec!['V']);
        assert_eq!(m.text, None);
    }

    #[test]
    fn rejects_invalid() {
        assert!(parse("1EV121314V").is_none());
        assert!(parse("0EQ121314V").is_none());
        assert!(parse("0EV256060V").is_none());
        assert!(parse("short").is_none());
    }
}
