use std::collections::HashMap;

#[cfg(any(test, feature = "test-signals"))]
mod pack;
#[cfg(any(test, feature = "test-signals"))]
pub(crate) use pack::pack;

pub(crate) const PAYLOAD_BITS: usize = 50;

const CALL_LIMIT: u32 = 262_177_560;
const GRID_LIMIT: u32 = 32_400;
const HASH_SEED: u32 = 146;
const HASH_MASK: u32 = 0x7fff;
const BOOK_LIMIT: usize = 4_096;
const POWERS: [i32; 19] = [
    0, 3, 7, 10, 13, 17, 20, 23, 27, 30, 33, 37, 40, 43, 47, 50, 53, 57, 60,
];
const A37: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ ";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Message {
    pub(crate) text: String,
    pub(crate) callsign: String,
    pub(crate) grid: Option<String>,
    pub(crate) power_dbm: i32,
}

#[derive(Default)]
pub(crate) struct CallBook {
    calls: HashMap<u32, String>,
}

impl CallBook {
    fn remember(&mut self, call: &str) {
        if self.calls.len() >= BOOK_LIMIT {
            self.calls.clear();
        }
        self.calls.insert(hash(call), call.to_owned());
    }

    fn lookup(&self, hash: u32) -> String {
        self.calls
            .get(&hash)
            .map_or_else(|| "<...>".to_owned(), |call| format!("<{call}>"))
    }
}

pub(crate) fn hash(call: &str) -> u32 {
    lookup3(call.as_bytes(), HASH_SEED) & HASH_MASK
}

fn lookup3(key: &[u8], seed: u32) -> u32 {
    let start = 0xdead_beef_u32
        .wrapping_add(key.len() as u32)
        .wrapping_add(seed);
    let (mut a, mut b, mut c) = (start, start, start);
    let mut rest = key;
    while rest.len() > 12 {
        a = a.wrapping_add(word(&rest[0..4]));
        b = b.wrapping_add(word(&rest[4..8]));
        c = c.wrapping_add(word(&rest[8..12]));
        mix(&mut a, &mut b, &mut c);
        rest = &rest[12..];
    }
    if rest.is_empty() {
        return c;
    }
    a = a.wrapping_add(word(&rest[..rest.len().min(4)]));
    if rest.len() > 4 {
        b = b.wrapping_add(word(&rest[4..rest.len().min(8)]));
    }
    if rest.len() > 8 {
        c = c.wrapping_add(word(&rest[8..]));
    }
    finish(&mut a, &mut b, &mut c);
    c
}

fn word(bytes: &[u8]) -> u32 {
    bytes.iter().enumerate().fold(0, |acc, (index, &byte)| {
        acc | u32::from(byte) << (8 * index)
    })
}

fn mix(a: &mut u32, b: &mut u32, c: &mut u32) {
    *a = a.wrapping_sub(*c) ^ c.rotate_left(4);
    *c = c.wrapping_add(*b);
    *b = b.wrapping_sub(*a) ^ a.rotate_left(6);
    *a = a.wrapping_add(*c);
    *c = c.wrapping_sub(*b) ^ b.rotate_left(8);
    *b = b.wrapping_add(*a);
    *a = a.wrapping_sub(*c) ^ c.rotate_left(16);
    *c = c.wrapping_add(*b);
    *b = b.wrapping_sub(*a) ^ a.rotate_left(19);
    *a = a.wrapping_add(*c);
    *c = c.wrapping_sub(*b) ^ b.rotate_left(4);
    *b = b.wrapping_add(*a);
}

fn finish(a: &mut u32, b: &mut u32, c: &mut u32) {
    *c = (*c ^ *b).wrapping_sub(b.rotate_left(14));
    *a = (*a ^ *c).wrapping_sub(c.rotate_left(11));
    *b = (*b ^ *a).wrapping_sub(a.rotate_left(25));
    *c = (*c ^ *b).wrapping_sub(b.rotate_left(16));
    *a = (*a ^ *c).wrapping_sub(c.rotate_left(4));
    *b = (*b ^ *a).wrapping_sub(a.rotate_left(14));
    *c = (*c ^ *b).wrapping_sub(b.rotate_left(24));
}

pub(crate) fn unpack(bits: u64, book: &mut CallBook) -> Option<Message> {
    let call_value = (bits >> 22) as u32;
    let rest = (bits & ((1 << 22) - 1)) as u32;
    let kind = (rest & 127) as i32 - 64;
    let call = call6(call_value)?;
    if kind < 0 {
        return hashed(&call, rest >> 7, -(kind + 1), book);
    }
    if POWERS.contains(&kind) {
        let grid = grid4(rest >> 7)?;
        book.remember(&call);
        return Some(Message {
            text: format!("{call} {grid} {kind}"),
            callsign: call,
            grid: Some(grid),
            power_dbm: kind,
        });
    }
    compound(&call, rest >> 7, kind, book)
}

