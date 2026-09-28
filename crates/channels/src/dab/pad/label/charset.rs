use crate::dab::fig::ebu_text;

const EBU_LATIN: u8 = 0b0000;
const ISO_LATIN_1: u8 = 0b0100;
const UTF_16_BE: u8 = 0b0110;
const UTF_8: u8 = 0b1111;
const PREFERRED_LINE_BREAK: u8 = 0x0a;
const END_OF_HEADLINE: u8 = 0x0b;

pub fn text(bytes: &[u8], charset: u8) -> Result<String, &'static str> {
    let decoded = match charset {
        EBU_LATIN => ebu_latin(bytes),
        ISO_LATIN_1 => bytes.iter().copied().map(char::from).collect(),
        UTF_16_BE => utf_16_be(bytes)?,
        UTF_8 => std::str::from_utf8(bytes)
            .map_err(|_| "Invalid UTF-8 DAB text")?
            .to_owned(),
        _ => return Err("Unsupported DAB character set"),
    };
    Ok(presentable(&decoded))
}

fn ebu_latin(bytes: &[u8]) -> String {
    let spaced: Vec<u8> = bytes
        .iter()
        .map(|&byte| match byte {
            PREFERRED_LINE_BREAK | END_OF_HEADLINE => b' ',
            byte => byte,
        })
        .collect();
    ebu_text(&spaced)
}

fn utf_16_be(bytes: &[u8]) -> Result<String, &'static str> {
    let (units, []) = bytes.as_chunks::<2>() else {
        return Err("Odd-length UTF-16 DAB text");
    };
    char::decode_utf16(units.iter().map(|&unit| u16::from_be_bytes(unit)))
        .collect::<Result<String, _>>()
        .map_err(|_| "Invalid UTF-16 DAB text")
}

fn presentable(text: &str) -> String {
    text.chars()
        .filter_map(|character| match character {
            '\n' | '\u{b}' => Some(' '),
            character if character.is_control() => None,
            character => Some(character),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_charsets_decode() {
        assert_eq!(text(b"Radio", EBU_LATIN), Ok("Radio".to_owned()));
        assert_eq!(text(&[0x4d, 0xfc], ISO_LATIN_1), Ok("M\u{fc}".to_owned()));
        assert_eq!(
            text(&[0x00, 0x41, 0x04, 0x14], UTF_16_BE),
            Ok("A\u{414}".to_owned())
        );
        assert_eq!(text("Grüße".as_bytes(), UTF_8), Ok("Grüße".to_owned()));
    }

    #[test]
    fn control_codes_become_breaks_or_vanish() {
        assert_eq!(
            text(b"Top\x0bNews\x0aNow", UTF_8),
            Ok("Top News Now".to_owned())
        );
        assert_eq!(text(b"Top\x0bNews", EBU_LATIN), Ok("Top News".to_owned()));
        assert_eq!(text(b"long\x1fword", UTF_8), Ok("longword".to_owned()));
    }

    #[test]
    fn malformed_or_unknown_text_is_rejected() {
        assert!(text(&[0xff], UTF_8).is_err());
        assert!(text(&[0x00], UTF_16_BE).is_err());
        assert!(text(b"x", 0b0001).is_err());
    }
}
