const NOT_DISPLAYED: char = '\u{0}';

#[rustfmt::skip]
const EBU_LATIN: [char; 256] = [
    NOT_DISPLAYED, 'Ę', 'Į', 'Ų', 'Ă', 'Ė', 'Ď', 'Ș', 'Ț', 'Ċ', NOT_DISPLAYED, NOT_DISPLAYED, 'Ġ', 'Ĺ', 'Ż', 'Ń',
    'ą', 'ę', 'į', 'ų', 'ă', 'ė', 'ď', 'ș', 'ț', 'ċ', 'Ň', 'Ě', 'ġ', 'ĺ', 'ż', NOT_DISPLAYED,
    ' ', '!', '"', '#', 'ł', '%', '&', '\u{27}', '(', ')', '*', '+', ',', '-', '.', '/',
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', ':', ';', '<', '=', '>', '?',
    '@', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O',
    'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '[', 'Ů', ']', 'Ł', '_',
    'Ą', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o',
    'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', '«', 'ů', '»', 'Ľ', 'Ħ',
    'á', 'à', 'é', 'è', 'í', 'ì', 'ó', 'ò', 'ú', 'ù', 'Ñ', 'Ç', 'Ş', 'ß', '¡', 'Ÿ',
    'â', 'ä', 'ê', 'ë', 'î', 'ï', 'ô', 'ö', 'û', 'ü', 'ñ', 'ç', 'ş', 'ğ', 'ı', 'ÿ',
    'Ķ', 'Ņ', '©', 'Ģ', 'Ğ', 'ě', 'ň', 'ő', 'Ő', '€', '£', '$', 'Ā', 'Ē', 'Ī', 'Ū',
    'ķ', 'ņ', 'Ļ', 'ģ', 'ļ', 'İ', 'ń', 'ű', 'Ű', '¿', 'ľ', '°', 'ā', 'ē', 'ī', 'ū',
    'Á', 'À', 'É', 'È', 'Í', 'Ì', 'Ó', 'Ò', 'Ú', 'Ù', 'Ř', 'Č', 'Š', 'Ž', 'Ð', 'Ŀ',
    'Â', 'Ä', 'Ê', 'Ë', 'Î', 'Ï', 'Ô', 'Ö', 'Û', 'Ü', 'ř', 'č', 'š', 'ž', 'đ', 'ŀ',
    'Ã', 'Å', 'Æ', 'Œ', 'ŷ', 'Ý', 'Õ', 'Ø', 'Þ', 'Ŋ', 'Ŕ', 'Ć', 'Ś', 'Ź', 'Ť', 'ð',
    'ã', 'å', 'æ', 'œ', 'ŵ', 'ý', 'õ', 'ø', 'þ', 'ŋ', 'ŕ', 'ć', 'ś', 'ź', 'ť', 'ħ',
];

pub fn ebu_text(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&byte| EBU_LATIN[usize::from(byte)])
        .filter(|&character| character != NOT_DISPLAYED)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_positions_map_to_themselves() {
        let ascii: Vec<u8> = (0x20..=0x7a)
            .filter(|byte| ![0x24, 0x5c, 0x5e, 0x60].contains(byte))
            .collect();
        let expected: String = ascii.iter().map(|&byte| char::from(byte)).collect();
        assert_eq!(ebu_text(&ascii), expected);
    }

    #[test]
    fn annex_c_spot_checks() {
        for (byte, expected) in [
            (0x01, 'Ę'),
            (0x1e, 'ż'),
            (0x24, 'ł'),
            (0x5c, 'Ů'),
            (0x5e, 'Ł'),
            (0x60, 'Ą'),
            (0x7b, '«'),
            (0x7f, 'Ħ'),
            (0x8d, 'ß'),
            (0x91, 'ä'),
            (0xa9, '€'),
            (0xab, '$'),
            (0xbb, '°'),
            (0xd3, 'Ë'),
            (0xd7, 'Ö'),
            (0xe3, 'Œ'),
            (0xff, 'ħ'),
        ] {
            assert_eq!(ebu_text(&[byte]), expected.to_string(), "{byte:#04x}");
        }
    }

    #[test]
    fn reserved_codes_are_not_displayed() {
        assert_eq!(ebu_text(&[b'A', 0x00, 0x0a, 0x0b, 0x1f, b'B']), "AB");
    }

    #[test]
    fn every_other_code_is_displayed_once() {
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(ebu_text(&all).chars().count(), 252);
    }
}
