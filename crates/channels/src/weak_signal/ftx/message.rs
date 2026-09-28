use std::collections::HashMap;

pub(crate) const PAYLOAD_BITS: u32 = 77;

const NTOKENS: u32 = 2_063_592;
const MAX22: u32 = 4_194_304;
const MAXGRID4: u32 = 32_400;
const CQ_LETTERS_END: u32 = 1_003 + 27 * 27 * 27 * 27;
const HASH_PRIME: u64 = 47_055_833_459;
const BOOK_LIMIT: usize = 4_096;

const A37: &[u8] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const A36: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const A27: &[u8] = b" ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const A38: &[u8] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ/";
const A42: &[u8] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ+-./?";

const SECTIONS: [&str; 84] = [
    "AB", "AK", "AL", "AR", "AZ", "BC", "CO", "CT", "DE", "EB", "EMA", "ENY", "EPA", "EWA", "GA",
    "GTA", "IA", "ID", "IL", "IN", "KS", "KY", "LA", "LAX", "MAR", "MB", "MDC", "ME", "MI", "MN",
    "MO", "MS", "MT", "NC", "ND", "NE", "NFL", "NH", "NL", "NLI", "NM", "NNJ", "NNY", "NT", "NTX",
    "NV", "OH", "OK", "ONE", "ONN", "ONS", "OR", "ORG", "PAC", "PR", "QC", "RI", "SB", "SC", "SCV",
    "SD", "SDG", "SF", "SFL", "SJV", "SK", "SNJ", "STX", "SV", "TN", "UT", "VA", "VI", "VT", "WCF",
    "WI", "WMA", "WNY", "WPA", "WTX", "WV", "WWA", "WY", "DX",
];

const STATES: [&str; 65] = [
    "AL", "AK", "AZ", "AR", "CA", "CO", "CT", "DE", "FL", "GA", "HI", "ID", "IL", "IN", "IA", "KS",
    "KY", "LA", "ME", "MD", "MA", "MI", "MN", "MS", "MO", "MT", "NE", "NV", "NH", "NJ", "NM", "NY",
    "NC", "ND", "OH", "OK", "OR", "PA", "RI", "SC", "SD", "TN", "TX", "UT", "VT", "VA", "WA", "WV",
    "WI", "WY", "NB", "NS", "QC", "ON", "MB", "SK", "AB", "BC", "NWT", "NF", "LB", "NU", "YT",
    "PEI", "DC",
];

#[derive(Default)]
pub(crate) struct CallBook {
    by22: HashMap<u32, String>,
    by12: HashMap<u32, String>,
    by10: HashMap<u32, String>,
}

impl CallBook {
    pub(crate) fn remember(&mut self, call: &str) {
        let Some(n22) = hash22(call) else {
            return;
        };
        if self.by22.len() >= BOOK_LIMIT {
            self.by22.clear();
            self.by12.clear();
            self.by10.clear();
        }
        self.by22.insert(n22, call.to_owned());
        self.by12.insert(n22 >> 10, call.to_owned());
        self.by10.insert(n22 >> 12, call.to_owned());
    }

    fn bracketed(found: Option<&String>) -> String {
        found.map_or_else(|| "<...>".to_owned(), |call| format!("<{call}>"))
    }

    fn lookup22(&self, n22: u32) -> String {
        Self::bracketed(self.by22.get(&n22))
    }

    fn lookup12(&self, n12: u32) -> String {
        Self::bracketed(self.by12.get(&n12))
    }

    fn lookup10(&self, n10: u32) -> String {
        Self::bracketed(self.by10.get(&n10))
    }
}

pub(crate) fn hash22(call: &str) -> Option<u32> {
    let bytes = call.as_bytes();
    if bytes.len() > 11 {
        return None;
    }
    let mut n = 0u64;
    for index in 0..11 {
        let byte = bytes.get(index).copied().unwrap_or(b' ');
        n = 38 * n + index_of(A38, byte)? as u64;
    }
    Some((HASH_PRIME.wrapping_mul(n) >> (64 - 22)) as u32)
}

fn index_of(alphabet: &[u8], byte: u8) -> Option<u32> {
    alphabet
        .iter()
        .position(|&candidate| candidate == byte)
        .map(|index| index as u32)
}

struct Reader {
    bits: u128,
    left: u32,
}

impl Reader {
    fn new(bits: u128) -> Self {
        Self {
            bits,
            left: PAYLOAD_BITS,
        }
    }

