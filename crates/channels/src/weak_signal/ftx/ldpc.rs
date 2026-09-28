use std::sync::LazyLock;

pub(crate) const N: usize = 174;
pub(crate) const K: usize = 91;
const M: usize = 83;
const PAYLOAD: usize = 77;
const CRC_POLY: u16 = 0x2757;
const CRC_BITS: u32 = 14;
const MAX_CHECK_DEGREE: usize = 7;
const WORDS: usize = 3;
const SYNDROMES: usize = 1 << CRC_BITS;
const NO_ROW: u8 = u8::MAX;
const MIN_SUM_SCALE: f32 = 0.8;
const HOPELESS_AFTER: usize = 5;
const HOPELESS_CHECKS: usize = 25;

const COLUMN_CHECKS: [[u8; 3]; N] = [
    [15, 44, 72],
    [24, 50, 61],
    [32, 57, 77],
    [0, 43, 44],
    [1, 6, 60],
    [2, 5, 53],
    [3, 34, 47],
    [4, 12, 20],
    [7, 55, 78],
    [8, 63, 68],
    [9, 18, 65],
    [10, 35, 59],
    [11, 36, 57],
    [13, 31, 42],
    [14, 62, 79],
    [16, 27, 76],
    [17, 73, 82],
    [21, 52, 80],
    [22, 29, 33],
    [23, 30, 39],
    [25, 40, 75],
    [26, 56, 69],
    [28, 48, 64],
    [2, 37, 77],
    [4, 38, 81],
    [45, 49, 72],
    [50, 51, 73],
    [54, 70, 71],
    [43, 66, 71],
    [42, 67, 77],
    [0, 31, 58],
    [1, 5, 70],
    [3, 15, 53],
    [6, 64, 66],
    [7, 29, 41],
    [8, 21, 30],
    [9, 17, 75],
    [10, 22, 81],
    [11, 27, 60],
    [12, 51, 78],
    [13, 49, 50],
    [14, 80, 82],
    [16, 28, 59],
    [18, 32, 63],
    [19, 25, 72],
    [20, 33, 39],
    [23, 26, 76],
    [24, 54, 57],
    [34, 52, 65],
    [35, 47, 67],
    [36, 45, 74],
    [37, 44, 46],
    [38, 56, 68],
    [40, 55, 61],
    [19, 48, 52],
    [45, 51, 62],
    [44, 69, 74],
    [26, 34, 79],
    [0, 14, 29],
    [1, 67, 79],
    [2, 35, 50],
    [3, 27, 50],
    [4, 30, 55],
    [5, 19, 36],
    [6, 39, 81],
    [7, 59, 68],
    [8, 9, 48],
    [10, 43, 56],
    [11, 38, 58],
    [12, 23, 54],
    [13, 20, 64],
    [15, 70, 77],
    [16, 29, 75],
    [17, 24, 79],
    [18, 60, 82],
    [21, 37, 76],
    [22, 40, 49],
    [6, 25, 57],
    [28, 31, 80],
    [32, 39, 72],
    [17, 33, 47],
    [12, 41, 63],
    [4, 25, 42],
    [46, 68, 71],
    [53, 54, 69],
    [44, 61, 67],
    [9, 62, 66],
    [13, 65, 71],
    [21, 59, 73],
    [34, 38, 78],
    [0, 45, 63],
    [0, 23, 65],
    [1, 4, 69],
    [2, 30, 64],
    [3, 48, 57],
    [0, 3, 4],
    [5, 59, 66],
    [6, 31, 74],
    [7, 47, 81],
    [8, 34, 40],
    [9, 38, 61],
    [10, 13, 60],
    [11, 70, 73],
    [12, 22, 77],
    [10, 34, 54],
    [14, 15, 78],
    [6, 8, 15],
    [16, 53, 62],
    [17, 49, 56],
    [18, 29, 46],
    [19, 63, 79],
    [20, 27, 68],
    [21, 24, 42],
    [12, 21, 36],
    [1, 46, 50],
    [22, 53, 73],
    [25, 33, 71],
    [26, 35, 36],
    [20, 35, 62],
    [28, 39, 43],
    [18, 25, 56],
    [2, 45, 81],
    [13, 14, 57],
    [32, 51, 52],
    [29, 42, 51],
    [5, 8, 51],
    [26, 32, 64],
    [24, 68, 72],
    [37, 54, 82],
    [19, 38, 76],
    [17, 28, 55],
    [31, 47, 70],
    [41, 50, 58],
    [27, 43, 78],
    [33, 59, 61],
    [30, 44, 60],
    [45, 67, 76],
    [5, 23, 75],
    [7, 9, 77],
    [39, 40, 69],
    [16, 49, 52],
    [41, 65, 67],
    [3, 21, 71],
    [35, 63, 80],
    [12, 28, 46],
    [1, 7, 80],
    [55, 66, 72],
    [4, 37, 49],
    [11, 37, 63],
    [58, 71, 79],
    [2, 25, 78],
    [44, 75, 80],
    [0, 64, 73],
    [6, 17, 76],
    [10, 55, 58],
    [13, 38, 53],
    [15, 36, 65],
    [9, 27, 54],
    [14, 59, 69],
    [16, 24, 81],
    [19, 29, 30],
    [11, 66, 67],
    [22, 74, 79],
    [26, 31, 61],
    [23, 68, 74],
    [18, 20, 70],
    [33, 52, 60],
    [34, 45, 46],
    [32, 58, 75],
    [39, 42, 82],
    [40, 41, 62],
    [48, 74, 82],
    [19, 43, 47],
    [41, 48, 56],
];

