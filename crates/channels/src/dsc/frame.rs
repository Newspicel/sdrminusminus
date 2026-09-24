use super::{
    demod::PHASING,
    message::{self, DscMessage, Format},
    symbol::{ERASURE, LEADING_DX_PHASING, RX_DELAY, SYMBOL_BITS, decode_symbol, zero_count},
};

const CHAR_PAIR_BITS: usize = 2 * SYMBOL_BITS;
const FIRST_RX_PHASING: u8 = 111;
const RX_PHASING: usize = LEADING_DX_PHASING + RX_DELAY;
const MAX_FILL_DISTANCE: u32 = 1;

pub struct Recovered {
    pub symbols: Vec<i32>,
    pub certain: Vec<bool>,
}

pub struct Received<'a> {
    pub hard: &'a [u8],
    pub soft: &'a [f32],
}

impl Received<'_> {
    fn chars(&self, start: usize) -> usize {
        self.hard.len().saturating_sub(start) / SYMBOL_BITS
    }

    fn hard_char(&self, start: usize, index: usize) -> Option<&[u8; SYMBOL_BITS]> {
        self.hard
            .get(start + index * SYMBOL_BITS..)
            .and_then(<[u8]>::first_chunk::<SYMBOL_BITS>)
    }

    fn soft_char(&self, start: usize, index: usize) -> Option<&[f32; SYMBOL_BITS]> {
        self.soft
            .get(start + index * SYMBOL_BITS..)
            .and_then(<[f32]>::first_chunk::<SYMBOL_BITS>)
    }

    fn valid_char(&self, start: usize, index: usize) -> Option<u8> {
        match self.hard_char(start, index).map(decode_symbol) {
            Some((value, true)) => Some(value),
            _ => None,
        }
    }

    fn combined(&self, start: usize, dx: usize, rx: usize) -> Option<[u8; SYMBOL_BITS]> {
        let dx = self.soft_char(start, dx)?;
        let rx = self.soft_char(start, rx)?;
        Some(std::array::from_fn(|bit| {
            u8::from(dx[bit] + rx[bit] >= 0.0)
        }))
    }

    fn data_symbol(&self, start: usize, data: usize) -> (i32, bool) {
        let dx = 2 * (LEADING_DX_PHASING + data);
        let rx = 2 * (RX_PHASING + data) + 1;
        match (self.valid_char(start, dx), self.valid_char(start, rx)) {
            (Some(dx), Some(rx)) => (i32::from(dx), dx == rx),
            (Some(value), None) | (None, Some(value)) => (i32::from(value), true),
            (None, None) => match self.combined(start, dx, rx).as_ref().map(decode_symbol) {
                Some((value, true)) => (i32::from(value), false),
                _ => (ERASURE, false),
            },
        }
    }

    pub fn symbols(&self, start: usize) -> Recovered {
        let dx_chars = self.chars(start).div_ceil(2);
        let (symbols, certain) = (0..dx_chars.saturating_sub(LEADING_DX_PHASING))
            .map(|data| self.data_symbol(start, data))
            .unzip();
        Recovered { symbols, certain }
    }

    pub fn phasing_score(&self, start: usize) -> usize {
        let dx = (0..LEADING_DX_PHASING)
            .filter(|&index| self.valid_char(start, 2 * index) == Some(PHASING))
            .count();
        let rx = (0..RX_PHASING)
            .filter(|&index| {
                self.valid_char(start, 2 * index + 1) == Some(FIRST_RX_PHASING - index as u8)
            })
            .count();
        dx + rx
    }

    fn fill_distance(&self, start: usize, data: usize, value: i32) -> Option<u32> {
        let dx = 2 * (LEADING_DX_PHASING + data);
        let rx = 2 * (RX_PHASING + data) + 1;
        let received = self
            .combined(start, dx, rx)
            .or_else(|| self.hard_char(start, dx).copied())?;
        let encoded = encode_symbol(value as u8);
        Some(
            received
                .iter()
                .zip(&encoded)
                .map(|(a, b)| u32::from(a != b))
                .sum(),
        )
    }
}

pub fn encode_symbol(value: u8) -> [u8; SYMBOL_BITS] {
    let check = zero_count(value);
    std::array::from_fn(|bit| match bit {
        0..7 => (value >> bit) & 1,
        _ => (check >> (9 - bit)) & 1,
    })
}

pub fn ecc_index(format: Format) -> Option<usize> {
    match format {
        Format::DistressAlert | Format::AllShipsCall => Some(17),
        Format::IndividualStationCall | Format::GeographicAreaGroupCall => Some(22),
        Format::GroupCall | Format::AutomaticServiceCall | Format::Unknown => None,
    }
}

pub fn frame_bits(message: &DscMessage) -> usize {
    let data = ecc_index(message.format).map_or(message.symbols.len(), |ecc| ecc + 1);
    CHAR_PAIR_BITS * (LEADING_DX_PHASING + data)
}