    fn take(&mut self, count: u32) -> u64 {
        self.left -= count;
        ((self.bits >> self.left) & ((1u128 << count) - 1)) as u64
    }

    fn take32(&mut self, count: u32) -> u32 {
        self.take(count) as u32
    }
}

#[derive(Default)]
struct Writer {
    bits: u128,
    used: u32,
}

impl Writer {
    fn put(mut self, value: u64, count: u32) -> Self {
        self.bits = (self.bits << count) | (u128::from(value) & ((1u128 << count) - 1));
        self.used += count;
        self
    }

    fn finish(self) -> Option<u128> {
        (self.used == PAYLOAD_BITS).then_some(self.bits)
    }
}

pub(crate) fn unpack(bits: u128, book: &mut CallBook) -> Option<String> {
    let i3 = (bits & 7) as u32;
    let n3 = ((bits >> 3) & 7) as u32;
    match (i3, n3) {
        (0, 0) => free_text(bits),
        (0, 1) => dxpedition(bits, book),
        (0, 3 | 4) => field_day(bits, book),
        (0, 5) => telemetry(bits),
        (1 | 2, _) => standard(bits, book),
        (3, _) => rtty_roundup(bits, book),
        (4, _) => nonstandard(bits, book),
        (5, _) => eu_vhf(bits, book),
        _ => None,
    }
}