type Bits = [u64; WORDS];

struct Code {
    checks: [[u8; MAX_CHECK_DEGREE]; M],
    degrees: [u8; M],
    parity: [u128; M],
    crc_syndrome: [u16; K],
}

static CODE: LazyLock<Code> = LazyLock::new(Code::build);

impl Code {
    fn build() -> Self {
        let mut checks = [[0u8; MAX_CHECK_DEGREE]; M];
        let mut degrees = [0u8; M];
        for (bit, rows) in COLUMN_CHECKS.iter().enumerate() {
            for &row in rows {
                let row = usize::from(row);
                checks[row][usize::from(degrees[row])] = bit as u8;
                degrees[row] += 1;
            }
        }
        Self {
            checks,
            degrees,
            parity: parity_rows(),
            crc_syndrome: std::array::from_fn(|bit| {
                if bit < PAYLOAD {
                    crc14(1u128 << (PAYLOAD - 1 - bit))
                } else {
                    1u16 << (K - 1 - bit)
                }
            }),
        }
    }
}

fn parity_rows() -> [u128; M] {
    let mut left = [0u128; M];
    let mut right = [0u128; M];
    for (bit, rows) in COLUMN_CHECKS.iter().enumerate() {
        for &row in rows {
            let row = usize::from(row);
            if bit < K {
                right[row] |= 1u128 << (K - 1 - bit);
            } else {
                left[row] |= 1u128 << (bit - K);
            }
        }
    }
    for pivot in 0..M {
        let Some(found) = (pivot..M).find(|&row| left[row] >> pivot & 1 == 1) else {
            continue;
        };
        left.swap(pivot, found);
        right.swap(pivot, found);
        for row in 0..M {
            if row != pivot && left[row] >> pivot & 1 == 1 {
                left[row] ^= left[pivot];
                right[row] ^= right[pivot];
            }
        }
    }
    right
}

pub(crate) fn crc14(payload: u128) -> u16 {
    let mut register = 0u16;
    for index in 0..PAYLOAD + 5 {
        let bit = if index < PAYLOAD {
            (payload >> (PAYLOAD - 1 - index)) as u16 & 1
        } else {
            0
        };
        let feedback = (register >> (CRC_BITS - 1)) & 1 ^ bit;
        register = (register << 1) & ((1 << CRC_BITS) - 1);
        if feedback == 1 {
            register ^= CRC_POLY;
        }
    }
    register
}

pub(crate) fn encode(payload: u128) -> [u8; N] {
    let message = (payload << CRC_BITS) | u128::from(crc14(payload));
    let mut bits = [0u8; N];
    for (index, bit) in bits.iter_mut().take(K).enumerate() {
        *bit = (message >> (K - 1 - index)) as u8 & 1;
    }
    for (bit, row) in bits[K..].iter_mut().zip(&CODE.parity) {
        *bit = ((row & message).count_ones() & 1) as u8;
    }
    bits
}

