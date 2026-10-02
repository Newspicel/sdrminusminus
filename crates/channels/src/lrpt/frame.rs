use sdrmm_dsp::fec::{ccsds_rs::CcsdsReedSolomon, conv7::ViterbiK7};

use super::link::{
    ASM_BYTES, ASM_CODED_BITS, AsmPattern, CODED_BITS, CODED_BYTES, INTERLEAVE, PairMap,
    VCDU_BYTES, asm_patterns, conv_code, pn_sequence,
};

const LEAD: usize = 96;
const TAIL: usize = ASM_CODED_BITS;
const WINDOW: usize = LEAD + CODED_BITS + TAIL;
const SEARCH_MISMATCHES: u32 = 4;
const TRACK_MISMATCHES: u32 = 14;
const MAX_MISSES: u8 = 2;
const SLIP_GUARD: usize = 64;
pub const SOFT_CAPACITY: usize = 2 * WINDOW + 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameOutcome {
    pub corrected: u32,
    pub failed: bool,
}

#[derive(Clone, Copy, Debug)]
enum State {
    Search {
        scan: usize,
        window: u64,
        filled: usize,
    },
    Track {
        start: usize,
        map: PairMap,
        misses: u8,
    },
}

impl State {
    fn search_from(scan: usize) -> Self {
        Self::Search {
            scan,
            window: 0,
            filled: 0,
        }
    }

    fn keep_from(&self) -> usize {
        match *self {
            Self::Search { scan, .. } => scan.saturating_sub(ASM_CODED_BITS + LEAD),
            Self::Track { start, .. } => start.saturating_sub(LEAD),
        }
    }

    fn shifted(self, by: usize) -> Self {
        match self {
            Self::Search {
                scan,
                window,
                filled,
            } => Self::Search {
                scan: scan - by,
                window,
                filled,
            },
            Self::Track { start, map, misses } => Self::Track {
                start: start - by,
                map,
                misses,
            },
        }
    }
}

pub struct Deframer {
    soft: Vec<i16>,
    state: State,
    patterns: [AsmPattern; 8],
    viterbi: ViterbiK7,
    coded: Vec<i16>,
    decoded: Vec<bool>,
    cadu: [u8; CODED_BYTES],
    pn: [u8; CODED_BYTES],
    rs: CcsdsReedSolomon,
    confirmed: bool,
}

impl Deframer {
    #[must_use]
    pub fn new() -> Self {
        let mut viterbi = ViterbiK7::new(conv_code());
        let mut decoded = Vec::with_capacity(WINDOW / 2 + 1);
        viterbi.decode(&[0; WINDOW], &mut decoded);
        decoded.clear();
        Self {
            soft: Vec::with_capacity(SOFT_CAPACITY),
            state: State::search_from(0),
            patterns: asm_patterns(),
            viterbi,
            coded: Vec::with_capacity(WINDOW),
            decoded,
            cadu: [0; CODED_BYTES],
            pn: pn_sequence(),
            rs: CcsdsReedSolomon::new(),
            confirmed: false,
        }
    }

    pub fn reset(&mut self) {
        self.soft.clear();
        self.state = State::search_from(0);
        self.confirmed = false;
    }

    #[must_use]
    pub fn vcdu(&self) -> &[u8] {
        &self.cadu[..VCDU_BYTES]
    }

    pub fn push(&mut self, soft: &[i16]) {
        let overflow = (self.soft.len() + soft.len()).saturating_sub(SOFT_CAPACITY);
        if overflow > 0 {
            self.discard(overflow);
        }
        let room = SOFT_CAPACITY - self.soft.len();
        let skip = soft.len().saturating_sub(room);
        self.soft.extend_from_slice(&soft[skip..]);
    }

    fn discard(&mut self, count: usize) {
        let count = count.min(self.soft.len());
        self.soft.copy_within(count.., 0);
        self.soft.truncate(self.soft.len() - count);
        self.state = match self.state {
            State::Track { start, .. } if start < count + LEAD => State::search_from(0),
            State::Search { scan, .. } if scan < count => State::search_from(0),
            other => other.shifted(count),
        };
    }

