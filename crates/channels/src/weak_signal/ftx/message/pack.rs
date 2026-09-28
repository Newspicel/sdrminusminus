use super::{
    A27, A36, A37, A38, A42, MAX22, MAXGRID4, NTOKENS, PAYLOAD_BITS, hash22, index_of, is_grid4,
    is_token, standard_call,
};

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