fn with_format_from_second_copy(symbols: &[i32]) -> Option<DscMessage> {
    let (&first, &second) = (symbols.first()?, symbols.get(1)?);
    if first == second || second == ERASURE {
        return None;
    }
    let mut repaired = symbols.to_vec();
    repaired[0] = second;
    Some(message::decode(&repaired)).filter(DscMessage::ecc_ok)
}

fn with_single_doubt_resolved(
    received: &Received,
    start: usize,
    recovered: &Recovered,
    format: Format,
) -> Option<DscMessage> {
    let ecc_at = ecc_index(format)?;
    let symbols = &recovered.symbols;
    let ecc = *symbols.get(ecc_at).filter(|&&ecc| ecc != ERASURE)?;
    let mut doubtful =
        (1..ecc_at).filter(|&index| !recovered.certain.get(index).copied().unwrap_or(false));
    let position = doubtful.next()?;
    if doubtful.next().is_some() {
        return None;
    }
    let parity = (1..ecc_at)
        .filter(|&index| index != position)
        .fold(ecc, |parity, index| parity ^ symbols[index])
        & 0x7f;
    if received.fill_distance(start, position, parity)? > MAX_FILL_DISTANCE {
        return None;
    }
    let mut repaired = symbols.clone();
    repaired[position] = parity;
    Some(message::decode(&repaired)).filter(DscMessage::ecc_ok)
}

pub fn decode_at(received: &Received, start: usize) -> DscMessage {
    let recovered = received.symbols(start);
    if let Some(protected) = with_format_from_second_copy(&recovered.symbols) {
        return protected;
    }
    let decoded = message::decode(&recovered.symbols);
    if decoded.ecc_ok() {
        return decoded;
    }
    with_single_doubt_resolved(received, start, &recovered, decoded.format).unwrap_or(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsc::modulate::m493_bits;

    const CALL: &[i32] = &[
        112, 112, 25, 58, 5, 99, 70, 107, 4, 52, 60, 13, 7, 12, 52, 109, 127, 52, 127, 127,
    ];

    fn soft_of(bits: &[u8]) -> Vec<f32> {
        bits.iter()
            .map(|&bit| if bit == 1 { 1.0 } else { -1.0 })
            .collect()
    }

    fn corrupt(bits: &mut [u8], soft: &mut [f32], char_index: usize, bit: usize) {
        let at = char_index * SYMBOL_BITS + bit;
        bits[at] ^= 1;
        soft[at] = if bits[at] == 1 { 0.1 } else { -0.1 };
    }

    #[test]
    fn encode_symbol_inverts_decode() {
        for value in 0..128u8 {
            assert_eq!(decode_symbol(&encode_symbol(value)), (value, true));
        }
    }

    #[test]
    fn phasing_score_counts_both_streams() {
        let bits = m493_bits(CALL);
        let soft = soft_of(&bits);
        let received = Received {
            hard: &bits,
            soft: &soft,
        };
        assert_eq!(received.phasing_score(0), 14);
        assert!(received.phasing_score(CHAR_PAIR_BITS) < 8);
    }

    #[test]
    fn a_lost_symbol_is_rebuilt_from_the_ecc() {
        let mut bits = m493_bits(CALL);
        let mut soft = soft_of(&bits);
        let data = 5;
        let dx = 2 * (LEADING_DX_PHASING + data);
        let rx = 2 * (RX_PHASING + data) + 1;
        corrupt(&mut bits, &mut soft, dx, 3);
        corrupt(&mut bits, &mut soft, rx, 3);
        let received = Received {
            hard: &bits,
            soft: &soft,
        };
        assert_eq!(received.symbols(0).symbols[data], ERASURE);
        let message = decode_at(&received, 0);
        assert!(message.ecc_ok());
        assert_eq!(&message.symbols[..CALL.len()], CALL);
    }

    #[test]
    fn soft_combining_recovers_two_damaged_copies() {
        let mut bits = m493_bits(CALL);
        let mut soft = soft_of(&bits);
        let data = 3;
        let dx = 2 * (LEADING_DX_PHASING + data);
        let rx = 2 * (RX_PHASING + data) + 1;
        corrupt(&mut bits, &mut soft, dx, 1);
        corrupt(&mut bits, &mut soft, rx, 6);
        let received = Received {
            hard: &bits,
            soft: &soft,
        };
        let recovered = received.symbols(0);
        assert_eq!(recovered.symbols[data], CALL[data]);
        assert!(!recovered.certain[data]);
    }

    #[test]
    fn the_second_format_copy_wins_when_the_first_is_wrong() {
        let mut symbols = CALL.to_vec();
        symbols[0] = 116;
        assert_eq!(message::decode(&symbols).format, Format::AllShipsCall);
        let repaired = with_format_from_second_copy(&symbols).expect("repaired");
        assert_eq!(repaired.format, Format::DistressAlert);
    }
}
