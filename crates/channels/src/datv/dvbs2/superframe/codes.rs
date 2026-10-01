use num_complex::Complex;

use super::super::pl;

pub const PERIOD: usize = (1 << 20) - 1;
const OFFSET: usize = 524_288;
const WINDOW: usize = 20;
const MASK: u32 = (1 << WINDOW) - 1;

pub struct Gold {
    x: Vec<u8>,
    y: Vec<u8>,
}

impl Gold {
    #[must_use]
    pub fn new() -> Self {
        let mut x = vec![0u8; PERIOD];
        let mut y = vec![1u8; PERIOD];
        x[0] = 1;
        for i in WINDOW..PERIOD {
            x[i] = x[i - 17] ^ x[i - 20];
            y[i] = y[i - 3] ^ y[i - 9] ^ y[i - 18] ^ y[i - 20];
        }
        Self { x, y }
    }

    fn z(&self, code: usize, i: usize) -> u8 {
        self.x[(i + code) % PERIOD] ^ self.y[i % PERIOD]
    }

    pub fn quarters(&self, code: u32, out: &mut Vec<u8>) {
        let code = code as usize % PERIOD;
        out.clear();
        out.extend((0..PERIOD).map(|i| 2 * self.z(code, (i + OFFSET) % PERIOD) + self.z(code, i)));
    }

    fn turn(&self, i: usize) -> u8 {
        self.x[i % PERIOD] ^ self.x[(i + PERIOD - 1) % PERIOD]
    }

    fn y_turn(&self, i: usize) -> u8 {
        self.y[i % PERIOD] ^ self.y[(i + PERIOD - 1) % PERIOD]
    }
}

impl Default for Gold {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Sequence {
    quarters: Vec<u8>,
    code: u32,
}

impl Sequence {
    #[must_use]
    pub fn new(gold: &Gold, code: u32) -> Self {
        let mut quarters = Vec::with_capacity(PERIOD);
        gold.quarters(code, &mut quarters);
        Self { quarters, code }
    }

    #[must_use]
    pub const fn code(&self) -> u32 {
        self.code
    }

    pub fn set(&mut self, gold: &Gold, code: u32) {
        if code != self.code {
            gold.quarters(code, &mut self.quarters);
            self.code = code;
        }
    }

    #[must_use]
    pub fn quarter(&self, i: usize) -> u8 {
        self.quarters[i % PERIOD]
    }

    #[must_use]
    pub fn scramble(&self, i: usize, symbol: Complex<f32>) -> Complex<f32> {
        pl::rotate(symbol, self.quarter(i))
    }

    #[must_use]
    pub fn descramble(&self, i: usize, symbol: Complex<f32>) -> Complex<f32> {
        pl::rotate(symbol, 4 - self.quarter(i))
    }

    #[must_use]
    pub fn known(&self, i: usize, negative: bool) -> Complex<f32> {
        let symbol = self.scramble(i, pl::pilot_symbol());
        if negative { -symbol } else { symbol }
    }
}

#[must_use]
pub const fn walsh(row: usize, column: usize) -> bool {
    (row & column).count_ones() % 2 == 1
}

#[must_use]
pub const fn sosf(row: u8, k: usize) -> bool {
    let row = row as usize;
    if k < 256 {
        walsh(row, k)
    } else {
        walsh(row % 16, k - 256 + 1)
    }
}

#[must_use]
pub const fn pilot(row: u8, k: usize) -> bool {
    let row = row as usize;
    if k < 32 {
        walsh(row, k)
    } else {
        walsh(row % 4, k - 32)
    }
}

#[must_use]
pub const fn short_pilot(row: u8, k: usize) -> bool {
    let row = row as usize;
    if k < 32 {
        walsh(row, k)
    } else {
        walsh(row % 16, k - 32)
    }
}

#[cfg(any(test, feature = "synth"))]
#[must_use]
pub const fn trailer(row: u8, k: usize) -> bool {
    let row = row as usize;
    if k < 64 {
        walsh(row, k)
    } else {
        walsh(row % 32, k - 64 + 3)
    }
}

#[cfg(any(test, feature = "synth"))]
#[must_use]
pub const fn extension(row: u8, k: usize) -> bool {
    let row = row as usize;
    if k < 252 {
        walsh(row, k + 3)
    } else {
        !walsh(row, k - 252 + 3)
    }
}

pub fn hadamard(values: &mut [Complex<f32>]) {
    let mut half = 1;
    while half < values.len() {
        for block in values.chunks_exact_mut(2 * half) {
            let (low, high) = block.split_at_mut(half);
            for (a, b) in low.iter_mut().zip(high.iter_mut()) {
                let sum = *a + *b;
                *b = *a - *b;
                *a = sum;
            }
        }
        half *= 2;
    }
}

#[must_use]
pub fn strongest(values: &[Complex<f32>]) -> (usize, Complex<f32>) {
    values
        .iter()
        .copied()
        .enumerate()
        .fold((0, Complex::new(0.0, 0.0)), |best, (index, value)| {
            if value.norm_sqr() > best.1.norm_sqr() {
                (index, value)
            } else {
                best
            }
        })
}

pub struct Search {
    windows: Vec<u32>,
}

const TRIES: [usize; 4] = [0, 60, 120, 180];
const CHECK: usize = 40;

impl Search {
    #[must_use]
    pub fn new(gold: &Gold) -> Self {
        let mut windows = vec![0u32; 1 << WINDOW];
        let mut pattern = 0u32;
        for m in 0..PERIOD + WINDOW - 1 {
            pattern = (pattern << 1 | u32::from(gold.turn(m))) & MASK;
            if m >= WINDOW - 1 {
                windows[pattern as usize] = ((m + 1 - WINDOW) % PERIOD) as u32;
            }
        }
        Self { windows }
    }