fn free_text(bits: u128) -> Option<String> {
    let mut value = bits >> 6;
    let mut text = [b' '; 13];
    for slot in text.iter_mut().rev() {
        *slot = A42[(value % 42) as usize];
        value /= 42;
    }
    if value != 0 {
        return None;
    }
    let text = std::str::from_utf8(&text).ok()?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn telemetry(bits: u128) -> Option<String> {
    let value = bits >> 6;
    if value == 0 {
        return None;
    }
    Some(format!("{value:X}"))
}

fn dxpedition(bits: u128, book: &mut CallBook) -> Option<String> {
    let mut reader = Reader::new(bits);
    let first = call28(reader.take32(28), book)?;
    let second = call28(reader.take32(28), book)?;
    let hashed = book.lookup10(reader.take32(10));
    let report = 2 * reader.take32(5) as i32 - 30;
    Some(format!("{first} RR73; {second} {hashed} {report:+03}"))
}

fn field_day(bits: u128, book: &mut CallBook) -> Option<String> {
    let mut reader = Reader::new(bits);
    let first = call28(reader.take32(28), book)?;
    let second = call28(reader.take32(28), book)?;
    let roger = if reader.take(1) == 1 { "R " } else { "" };
    let mut transmitters = reader.take32(4) + 1;
    let class = char::from(b'A' + reader.take(3) as u8);
    let section = SECTIONS.get((reader.take32(7) as usize).checked_sub(1)?)?;
    if reader.take(3) == 4 {
        transmitters += 16;
    }
    Some(format!(
        "{first} {second} {roger}{transmitters}{class} {section}"
    ))
}

fn rtty_roundup(bits: u128, book: &mut CallBook) -> Option<String> {
    let mut reader = Reader::new(bits);
    let thanks = if reader.take(1) == 1 { "TU; " } else { "" };
    let first = call28(reader.take32(28), book)?;
    let second = call28(reader.take32(28), book)?;
    let roger = if reader.take(1) == 1 { "R " } else { "" };
    let report = reader.take32(3) + 2;
    let exchange = match reader.take32(13) {
        serial @ 0..8_000 => format!("{serial:04}"),
        state => STATES.get(state.checked_sub(8_001)? as usize)?.to_string(),
    };
    Some(format!(
        "{thanks}{first} {second} {roger}5{report}9 {exchange}"
    ))
}

fn nonstandard(bits: u128, book: &mut CallBook) -> Option<String> {
    let mut reader = Reader::new(bits);
    let hashed = reader.take32(12);
    let plain = call58(reader.take(58))?;
    let flip = reader.take(1) == 1;
    let reply = reader.take(2);
    let cq = reader.take(1) == 1;
    book.remember(&plain);
    if cq {
        return (reply == 0).then(|| format!("CQ {plain}"));
    }
    let hashed = book.lookup12(hashed);
    let (first, second) = if flip {
        (plain, hashed)
    } else {
        (hashed, plain)
    };
    Some(match reply {
        1 => format!("{first} {second} RRR"),
        2 => format!("{first} {second} RR73"),
        3 => format!("{first} {second} 73"),
        _ => format!("{first} {second}"),
    })
}

fn eu_vhf(bits: u128, book: &mut CallBook) -> Option<String> {
    let mut reader = Reader::new(bits);
    let first = book.lookup12(reader.take32(12));
    let second = book.lookup22(reader.take32(22));
    let roger = if reader.take(1) == 1 { "R " } else { "" };
    let report = 52 + reader.take32(3);
    let serial = reader.take32(11);
    let grid = grid6(reader.take32(25))?;
    Some(format!(
        "{first} {second} {roger}{report}{serial:04} {grid}"
    ))
}

fn standard(bits: u128, book: &mut CallBook) -> Option<String> {
    let mut reader = Reader::new(bits);
    let first_value = reader.take32(28);
    let first_suffix = reader.take(1) == 1;
    let second_value = reader.take32(28);
    let second_suffix = reader.take(1) == 1;
    let roger = reader.take(1) == 1;
    let extra = reader.take32(15);
    let suffix = if reader.take(3) == 1 { "/R" } else { "/P" };
    let first = with_suffix(call28(first_value, book)?, first_suffix, suffix, book);
    let second = with_suffix(call28(second_value, book)?, second_suffix, suffix, book);
    if is_token(&second) {
        return None;
    }
    let extra = extra_field(extra, roger)?;
    if is_token(&first) && (roger || !(extra.is_empty() || is_grid4(&extra))) {
        return None;
    }
    Some(if extra.is_empty() {
        format!("{first} {second}")
    } else {
        format!("{first} {second} {extra}")
    })
}

fn is_token(field: &str) -> bool {
    field == "CQ" || field == "QRZ" || field == "DE" || field.starts_with("CQ ")
}

fn is_grid4(field: &str) -> bool {
    let bytes = field.as_bytes();
    bytes.len() == 4
        && (b'A'..=b'R').contains(&bytes[0])
        && (b'A'..=b'R').contains(&bytes[1])
        && bytes[2].is_ascii_digit()
        && bytes[3].is_ascii_digit()
}

fn with_suffix(call: String, flagged: bool, suffix: &str, book: &mut CallBook) -> String {
    if !flagged || call.starts_with('<') || is_token(&call) {
        return call;
    }
    let call = format!("{call}{suffix}");
    book.remember(&call);
    call
}

fn extra_field(value: u32, roger: bool) -> Option<String> {
    if value < MAXGRID4 {
        let grid = grid4(value);
        return Some(if roger { format!("R {grid}") } else { grid });
    }
    let report = value - MAXGRID4;
    let text = match report {
        1 => String::new(),
        2 => "RRR".to_owned(),
        3 => "RR73".to_owned(),
        4 => "73".to_owned(),
        5..=105 => {
            let snr = report as i32 - 35;
            let snr = if snr > 50 { snr - 101 } else { snr };
            format!("{snr:+03}")
        }
        _ => return None,
    };
    Some(if roger && report >= 5 {
        format!("R{text}")
    } else if roger {
        return None;
    } else {
        text
    })
}

fn grid4(mut value: u32) -> String {
    let mut grid = [0u8; 4];
    grid[3] = b'0' + (value % 10) as u8;
    value /= 10;
    grid[2] = b'0' + (value % 10) as u8;
    value /= 10;
    grid[1] = b'A' + (value % 18) as u8;
    value /= 18;
    grid[0] = b'A' + (value % 18) as u8;
    String::from_utf8_lossy(&grid).into_owned()
}

fn grid6(mut value: u32) -> Option<String> {
    let radices = [24u32, 24, 10, 10, 18, 18];
    let bases = *b"AA00AA";
    let mut grid = [0u8; 6];
    for (index, (&radix, &base)) in radices.iter().zip(&bases).enumerate() {
        grid[5 - index] = base + (value % radix) as u8;
        value /= radix;
    }
    (value == 0).then(|| String::from_utf8_lossy(&grid).into_owned())
}

fn call28(value: u32, book: &mut CallBook) -> Option<String> {
    match value {
        0 => Some("DE".to_owned()),
        1 => Some("QRZ".to_owned()),
        2 => Some("CQ".to_owned()),
        3..=1_002 => Some(format!("CQ {:03}", value - 3)),
        1_003..CQ_LETTERS_END => cq_letters(value - 1_003),
        CQ_LETTERS_END..NTOKENS => None,
        _ if value - NTOKENS < MAX22 => Some(book.lookup22(value - NTOKENS)),
        _ => {
            let call = standard_call(value - NTOKENS - MAX22)?;
            book.remember(&call);
            Some(call)
        }
    }
}

fn cq_letters(mut value: u32) -> Option<String> {
    let mut letters = [b' '; 4];
    for slot in letters.iter_mut().rev() {
        *slot = A27[(value % 27) as usize];
        value /= 27;
    }
    let text = std::str::from_utf8(&letters).ok()?.trim_start();
    text.bytes()
        .all(|byte| byte.is_ascii_uppercase())
        .then(|| format!("CQ {text}"))
}

fn standard_call(mut value: u32) -> Option<String> {
    let mut call = [0u8; 6];
    for (index, alphabet) in [A27, A27, A27].iter().enumerate() {
        call[5 - index] = alphabet[(value % 27) as usize];
        value /= 27;
    }
    call[2] = b'0' + (value % 10) as u8;
    value /= 10;
    call[1] = A36[(value % 36) as usize];
    value /= 36;
    call[0] = *A37.get(value as usize)?;
    let text = std::str::from_utf8(&call).ok()?;
    let area = text[3..].trim_end();
    if area.contains(' ') || area.is_empty() {
        return None;
    }
    let prefix = text[..2].trim_start();
    if !prefix.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return None;
    }
    let call = text.trim();
    Some(if let Some(rest) = call.strip_prefix("3D0") {
        format!("3DA0{rest}")
    } else if call.starts_with('Q') && call.as_bytes()[1].is_ascii_uppercase() {
        format!("3X{}", &call[1..])
    } else {
        call.to_owned()
    })
}