fn payload_of(codeword: &[u8; N]) -> Option<u128> {
    let message = codeword[..K]
        .iter()
        .fold(0u128, |acc, &bit| (acc << 1) | u128::from(bit));
    let payload = message >> CRC_BITS;
    (crc14(payload) == (message & ((1 << CRC_BITS) - 1)) as u16).then_some(payload)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Decoded {
    pub(crate) payload: u128,
    pub(crate) hard_errors: u32,
    pub(crate) distance: f32,
    pub(crate) osd: bool,
}

pub(crate) struct Decoder {
    posterior: [f32; N],
    messages: [[f32; MAX_CHECK_DEGREE]; M],
    osd: Osd,
}

impl Decoder {
    pub(crate) fn new() -> Self {
        Self {
            posterior: [0.0; N],
            messages: [[0.0; MAX_CHECK_DEGREE]; M],
            osd: Osd::new(),
        }
    }

    pub(crate) fn decode(
        &mut self,
        llr: &[f32; N],
        iterations: usize,
        osd_order: usize,
    ) -> Option<Decoded> {
        if let Some(codeword) = self.belief_propagation(llr, iterations)
            && let Some(payload) = payload_of(&codeword)
        {
            return Some(Decoded {
                payload,
                hard_errors: hard_errors(llr, &codeword),
                distance: distance(llr, &codeword),
                osd: false,
            });
        }
        if osd_order == 0 {
            return None;
        }
        let codeword = self.osd.decode(llr, osd_order)?;
        Some(Decoded {
            payload: payload_of(&codeword)?,
            hard_errors: hard_errors(llr, &codeword),
            distance: distance(llr, &codeword),
            osd: true,
        })
    }

    fn belief_propagation(&mut self, llr: &[f32; N], iterations: usize) -> Option<[u8; N]> {
        let code = &*CODE;
        for (posterior, &value) in self.posterior.iter_mut().zip(llr) {
            *posterior = -value;
        }
        self.messages = [[0.0; MAX_CHECK_DEGREE]; M];
        let mut fewest = M;
        let mut stale = 0;
        for iteration in 0..iterations {
            for check in 0..M {
                self.update_check(code, check);
            }
            let hard: [u8; N] = std::array::from_fn(|bit| u8::from(self.posterior[bit] < 0.0));
            let unsatisfied = unsatisfied_checks(code, &hard);
            if unsatisfied == 0 {
                return Some(hard);
            }
            if iteration >= HOPELESS_AFTER && unsatisfied > HOPELESS_CHECKS {
                return None;
            }
            if unsatisfied < fewest {
                fewest = unsatisfied;
                stale = 0;
            } else {
                stale += 1;
                if stale >= 6 && iteration >= 12 {
                    return None;
                }
            }
        }
        None
    }

    fn update_check(&mut self, code: &Code, check: usize) {
        let degree = usize::from(code.degrees[check]);
        let variables = &code.checks[check][..degree];
        let stored = &mut self.messages[check];
        let mut incoming = [0f32; MAX_CHECK_DEGREE];
        let (mut first, mut second, mut weakest, mut negative) = (f32::MAX, f32::MAX, 0, false);
        for (edge, &variable) in variables.iter().enumerate() {
            let value = self.posterior[usize::from(variable)] - stored[edge];
            incoming[edge] = value;
            negative ^= value < 0.0;
            let magnitude = value.abs();
            if magnitude < first {
                second = first;
                first = magnitude;
                weakest = edge;
            } else if magnitude < second {
                second = magnitude;
            }
        }
        for (edge, &variable) in variables.iter().enumerate() {
            let magnitude = MIN_SUM_SCALE * if edge == weakest { second } else { first };
            let message = if negative ^ (incoming[edge] < 0.0) {
                -magnitude
            } else {
                magnitude
            };
            stored[edge] = message;
            self.posterior[usize::from(variable)] = incoming[edge] + message;
        }
    }
}

fn unsatisfied_checks(code: &Code, hard: &[u8; N]) -> usize {
    (0..M)
        .filter(|&check| {
            code.checks[check][..usize::from(code.degrees[check])]
                .iter()
                .fold(0u8, |acc, &bit| acc ^ hard[usize::from(bit)])
                == 1
        })
        .count()
}

fn distance(llr: &[f32; N], codeword: &[u8; N]) -> f32 {
    let total: f32 = llr.iter().map(|value| value.abs()).sum();
    if total <= 0.0 {
        return 1.0;
    }
    llr.iter()
        .zip(codeword)
        .filter(|&(&value, &bit)| (value > 0.0) != (bit == 1))
        .map(|(value, _)| value.abs())
        .sum::<f32>()
        / total
}

fn hard_errors(llr: &[f32; N], codeword: &[u8; N]) -> u32 {
    llr.iter()
        .zip(codeword)
        .filter(|&(&value, &bit)| (value > 0.0) != (bit == 1))
        .count() as u32
}

struct Osd {
    order: [u8; N],
    reliability: [f32; N],
    rows: [Bits; K],
    syndromes: [u16; K],
    heads: Vec<u8>,
    next: [u8; K],
}

impl Osd {
    fn new() -> Self {
        Self {
            order: [0; N],
            reliability: [0.0; N],
            rows: [[0; WORDS]; K],
            syndromes: [0; K],
            heads: vec![NO_ROW; SYNDROMES],
            next: [NO_ROW; K],
        }
    }

    fn decode(&mut self, llr: &[f32; N], max_order: usize) -> Option<[u8; N]> {
        let mut order: [u8; N] = std::array::from_fn(|index| index as u8);
        order.sort_unstable_by(|&a, &b| {
            llr[usize::from(b)]
                .abs()
                .total_cmp(&llr[usize::from(a)].abs())
        });
        self.order = order;
        let mut hard: Bits = [0; WORDS];
        for (position, &natural) in order.iter().enumerate() {
            let value = llr[usize::from(natural)];
            self.reliability[position] = value.abs();
            if value > 0.0 {
                set(&mut hard, position);
            }
        }
        self.permuted_generator();
        let basis = self.eliminate()?;
        let (start, syndrome) = self.reencode(&hard, &basis);
        self.index_syndromes();
        let error = self.search(xor(&start, &hard), syndrome, max_order)?;
        let codeword_permuted = xor(&hard, &error);
        let mut codeword = [0u8; N];
        for (position, &natural) in self.order.iter().enumerate() {
            codeword[usize::from(natural)] = u8::from(get(&codeword_permuted, position));
        }
        Some(codeword)
    }

    fn permuted_generator(&mut self) {
        let code = &*CODE;
        self.rows = [[0; WORDS]; K];
        self.syndromes = code.crc_syndrome;
        for (position, &natural) in self.order.iter().enumerate() {
            let natural = usize::from(natural);
            if natural < K {
                set(&mut self.rows[natural], position);
                continue;
            }
            let mut mask = code.parity[natural - K];
            while mask != 0 {
                let bit = mask.trailing_zeros() as usize;
                set(&mut self.rows[K - 1 - bit], position);
                mask &= mask - 1;
            }
        }
    }

    fn eliminate(&mut self) -> Option<[u8; K]> {
        let mut basis = [0u8; K];
        let mut pivots = 0;
        for position in 0..N {
            if pivots == K {
                break;
            }
            let Some(found) = (pivots..K).find(|&row| get(&self.rows[row], position)) else {
                continue;
            };
            self.rows.swap(pivots, found);
            self.syndromes.swap(pivots, found);
            let (pivot_row, pivot_syndrome) = (self.rows[pivots], self.syndromes[pivots]);
            for row in 0..K {
                if row != pivots && get(&self.rows[row], position) {
                    self.rows[row] = xor(&self.rows[row], &pivot_row);
                    self.syndromes[row] ^= pivot_syndrome;
                }
            }
            basis[pivots] = position as u8;
            pivots += 1;
        }
        (pivots == K).then_some(basis)
    }

    fn reencode(&self, hard: &Bits, basis: &[u8; K]) -> (Bits, u16) {
        let mut codeword = [0; WORDS];
        let mut syndrome = 0;
        for (row, &position) in basis.iter().enumerate() {
            if get(hard, usize::from(position)) {
                codeword = xor(&codeword, &self.rows[row]);
                syndrome ^= self.syndromes[row];
            }
        }
        (codeword, syndrome)
    }

    fn index_syndromes(&mut self) {
        for &syndrome in &self.syndromes {
            self.heads[usize::from(syndrome)] = NO_ROW;
        }
        for row in (0..K).rev() {
            let syndrome = usize::from(self.syndromes[row]);
            self.next[row] = self.heads[syndrome];
            self.heads[syndrome] = row as u8;
        }
    }

    fn rows_with(&self, syndrome: u16) -> impl Iterator<Item = usize> + '_ {
        let mut row = self.heads[usize::from(syndrome)];
        std::iter::from_fn(move || {
            (row != NO_ROW).then(|| {
                let current = usize::from(row);
                row = self.next[current];
                current
            })
        })
    }

    fn search(&self, error: Bits, syndrome: u16, max_order: usize) -> Option<Bits> {
        let mut best: Option<(f32, Bits)> = None;
        let mut consider = |candidate: Bits| {
            let limit = best.map_or(f32::MAX, |(distance, _)| distance);
            if let Some(distance) = self.distance(&candidate, limit) {
                best = Some((distance, candidate));
            }
        };
        if syndrome == 0 {
            consider(error);
        }
        for first in self.rows_with(syndrome) {
            consider(xor(&error, &self.rows[first]));
        }
        if max_order >= 2 {
            for first in 0..K {
                let single = xor(&error, &self.rows[first]);
                let target = syndrome ^ self.syndromes[first];
                for second in self.rows_with(target).filter(|&row| row > first) {
                    consider(xor(&single, &self.rows[second]));
                }
                if max_order < 3 {
                    continue;
                }
                for second in first + 1..K {
                    let pair = xor(&single, &self.rows[second]);
                    let target = target ^ self.syndromes[second];
                    for third in self.rows_with(target).filter(|&row| row > second) {
                        consider(xor(&pair, &self.rows[third]));
                    }
                }
            }
        }
        best.map(|(_, error)| error)
    }

    fn distance(&self, error: &Bits, limit: f32) -> Option<f32> {
        let mut total = 0.0;
        for (word_index, &word) in error.iter().enumerate() {
            let mut remaining = word;
            while remaining != 0 {
                total += self.reliability[word_index * 64 + remaining.trailing_zeros() as usize];
                if total >= limit {
                    return None;
                }
                remaining &= remaining - 1;
            }
        }
        Some(total)
    }
}