    pub fn identify(
        &self,
        gold: &Gold,
        start: usize,
        count: usize,
        turns: impl Fn(usize) -> u8,
    ) -> Option<u32> {
        TRIES
            .iter()
            .filter(|&&a| a + WINDOW <= count)
            .find_map(|&a| self.attempt(gold, start, count, a, &turns))
    }

    fn attempt(
        &self,
        gold: &Gold,
        start: usize,
        count: usize,
        a: usize,
        turns: &impl Fn(usize) -> u8,
    ) -> Option<u32> {
        let observed = |k: usize| turns(k) ^ gold.y_turn(start + k);
        let pattern = (a..a + WINDOW).fold(0u32, |word, k| word << 1 | u32::from(observed(k)));
        let m = self.windows[pattern as usize] as usize;
        let code = (m + PERIOD - (start + a) % PERIOD) % PERIOD;
        let mut wrong = 0usize;
        for (checked, k) in (0..count)
            .filter(|k| !(a..a + WINDOW).contains(k))
            .enumerate()
        {
            if observed(k) != gold.turn(start + k + code) {
                wrong += 1;
            }
            if checked + 1 == CHECK && wrong * 4 > CHECK {
                return None;
            }
        }
        (wrong * 5 <= count - WINDOW).then_some(code as u32)
    }
}

#[must_use]
pub fn turn(current: Complex<f32>, previous: Complex<f32>) -> u8 {
    let product = current * previous.conj();
    u8::from(product.im.abs() > product.re.abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hadamard_transform_names_every_row() {
        for row in 0..64usize {
            let mut values: Vec<Complex<f32>> = (0..64)
                .map(|k| Complex::new(if walsh(row, k) { -1.0 } else { 1.0 }, 0.0))
                .collect();
            hadamard(&mut values);
            assert_eq!(strongest(&values).0, row);
            assert!((strongest(&values).1.re - 64.0).abs() < 1e-3);
        }
    }

    #[test]
    fn the_padding_columns_follow_annex_e() {
        assert!(!sosf(0, 269));
        assert_eq!(sosf(17, 256), walsh(1, 1));
        assert_eq!(sosf(3, 269), walsh(3, 14));
        assert_eq!(pilot(6, 34), walsh(2, 2));
        assert_eq!(short_pilot(21, 47), walsh(5, 15));
        assert_eq!(trailer(40, 64), walsh(8, 3));
        assert_eq!(trailer(40, 89), walsh(8, 28));
        assert_eq!(extension(5, 0), walsh(5, 3));
        assert_eq!(extension(5, 252), !walsh(5, 3));
        assert_eq!(extension(5, 503), !walsh(5, 254));
    }

    #[test]
    fn the_default_sequence_matches_the_two_m_sequences() {
        let gold = Gold::new();
        let sequence = Sequence::new(&gold, 0);
        for i in [0usize, 1, 19, 20, 524_287, 612_539] {
            let shifted = (i + OFFSET) % PERIOD;
            let expected = 2 * (gold.x[shifted] ^ gold.y[shifted]) + (gold.x[i] ^ gold.y[i]);
            assert_eq!(sequence.quarter(i), expected, "{i}");
        }
        assert_eq!(sequence.quarter(PERIOD), sequence.quarter(0));
        let shifted = Sequence::new(&gold, 5);
        assert_eq!(
            shifted.quarter(0) & 1,
            gold.x[5] ^ gold.y[0],
            "the payload code shifts x only"
        );
    }

    #[test]
    fn a_code_is_recovered_from_differential_turns() {
        let gold = Gold::new();
        let search = Search::new(&gold);
        for code in [0u32, 1, 77, 524_287, 1_000_000] {
            let sequence = Sequence::new(&gold, code);
            let symbols: Vec<Complex<f32>> = (0..270)
                .map(|k| sequence.known(k, walsh(5, k % 256)))
                .collect();
            let found = search.identify(&gold, 1, 269, |k| turn(symbols[k + 1], symbols[k]));
            assert_eq!(found, Some(code), "{code}");
        }
    }
}