fn call58(mut value: u64) -> Option<String> {
    let mut call = [b' '; 11];
    for slot in call.iter_mut().rev() {
        *slot = A38[(value % 38) as usize];
        value /= 38;
    }
    if value != 0 {
        return None;
    }
    let text = std::str::from_utf8(&call).ok()?.trim();
    let well_formed = text.len() >= 3
        && !text.contains(' ')
        && text.split('/').all(|part| !part.is_empty())
        && text.bytes().any(|byte| byte.is_ascii_digit())
        && text.bytes().any(|byte| byte.is_ascii_uppercase());
    well_formed.then(|| text.to_owned())
}

pub(crate) fn pack(text: &str) -> Option<u128> {
    let words: Vec<&str> = text.split_whitespace().collect();
    pack_standard(&words)
        .or_else(|| pack_nonstandard(&words))
        .or_else(|| pack_free_text(text))
}

fn pack_standard(words: &[&str]) -> Option<u128> {
    let (first, rest) = match words {
        ["CQ", modifier, rest @ ..] if cq_modifier(modifier).is_some() && !rest.is_empty() => {
            (pack28_token(&format!("CQ {modifier}"))?, rest)
        }
        [first, rest @ ..] => (pack28(first)?, rest),
        [] => return None,
    };
    let (second_word, extra) = match rest {
        [second] => (*second, &[][..]),
        [second, extra @ ..] => (*second, extra),
        [] => return None,
    };
    let second = pack28(second_word)?;
    if is_token(second_word) {
        return None;
    }
    let (roger, g15) = pack_extra(extra)?;
    let suffix = |word: &str| word.ends_with("/R") || word.ends_with("/P");
    let slash_p = words.iter().any(|word| word.ends_with("/P"));
    let slash_r = words.iter().any(|word| word.ends_with("/R"));
    if slash_p && slash_r {
        return None;
    }
    let first_word = words.first()?;
    Writer::default()
        .put(u64::from(first), 28)
        .put(u64::from(suffix(first_word)), 1)
        .put(u64::from(second), 28)
        .put(u64::from(suffix(second_word)), 1)
        .put(u64::from(roger), 1)
        .put(u64::from(g15), 15)
        .put(if slash_p { 2 } else { 1 }, 3)
        .finish()
}

fn pack_extra(extra: &[&str]) -> Option<(bool, u32)> {
    match extra {
        [] => Some((false, MAXGRID4 + 1)),
        ["R", grid] if is_grid4(grid) => Some((true, pack_grid4(grid))),
        [word] => pack_single_extra(word),
        _ => None,
    }
}