fn hashed(call: &str, hash: u32, power_dbm: i32, book: &CallBook) -> Option<Message> {
    let bytes = call.as_bytes();
    if bytes.len() != 6 || !POWERS.contains(&power_dbm) {
        return None;
    }
    let grid = format!("{}{}", &call[5..], &call[..5]);
    let locator = grid.as_bytes();
    let field = |byte: u8| (b'A'..=b'R').contains(&byte);
    let square = |byte: u8| (b'A'..=b'X').contains(&byte);
    if !(field(locator[0])
        && field(locator[1])
        && locator[2].is_ascii_digit()
        && locator[3].is_ascii_digit()
        && square(locator[4])
        && square(locator[5]))
    {
        return None;
    }
    let callsign = book.lookup(hash);
    Some(Message {
        text: format!("{callsign} {grid} {power_dbm}"),
        callsign,
        grid: Some(grid),
        power_dbm,
    })
}

fn compound(call: &str, affix: u32, kind: i32, book: &mut CallBook) -> Option<Message> {
    if !(0..=62).contains(&kind) {
        return None;
    }
    let unit = kind % 10;
    let extra = match unit {
        1 | 2 => unit,
        4..=6 => unit - 3,
        8 | 9 => unit - 7,
        _ => return None,
    };
    let power_dbm = kind - extra;
    let callsign = with_affix(call, affix + 32_768 * (extra as u32 - 1))?;
    book.remember(&callsign);
    Some(Message {
        text: format!("{callsign} {power_dbm}"),
        callsign,
        grid: None,
        power_dbm,
    })
}

fn with_affix(call: &str, value: u32) -> Option<String> {
    if value < 60_000 {
        let mut prefix = [b' '; 3];
        let mut rest = value;
        for slot in prefix.iter_mut().rev() {
            *slot = A37[(rest % 37) as usize];
            rest /= 37;
        }
        let prefix = std::str::from_utf8(&prefix).ok()?.trim_start();
        return (!prefix.is_empty() && !prefix.contains(' ')).then(|| format!("{prefix}/{call}"));
    }
    let suffix = match value - 60_000 {
        digit @ 0..=9 => format!("{digit}"),
        letter @ 10..=35 => char::from(b'A' + (letter - 10) as u8).to_string(),
        pair @ 36..=125 => format!("{:02}", pair - 26),
        _ => return None,
    };
    Some(format!("{call}/{suffix}"))
}

fn call6(mut value: u32) -> Option<String> {
    if value >= CALL_LIMIT {
        return None;
    }
    let mut call = [0u8; 6];
    for slot in call[3..].iter_mut().rev() {
        *slot = A37[(value % 27) as usize + 10];
        value /= 27;
    }
    call[2] = A37[(value % 10) as usize];
    value /= 10;
    call[1] = A37[(value % 36) as usize];
    value /= 36;
    call[0] = A37[value as usize];
    let text = std::str::from_utf8(&call).ok()?;
    let trimmed = text.trim();
    (!trimmed.contains(' ') && trimmed.len() >= 3).then(|| trimmed.to_owned())
}

fn grid4(value: u32) -> Option<String> {
    if value >= GRID_LIMIT {
        return None;
    }
    let longitude = 179 - value / 180;
    let latitude = value % 180;
    Some(format!(
        "{}{}{}{}",
        char::from(b'A' + (longitude / 10) as u8),
        char::from(b'A' + (latitude / 10) as u8),
        longitude % 10,
        latitude % 10
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(text: &str, book: &mut CallBook) -> String {
        let bits = pack(text).unwrap_or_else(|| panic!("{text} did not pack"));
        unpack(bits, book)
            .unwrap_or_else(|| panic!("{text} did not unpack"))
            .text
    }

    #[test]
    fn every_message_type_survives_a_round_trip() {
        let mut book = CallBook::default();
        for text in [
            "K1ABC FN42 37",
            "G4ABC IO91 0",
            "VK2DEF QF56 60",
            "PJ4/K1ABC 37",
            "K1ABC/7 23",
            "K1ABC/P 10",
            "K1ABC/12 33",
            "<K1ABC> FN42AB 37",
        ] {
            assert_eq!(round_trip(text, &mut book), text);
        }
    }

    #[test]
    fn an_unheard_hash_stays_unresolved() {
        assert_eq!(
            round_trip("<W9XYZ> EN37WX 20", &mut CallBook::default()),
            "<...> EN37WX 20"
        );
    }

    #[test]
    fn the_hash_is_bob_jenkins_lookup3() {
        assert_eq!(lookup3(b"", 0), 0xdead_beef);
        assert_eq!(lookup3(b"Four score and seven years ago", 0), 0x1777_0551);
        assert_eq!(lookup3(b"Four score and seven years ago", 1), 0xcd62_8161);
    }
}
