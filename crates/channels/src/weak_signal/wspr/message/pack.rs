use super::{A37, POWERS, hash};

pub(crate) fn pack(text: &str) -> Option<u64> {
    match text.split_whitespace().collect::<Vec<_>>()[..] {
        [call, grid, power] if !call.starts_with('<') => {
            let power: i32 = power.parse().ok()?;
            if !POWERS.contains(&power) {
                return None;
            }
            Some(
                u64::from(pack_call(call)?) << 22
                    | u64::from(pack_grid(grid)? << 7 | (power + 64) as u32),
            )
        }
        [call, grid, power] => {
            let call = call.strip_prefix('<')?.strip_suffix('>')?;
            let power: i32 = power.parse().ok()?;
            if grid.len() != 6 || !POWERS.contains(&power) {
                return None;
            }
            let rotated = format!("{}{}", &grid[1..], &grid[..1]);
            Some(
                u64::from(pack_call(&rotated)?) << 22
                    | u64::from(hash(call) << 7 | (64 - power - 1) as u32),
            )
        }
        [call, power] => {
            let power: i32 = power.parse().ok()?;
            let (base, affix) = split_affix(call)?;
            let extra = affix / 32_768 + 1;
            let kind = power + extra as i32;
            Some(
                u64::from(pack_call(base)?) << 22
                    | u64::from((affix % 32_768) << 7 | (kind + 64) as u32),
            )
        }
        _ => None,
    }
}

fn split_affix(call: &str) -> Option<(&str, u32)> {
    let (left, right) = call.split_once('/')?;
    if right.len() <= 2 && left.len() > right.len() {
        let value = match right.as_bytes() {
            [digit] if digit.is_ascii_digit() => u32::from(digit - b'0'),
            [letter] if letter.is_ascii_uppercase() => u32::from(letter - b'A') + 10,
            [tens, ones] if tens.is_ascii_digit() && ones.is_ascii_digit() => {
                u32::from(tens - b'0') * 10 + u32::from(ones - b'0') + 26
            }
            _ => return None,
        };
        return Some((left, 60_000 + value));
    }
    let value = format!("{left:>3}").bytes().try_fold(0u32, |acc, byte| {
        Some(acc * 37 + A37.iter().position(|&c| c == byte)? as u32)
    })?;
    Some((right, value))
}

fn pack_call(call: &str) -> Option<u32> {
    let bytes = call.as_bytes();
    let aligned = if bytes.len() >= 3 && bytes[2].is_ascii_digit() {
        format!("{call:<6}")
    } else if bytes.len() >= 2 && bytes[1].is_ascii_digit() {
        format!(" {call:<5}")
    } else {
        return None;
    };
    let c = aligned.as_bytes();
    if c.len() != 6 {
        return None;
    }
    let code = |byte: u8| {
        A37.iter()
            .position(|&candidate| candidate == byte)
            .map(|index| index as u32)
    };
    let mut value = code(c[0])?;
    value = value * 36 + code(c[1]).filter(|&index| index < 36)?;
    value = value * 10 + code(c[2]).filter(|&index| index < 10)?;
    for &byte in &c[3..] {
        value = value * 27 + code(byte).filter(|&index| index >= 10)? - 10;
    }
    Some(value)
}

fn pack_grid(grid: &str) -> Option<u32> {
    let bytes = grid.as_bytes();
    if bytes.len() != 4 {
        return None;
    }
    let field = |byte: u8| {
        (b'A'..=b'R')
            .contains(&byte)
            .then(|| u32::from(byte - b'A'))
    };
    let digit = |byte: u8| byte.is_ascii_digit().then(|| u32::from(byte - b'0'));
    let longitude = 179 - 10 * field(bytes[0])? - digit(bytes[2])?;
    let latitude = 10 * field(bytes[1])? + digit(bytes[3])?;
    Some(longitude * 180 + latitude)
}