fn pack_single_extra(word: &str) -> Option<(bool, u32)> {
    match word {
        "RRR" => return Some((false, MAXGRID4 + 2)),
        "RR73" => return Some((false, MAXGRID4 + 3)),
        "73" => return Some((false, MAXGRID4 + 4)),
        _ if is_grid4(word) => return Some((false, pack_grid4(word))),
        _ => {}
    }
    let (roger, report) = match word.strip_prefix('R') {
        Some(report) => (true, report),
        None => (false, word),
    };
    if !report.starts_with(['+', '-']) {
        return None;
    }
    let snr: i32 = report.parse().ok()?;
    let value = match snr {
        -30..=50 => snr + 35,
        -50..=-31 => snr + 136,
        _ => return None,
    };
    Some((roger, MAXGRID4 + value as u32))
}

fn pack_grid4(grid: &str) -> u32 {
    let bytes = grid.as_bytes();
    let field = |index: usize| u32::from(bytes[index] - b'A');
    let digit = |index: usize| u32::from(bytes[index] - b'0');
    ((field(0) * 18 + field(1)) * 10 + digit(2)) * 10 + digit(3)
}

fn cq_modifier(word: &str) -> Option<u32> {
    let bytes = word.as_bytes();
    if bytes.len() == 3 && bytes.iter().all(u8::is_ascii_digit) {
        return word.parse::<u32>().ok().map(|value| 3 + value);
    }
    if (1..=4).contains(&bytes.len()) && bytes.iter().all(u8::is_ascii_uppercase) {
        let value = bytes
            .iter()
            .fold(0u32, |acc, &byte| acc * 27 + u32::from(byte - b'A' + 1));
        return Some(1_003 + value);
    }
    None
}

fn pack28_token(word: &str) -> Option<u32> {
    match word {
        "DE" => Some(0),
        "QRZ" => Some(1),
        "CQ" => Some(2),
        _ => cq_modifier(word.strip_prefix("CQ ")?),
    }
}

fn pack28(word: &str) -> Option<u32> {
    if let Some(token) = pack28_token(word) {
        return Some(token);
    }
    if let Some(inner) = word
        .strip_prefix('<')
        .and_then(|rest| rest.strip_suffix('>'))
    {
        return hash22(inner).map(|hash| NTOKENS + hash);
    }
    let base = word
        .strip_suffix("/R")
        .or_else(|| word.strip_suffix("/P"))
        .unwrap_or(word);
    pack_basecall(base).map(|value| NTOKENS + MAX22 + value)
}

fn pack_basecall(original: &str) -> Option<u32> {
    let call = if let Some(rest) = original.strip_prefix("3DA0") {
        format!("3D0{rest}")
    } else if original.starts_with("3X") && original.as_bytes().get(2)?.is_ascii_uppercase() {
        format!("Q{}", &original[2..])
    } else {
        original.to_owned()
    };
    let bytes = call.as_bytes();
    let aligned = if bytes.len() >= 3 && bytes[2].is_ascii_digit() && bytes.len() <= 6 {
        format!("{call:<6}")
    } else if bytes.len() >= 2 && bytes[1].is_ascii_digit() && bytes.len() <= 5 {
        format!(" {call:<5}")
    } else {
        return None;
    };
    let c = aligned.as_bytes();
    let mut value = index_of(A37, c[0])?;
    value = value * 36 + index_of(A36, c[1])?;
    value = value * 10 + u32::from(c[2].checked_sub(b'0').filter(|digit| *digit < 10)?);
    for &byte in &c[3..] {
        value = value * 27 + index_of(A27, byte)?;
    }
    (standard_call(value)? == original).then_some(value)
}

fn pack_nonstandard(words: &[&str]) -> Option<u128> {
    let (hashed, plain, flip, reply, cq) = match words {
        ["CQ", call] => ("", *call, false, 0, true),
        [first, second, rest @ ..] => {
            let reply = match rest {
                [] => 0,
                ["RRR"] => 1,
                ["RR73"] => 2,
                ["73"] => 3,
                _ => return None,
            };
            match (strip_brackets(first), strip_brackets(second)) {
                (Some(hashed), None) => (hashed, *second, false, reply, false),
                (None, Some(hashed)) => (hashed, *first, true, reply, false),
                _ => return None,
            }
        }
        _ => return None,
    };
    let n12 = if cq { 0 } else { hash22(hashed)? >> 10 };
    let mut n58 = 0u64;
    let bytes = plain.as_bytes();
    if bytes.len() > 11 || bytes.len() < 3 {
        return None;
    }
    for index in 0..11 {
        let pad = 11 - bytes.len();
        let byte = if index < pad {
            b' '
        } else {
            bytes[index - pad]
        };
        n58 = n58 * 38 + u64::from(index_of(A38, byte)?);
    }
    Writer::default()
        .put(u64::from(n12), 12)
        .put(n58, 58)
        .put(u64::from(flip), 1)
        .put(reply, 2)
        .put(u64::from(cq), 1)
        .put(4, 3)
        .finish()
}

