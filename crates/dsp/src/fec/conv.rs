//! The rate-1/2, constraint-length-5 convolutional code every amateur digital-voice mode
//! protects its signalling with, and a soft-decision Viterbi decoder for it.
/// Soft value of one received coded bit: positive votes for 1, negative for 0, and the
/// magnitude is the confidence. [`ERASURE`] is the absence of a vote.
pub type Soft = i16;

/// A coded bit the transmitter punctured away.
pub const ERASURE: Soft = 0;

/// Full confidence, the value a hard decision maps to.
pub const CONFIDENT: Soft = 64;

const STATES: usize = 16;
const G1: u8 = 0b1_1001;
const G2: u8 = 0b1_0111;

/// Map a hard bit to a soft value.
#[must_use]
pub fn soft(bit: bool) -> Soft {
    if bit { CONFIDENT } else { -CONFIDENT }
}

/// Encode `bits` at rate 1/2, appending two coded bits per information bit. The caller adds
/// whatever flush bits its mode specifies — some flush the register, some leave it running
/// into the next frame, and this code does not decide that for them.
pub fn encode(bits: &[bool], out: &mut Vec<bool>) {
    let mut reg = 0u8;
    for &bit in bits {
        reg = (reg << 1 | u8::from(bit)) & 0x1F;
        out.push((reg & G1).count_ones() % 2 == 1);
        out.push((reg & G2).count_ones() % 2 == 1);
    }
}

/// Soft-decision Viterbi decoder for [`encode`].
///
/// Owns its traceback buffer, which grows once to the longest frame it is given and is reused
/// after that — the one allocating primitive in `fec`, and it allocates only on the first frame
/// of a call. Everything else is fixed-size state.
#[derive(Clone, Debug, Default)]
pub struct Viterbi5 {
    /// One bit per state per step: which predecessor won.
    decisions: Vec<u16>,
    metrics: [i32; STATES],
    next: [i32; STATES],
}

impl Viterbi5 {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode `coded` (two soft values per information bit) into `out`, returning the winning
    /// path metric — the accumulated agreement between the received soft values and the
    /// codeword the decoder settled on, so a caller can compare two hypotheses.
    ///
    /// The encoder is assumed to have started from the all-zero state, which every mode here
    /// specifies. The final state is not constrained: modes that flush their register end in
    /// state zero anyway, and modes that do not would be punished for it.
    ///
    /// # Panics
    /// If `coded` has an odd length.
    pub fn decode(&mut self, coded: &[Soft], out: &mut Vec<bool>) -> i32 {
        assert!(
            coded.len().is_multiple_of(2),
            "rate 1/2 needs an even number of bits"
        );
        let steps = coded.len() / 2;
        self.decisions.clear();
        self.decisions.resize(steps, 0);
        // Only the all-zero start state is reachable; the rest begin unreachably bad.
        self.metrics = [i32::MIN / 4; STATES];
        self.metrics[0] = 0;

        let (pairs, _) = coded.as_chunks::<2>();
        for (step, &[first, second]) in pairs.iter().enumerate() {
            let (r1, r2) = (i32::from(first), i32::from(second));
            let mut decisions = 0u16;
            for state in 0..STATES {
                let mut best = (i32::MIN, false);
                for prev_high in [false, true] {
                    let prev = state >> 1 | usize::from(prev_high) << 3;
                    let reg = (prev << 1 | state & 1) as u8;
                    let b1 = (reg & G1).count_ones() % 2 == 1;
                    let b2 = (reg & G2).count_ones() % 2 == 1;
                    let branch = if b1 { r1 } else { -r1 } + if b2 { r2 } else { -r2 };
                    let metric = self.metrics[prev].saturating_add(branch);
                    if metric > best.0 {
                        best = (metric, prev_high);
                    }
                }
                self.next[state] = best.0;
                decisions |= u16::from(best.1) << state;
            }
            self.decisions[step] = decisions;
            self.metrics = self.next;
        }

        let mut state = (0..STATES)
            .max_by_key(|&s| self.metrics[s])
            .unwrap_or_default();
        let metric = self.metrics[state];
        let start = out.len();
        for step in (0..steps).rev() {
            out.push(state & 1 == 1);
            let prev_high = self.decisions[step] >> state & 1 == 1;
            state = state >> 1 | usize::from(prev_high) << 3;
        }
        out[start..].reverse();
        metric
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn softs(bits: &[bool]) -> Vec<Soft> {
        bits.iter().copied().map(soft).collect()
    }

    fn message(len: usize, seed: u32) -> Vec<bool> {
        let mut state = seed | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state & 1 == 1
            })
            .collect()
    }

    #[test]
    fn round_trips_a_flushed_frame() {
        let mut bits = message(96, 5);
        bits.extend([false; 4]);
        let mut coded = Vec::new();
        encode(&bits, &mut coded);
        assert_eq!(coded.len(), bits.len() * 2);

        let mut out = Vec::new();
        Viterbi5::new().decode(&softs(&coded), &mut out);
        assert_eq!(out, bits);
    }

    /// The reason a convolutional code is there at all: errors the block codes around it would
    /// have to detect and drop, this one repairs.
    #[test]
    fn repairs_scattered_channel_errors() {
        let mut bits = message(72, 9);
        bits.extend([false; 4]);
        let mut coded = Vec::new();
        encode(&bits, &mut coded);
        let mut received = softs(&coded);
        for bit in [3usize, 40, 41, 90, 130] {
            received[bit] = -received[bit];
        }
        let mut out = Vec::new();
        Viterbi5::new().decode(&received, &mut out);
        assert_eq!(out, bits);
    }

    /// A punctured code is the same decoder with holes in its input.
    #[test]
    fn erasures_stand_in_for_punctured_bits() {
        let mut bits = message(48, 3);
        bits.extend([false; 4]);
        let mut coded = Vec::new();
        encode(&bits, &mut coded);
        let mut received = softs(&coded);
        for (i, value) in received.iter_mut().enumerate() {
            if i % 8 == 7 {
                *value = ERASURE;
            }
        }
        let mut out = Vec::new();
        Viterbi5::new().decode(&received, &mut out);
        assert_eq!(out, bits);
    }

    /// Soft input is not a luxury here: the same errors that a soft decoder rides through sink
    /// a hard one, and the modes below feed it real symbol distances.
    #[test]
    fn weak_bits_lose_to_confident_ones() {
        let mut bits = message(64, 21);
        bits.extend([false; 4]);
        let mut coded = Vec::new();
        encode(&bits, &mut coded);
        let mut received = softs(&coded);
        for bit in [10usize, 11, 12, 13] {
            received[bit] = if received[bit] > 0 { -1 } else { 1 };
        }
        let mut out = Vec::new();
        Viterbi5::new().decode(&received, &mut out);
        assert_eq!(out, bits);
    }
}