    fn compact(&mut self) {
        let keep = self.state.keep_from();
        if keep >= CODED_BITS / 2 {
            self.soft.copy_within(keep.., 0);
            self.soft.truncate(self.soft.len() - keep);
            self.state = self.state.shifted(keep);
        }
    }

    pub fn next_frame(&mut self) -> Option<FrameOutcome> {
        loop {
            let progressed = match self.state {
                State::Search {
                    scan,
                    window,
                    filled,
                } => self.search(scan, window, filled),
                State::Track { start, map, misses } => {
                    if start + CODED_BITS + TAIL > self.soft.len() {
                        false
                    } else if let Some(outcome) = self.track(start, map, misses) {
                        self.compact();
                        return Some(outcome);
                    } else {
                        true
                    }
                }
            };
            if !progressed {
                self.compact();
                return None;
            }
        }
    }

    fn search(&mut self, mut scan: usize, mut window: u64, mut filled: usize) -> bool {
        while scan < self.soft.len() {
            window = window << 1 | u64::from(self.soft[scan] > 0);
            scan += 1;
            filled += 1;
            if filled < ASM_CODED_BITS {
                continue;
            }
            let best = self
                .patterns
                .iter()
                .map(|pattern| (pattern.mismatches(window), pattern.map))
                .min_by_key(|&(mismatches, _)| mismatches);
            if let Some((mismatches, map)) = best
                && mismatches <= SEARCH_MISMATCHES
            {
                self.state = State::Track {
                    start: scan - ASM_CODED_BITS,
                    map,
                    misses: 0,
                };
                self.confirmed = false;
                return true;
            }
        }
        self.state = State::Search {
            scan,
            window,
            filled,
        };
        false
    }

    fn asm_mismatches(&self, start: usize, map: PairMap) -> u32 {
        let window = self.soft[start..start + ASM_CODED_BITS]
            .iter()
            .fold(0u64, |acc, &value| acc << 1 | u64::from(value > 0));
        self.patterns
            .iter()
            .find(|pattern| pattern.map == map)
            .map_or(u32::MAX, |pattern| pattern.mismatches(window))
    }

    fn track(&mut self, start: usize, map: PairMap, misses: u8) -> Option<FrameOutcome> {
        let next = start + CODED_BITS;
        if self.asm_mismatches(start, map) <= TRACK_MISMATCHES {
            self.state = State::Track {
                start: next,
                map,
                misses: 0,
            };
            let outcome = self.decode(start, map);
            if outcome.failed && !self.confirmed {
                return None;
            }
            self.confirmed = true;
            return Some(outcome);
        }
        self.state = if misses + 1 >= MAX_MISSES {
            State::search_from(start.saturating_sub(SLIP_GUARD))
        } else {
            State::Track {
                start: next,
                map,
                misses: misses + 1,
            }
        };
        None
    }

    fn decode(&mut self, start: usize, map: PairMap) -> FrameOutcome {
        let lead = start.min(LEAD) & !1;
        let from = start - lead;
        let to = start + CODED_BITS + TAIL;
        self.coded.clear();
        for &[a, b] in self.soft[from..to].as_chunks::<2>().0 {
            let (first, second) = map.apply(a, b);
            self.coded.push(first);
            self.coded.push(second);
        }
        self.decoded.clear();
        self.viterbi.decode(&self.coded, &mut self.decoded);
        let first_bit = lead / 2 + ASM_BYTES * 8;
        for (index, byte) in self.cadu.iter_mut().enumerate() {
            let bits = &self.decoded[first_bit + index * 8..first_bit + index * 8 + 8];
            *byte = bits.iter().fold(0u8, |acc, &bit| acc << 1 | u8::from(bit)) ^ self.pn[index];
        }
        let outcome = self.rs.decode_interleaved(&mut self.cadu, INTERLEAVE);
        FrameOutcome {
            corrected: outcome.corrected,
            failed: outcome.failed > 0,
        }
    }
}