fn strip_brackets(word: &str) -> Option<&str> {
    word.strip_prefix('<')?.strip_suffix('>')
}

fn pack_free_text(text: &str) -> Option<u128> {
    let text = text.trim();
    if text.len() > 13 {
        return None;
    }
    let mut value = 0u128;
    for byte in format!("{text:>13}").bytes() {
        value = value * 42 + u128::from(index_of(A42, byte)?);
    }
    Writer::default()
        .put((value >> 64) as u64, 7)
        .put(value as u64, 64)
        .put(0, 6)
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(text: &str) -> String {
        let bits = pack(text).unwrap_or_else(|| panic!("{text} did not pack"));
        unpack(bits, &mut CallBook::default()).unwrap_or_else(|| panic!("{text} did not unpack"))
    }

    #[test]
    fn standard_messages_survive_a_round_trip() {
        for text in [
            "CQ K1ABC FN42",
            "CQ DX K1ABC FN42",
            "CQ 042 K1ABC FN42",
            "K1ABC W9XYZ -12",
            "K1ABC W9XYZ R+05",
            "K1ABC W9XYZ R FN42",
            "K1ABC W9XYZ RRR",
            "K1ABC W9XYZ RR73",
            "K1ABC W9XYZ 73",
            "K1ABC/R W9XYZ EN37",
            "G4ABC/P PA9XYZ JO22",
            "QRZ K1ABC FN42",
            "3DA0XYZ K1ABC -40",
            "3XA0XYZ K1ABC +40",
            "ET3RFG/R IN3ADG -23",
            "CQ K1ABC",
        ] {
            assert_eq!(round_trip(text), text);
        }
    }

    #[test]
    fn free_text_and_nonstandard_calls_round_trip() {
        for text in ["TNX BOB 73 GL", "CQ PJ4/K1ABC", "HELLO?"] {
            assert_eq!(round_trip(text), text);
        }
        let mut book = CallBook::default();
        book.remember("K1ABC");
        let bits = pack("<K1ABC> PJ4/W9XYZ RR73").unwrap();
        assert_eq!(unpack(bits, &mut book).unwrap(), "<K1ABC> PJ4/W9XYZ RR73");
        let bits = pack("PJ4/W9XYZ <K1ABC> 73").unwrap();
        assert_eq!(
            unpack(bits, &mut CallBook::default()).unwrap(),
            "PJ4/W9XYZ <...> 73"
        );
    }

    #[test]
    fn a_heard_call_resolves_its_hash() {
        let mut book = CallBook::default();
        let bits = pack("<W9XYZ> K1ABC -10").unwrap();
        assert_eq!(unpack(bits, &mut book).unwrap(), "<...> K1ABC -10");
        unpack(pack("CQ W9XYZ EN37").unwrap(), &mut book).unwrap();
        assert_eq!(unpack(bits, &mut book).unwrap(), "<W9XYZ> K1ABC -10");
    }

    #[test]
    fn nonsense_combinations_are_refused() {
        let mut book = CallBook::default();
        let cq_roger =
            pack("K1ABC W9XYZ R FN42").unwrap() & !(((1u128 << 28) - 1) << 49) | (2u128 << 49);
        assert_eq!(unpack(cq_roger, &mut book), None);
        let unused_token = (600_000u128 << 49) | 1;
        assert_eq!(unpack(unused_token, &mut book), None);
        assert_eq!(unpack(7, &mut book), None);
        let oversized_text = ((1u128 << 71) - 1) << 6;
        assert_eq!(unpack(oversized_text, &mut book), None);
    }

    #[test]
    fn a_call_too_long_to_hash_has_no_hash() {
        assert!(hash22("K1ABC").is_some_and(|hash| hash < 1 << 22));
        assert_eq!(hash22("TOO-LONG-CALL"), None);
    }
}
