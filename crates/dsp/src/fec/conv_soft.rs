const MIN_K: u32 = 3;
const MAX_K: u32 = 8;
const UNREACHED: f32 = f32::NEG_INFINITY;

#[derive(Clone, Debug)]
pub struct SoftViterbi {
    k: u32,
    states: usize,
    outputs: Vec<u8>,
}

impl SoftViterbi {
    #[must_use]
    pub fn new(k: u32, g1: u32, g2: u32) -> Self {
        assert!((MIN_K..=MAX_K).contains(&k), "constraint length {k}");
        let states = 1usize << (k - 1);
        let outputs = (0..states * 2)
            .map(|index| {
                let register = ((index as u32 & 1) << (k - 1)) | (index as u32 >> 1);
                let first = (register & g1).count_ones() & 1;
                let second = (register & g2).count_ones() & 1;
                (first | second << 1) as u8
            })
            .collect();
        Self { k, states, outputs }
    }

    #[must_use]
    pub fn k7() -> Self {
        Self::new(7, 0o171, 0o133)
    }

    fn next_state(&self, state: usize, bit: usize) -> usize {
        (bit << (self.k - 2)) | (state >> 1)
    }

    #[must_use]
    pub fn encode(&self, bits: &[u8]) -> Vec<u8> {
        let mut state = 0usize;
        let mut out = Vec::with_capacity(bits.len() * 2);
        for &bit in bits {
            let bit = usize::from(bit & 1);
            let pair = self.outputs[state * 2 + bit];
            out.push(pair & 1);
            out.push(pair >> 1);
            state = self.next_state(state, bit);
        }
        out
    }

    #[must_use]
    pub fn decode(&self, soft: &[f32]) -> Vec<u8> {
        let steps = soft.len() / 2;
        let mut metric = vec![UNREACHED; self.states];
        metric[0] = 0.0;
        let mut next = vec![UNREACHED; self.states];
        let mut decisions = Vec::with_capacity(steps);
        for &[first, second] in soft.as_chunks::<2>().0 {
            decisions.push(self.step(&metric, &mut next, first, second));
            std::mem::swap(&mut metric, &mut next);
        }
        self.traceback(&metric, &decisions)
    }

    fn step(&self, metric: &[f32], next: &mut [f32], first: f32, second: f32) -> u64 {
        next.fill(UNREACHED);
        let mut decision = 0u64;
        for (state, &m) in metric.iter().enumerate() {
            if m == UNREACHED {
                continue;
            }
            for bit in 0..2 {
                let pair = self.outputs[state * 2 + bit];
                let e1 = if pair & 1 == 1 { first } else { -first };
                let e2 = if pair >> 1 == 1 { second } else { -second };
                let candidate = m + e1 + e2;
                let target = self.next_state(state, bit);
                if candidate > next[target] {
                    next[target] = candidate;
                    decision = decision & !(1 << target) | ((state as u64 & 1) << target);
                }
            }
        }
        decision
    }

    fn traceback(&self, metric: &[f32], decisions: &[u64]) -> Vec<u8> {
        let mask = self.states - 1;
        let mut state = metric
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map_or(0, |(index, _)| index);
        let mut bits = vec![0u8; decisions.len()];
        for (bit, &decision) in bits.iter_mut().zip(decisions).rev() {
            *bit = (state >> (self.k - 2)) as u8;
            let lost = (decision >> state) & 1;
            state = ((state << 1) | lost as usize) & mask;
        }
        bits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn random_bits(n: usize, seed: u64) -> Vec<u8> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s & 1) as u8
            })
            .collect()
    }

    fn antipodal(coded: &[u8]) -> Vec<f32> {
        coded
            .iter()
            .map(|&b| if b == 1 { 1.0 } else { -1.0 })
            .collect()
    }

    #[test]
    fn a_clean_block_round_trips() {
        let code = SoftViterbi::k7();
        let mut bits = random_bits(300, 1);
        bits.extend([0; 6]);
        assert_eq!(code.decode(&antipodal(&code.encode(&bits))), bits);
    }

    #[test]
    fn sparse_hard_errors_are_corrected() {
        let code = SoftViterbi::k7();
        let mut bits = random_bits(400, 2);
        bits.extend([0; 6]);
        let mut soft = antipodal(&code.encode(&bits));
        for i in (7..soft.len()).step_by(25) {
            soft[i] = -soft[i];
        }
        assert_eq!(code.decode(&soft), bits);
    }

    #[test]
    fn moderate_noise_decodes() {
        let code = SoftViterbi::new(7, 0o133, 0o171);
        let mut bits = random_bits(400, 3);
        bits.extend([0; 6]);
        let mut s = 0x1234_5678u64;
        let soft: Vec<f32> = antipodal(&code.encode(&bits))
            .into_iter()
            .map(|v| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                v + ((s as f32 / u64::MAX as f32) * 2.0 - 1.0) * 0.9
            })
            .collect();
        assert_eq!(code.decode(&soft), bits);
    }

    #[test]
    fn shorter_constraint_lengths_round_trip() {
        let code = SoftViterbi::new(5, 0o23, 0o35);
        let mut bits = random_bits(120, 4);
        bits.extend([0; 4]);
        assert_eq!(code.decode(&antipodal(&code.encode(&bits))), bits);
    }
}
