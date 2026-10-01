pub fn word_after(text: &str, marker: &str) -> Option<String> {
    let (_, rest) = text.split_once(marker)?;
    let word = rest
        .split_whitespace()
        .next()?
        .trim_matches(|c: char| c == ',' || c == '|');
    (!word.is_empty()).then(|| word.to_owned())
}

pub fn strip_ansi(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for end in chars.by_ref() {
                if end.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(c);
        }
    }
    plain
}

pub fn mode_s(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix('*')?.strip_suffix(';'))
        .filter(|hex| !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_ascii_uppercase)
        .collect()
}

pub fn nmea_payloads(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| line.starts_with("!AIVDM") || line.starts_with("!AIVDO"))
        .filter_map(|line| line.split(',').nth(5))
        .map(str::to_owned)
        .collect()
}

pub fn direwolf(text: &str) -> Vec<String> {
    strip_ansi(text)
        .lines()
        .filter_map(|line| {
            let rest = line.trim_start().strip_prefix('[')?;
            let (channel, packet) = rest.split_once("] ")?;
            let numbered = channel.chars().all(|c| c.is_ascii_digit() || c == '.');
            (numbered && packet.contains('>') && packet.contains(':')).then(|| packet.to_owned())
        })
        .collect()
}

pub fn multimon_aprs(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.strip_prefix("APRS: "))
        .map(str::to_owned)
        .collect()
}

pub fn multimon_pocsag(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| line.starts_with("POCSAG") && line.contains("Address:"))
        .map(|line| line.replace("<NUL>", "").trim_end().to_owned())
        .collect()
}

pub fn acarsdec(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .map(|message| {
            let field = |name: &str| message[name].as_str().unwrap_or_default().to_owned();
            format!("{} {} {}", field("tail"), field("label"), field("text"))
        })
        .collect()
}

pub fn ft8_lib(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| {
            line.split_whitespace()
                .next()
                .is_some_and(|stamp| stamp.len() == 6 && stamp.chars().all(|c| c.is_ascii_digit()))
        })
        .filter_map(|line| line.split_once('~'))
        .map(|(_, message)| message.trim().to_owned())
        .collect()
}

pub fn dsd_fme(text: &str) -> Vec<String> {
    strip_ansi(text)
        .lines()
        .filter(|line| line.contains("TGT=") && line.contains("SRC="))
        .map(|line| line.trim().to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_the_word_after_the_marker() {
        let banner = "| dump1090 ModeS Receiver      dump1090-fa 11.1 |";
        assert_eq!(word_after(banner, "dump1090-fa ").as_deref(), Some("11.1"));
        let readsb = "readsb version: 3.16.17 compiled on 260927";
        assert_eq!(word_after(readsb, "version:").as_deref(), Some("3.16.17"));
        let direwolf = "Dire Wolf Release 1.8.1, November 2025";
        assert_eq!(word_after(direwolf, "Release").as_deref(), Some("1.8.1"));
        assert_eq!(word_after("nothing", "Release"), None);
    }

    #[test]
    fn mode_s_keeps_hex_frames() {
        let out = "starting up\n*8d3ff91d99144e0c18080a682221;\n*5d780561d30828;\n*zz;\n";
        assert_eq!(
            mode_s(out),
            ["8D3FF91D99144E0C18080A682221", "5D780561D30828"]
        );
    }

    #[test]
    fn nmea_payloads_ignore_the_channel() {
        let out = "!AIVDM,1,1,,A,139Lg00P,0*3F\n!AIVDM,1,1,,B,139Lg00P,0*3C\nbanner";
        assert_eq!(nmea_payloads(out), ["139Lg00P", "139Lg00P"]);
    }

    #[test]
    fn direwolf_packets_lose_colour_and_channel() {
        let out = "\u{1b}[38;2;0;192;0m[0] DL1ABC-9>APRS,WIDE1-1:!5230.00N/01324.00E>x\n\
                   \u{1b}[38;2;0;0;0m\n[0.3] A>B:c\n1 packets decoded in 0.017 seconds.";
        assert_eq!(
            direwolf(out),
            ["DL1ABC-9>APRS,WIDE1-1:!5230.00N/01324.00E>x", "A>B:c"]
        );
    }

    #[test]
    fn multimon_lines_are_read_per_mode() {
        let out = "APRS: DL1ABC-9>APRS:!x\nPOCSAG1200: Address: 1234567  Function: 3  Alpha:   SDR-- FIXTURE<NUL>\nAFSK1200: fm";
        assert_eq!(multimon_aprs(out), ["DL1ABC-9>APRS:!x"]);
        assert_eq!(
            multimon_pocsag(out),
            ["POCSAG1200: Address: 1234567  Function: 3  Alpha:   SDR-- FIXTURE"]
        );
    }

    #[test]
    fn acarsdec_json_becomes_tail_label_text() {
        let out = "{\"tail\":\"F-GTAE\",\"label\":\"H1\",\"text\":\"#DFB\"}\n\
                   {\"tail\":\"LN-DYY\",\"label\":\"_d\"}\nexiting ...";
        assert_eq!(acarsdec(out), ["F-GTAE H1 #DFB", "LN-DYY _d "]);
    }

    #[test]
    fn ft8_lib_decodes_are_the_text_after_the_tilde() {
        let out = "Decoded 2 messages\n000000 +19.0 +1.52  906 ~  PA3EPP SP8NFO KN09\n\
                   000000 +10.5 +1.68  297 ~  <...> ON7EE JO10\n";
        assert_eq!(ft8_lib(out), ["PA3EPP SP8NFO KN09", "<...> ON7EE JO10"]);
    }

    #[test]
    fn dsd_fme_counts_frames_with_addressing() {
        let out = "Sync: VLC \u{1b}[33m\n SLOT 1 Unknown LC\n\
                   \u{1b}[32m SLOT 1 TGT=12345678 SRC=12345678 Group Call \u{1b}[0m\n";
        assert_eq!(
            dsd_fme(out),
            ["SLOT 1 TGT=12345678 SRC=12345678 Group Call"]
        );
    }
}