fn set(bits: &mut Bits, position: usize) {
    bits[position / 64] |= 1 << (position % 64);
}

fn get(bits: &Bits, position: usize) -> bool {
    bits[position / 64] >> (position % 64) & 1 == 1
}

fn xor(a: &Bits, b: &Bits) -> Bits {
    [a[0] ^ b[0], a[1] ^ b[1], a[2] ^ b[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn llr_for(codeword: &[u8; N], amplitude: f32) -> [f32; N] {
        std::array::from_fn(|bit| {
            if codeword[bit] == 1 {
                amplitude
            } else {
                -amplitude
            }
        })
    }

    fn noise(seed: &mut u64) -> f32 {
        let mut total = 0.0;
        for _ in 0..12 {
            *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            total += (*seed >> 40) as f32 / (1u64 << 24) as f32;
        }
        total - 6.0
    }

    #[test]
    fn every_codeword_satisfies_every_check() {
        for payload in [0u128, 1, 0x1234_5678_9abc_def0_1234, (1 << 77) - 1] {
            let codeword = encode(payload);
            assert_eq!(unsatisfied_checks(&CODE, &codeword), 0);
            assert_eq!(payload_of(&codeword), Some(payload));
        }
    }

    #[test]
    fn belief_propagation_repairs_scattered_errors() {
        let payload = 0x0abc_def0_1234_5678_9abcu128;
        let codeword = encode(payload);
        let mut llr = llr_for(&codeword, 3.0);
        for bit in [3, 40, 77, 101, 150, 170] {
            llr[bit] = -llr[bit] * 0.3;
        }
        let decoded = Decoder::new().decode(&llr, 30, 0).unwrap();
        assert_eq!(decoded.payload, payload);
        assert!(!decoded.osd);
    }

    #[test]
    fn ordered_statistics_recovers_what_belief_propagation_cannot() {
        let payload = 0x1f00_ba11_c0ff_ee00_0042u128;
        let codeword = encode(payload);
        let mut decoder = Decoder::new();
        let mut seed = 7u64;
        let mut osd_wins = 0;
        for _ in 0..40 {
            let llr: [f32; N] = std::array::from_fn(|bit| {
                let sign = if codeword[bit] == 1 { 1.0 } else { -1.0 };
                2.0 * (sign * 0.9 + 0.95 * noise(&mut seed))
            });
            let bp = decoder.decode(&llr, 30, 0).map(|decoded| decoded.payload);
            let osd = decoder.decode(&llr, 30, 3).map(|decoded| decoded.payload);
            if bp.is_none() && osd == Some(payload) {
                osd_wins += 1;
            }
            assert!(osd.is_none_or(|found| found == payload) || bp.is_none());
        }
        assert!(osd_wins > 0, "OSD never beat belief propagation");
    }
}
