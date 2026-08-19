use super::conv::{ERASURE, Soft};

pub const STATES: usize = 64;
const MAX_OUTPUTS: usize = 8;
const STATE_MASK: usize = STATES - 1;

#[derive(Clone, Debug)]
pub struct ConvCode {
    outputs: usize,
    branch: [u8; 128],
}

impl ConvCode {
    #[must_use]
    pub fn new(polys: &[u16]) -> Self {
        assert!(
            (1..=MAX_OUTPUTS).contains(&polys.len()),
            "a K=7 code needs between 1 and {MAX_OUTPUTS} generators"
        );
        let mut branch = [0u8; 128];
        for (register, slot) in branch.iter_mut().enumerate() {
            let mut mask = 0u8;
            for (index, &poly) in polys.iter().enumerate() {
                if (register as u16 & poly).count_ones() % 2 == 1 {
                    mask |= 1 << index;
                }
            }
            *slot = mask;
        }
        Self {
            outputs: polys.len(),
            branch,
        }
    }

    #[must_use]
    pub const fn outputs(&self) -> usize {
        self.outputs
    }

    fn register(state: usize, input: bool) -> usize {
        usize::from(input) << 6 | state
    }

    const fn advance(state: usize, input: bool) -> usize {
        state >> 1 | (input as usize) << 5
    }

    pub fn encode(&self, bits: &[bool], out: &mut Vec<bool>) {
        let mut state = 0usize;
        for &bit in bits {
            let mask = self.branch[Self::register(state, bit)];
            for index in 0..self.outputs {
                out.push(mask >> index & 1 == 1);
            }
            state = Self::advance(state, bit);
        }
    }
}

pub fn puncture(coded: &[bool], pattern: &[bool], out: &mut Vec<bool>) {
    for (index, &bit) in coded.iter().enumerate() {
        if pattern[index % pattern.len()] {
            out.push(bit);
        }
    }
}

pub fn depuncture(received: &[Soft], pattern: &[bool], out: &mut Vec<Soft>) {
    let kept = pattern.iter().filter(|&&keep| keep).count();
    if kept == 0 {
        return;
    }
    let cycles = received.len().div_ceil(kept);
    let mut source = received.iter();
    for _ in 0..cycles {
        for &keep in pattern {
            match keep.then(|| source.next()) {
                Some(Some(&value)) => out.push(value),
                Some(None) => return,
                None => out.push(ERASURE),
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Depuncturer {
    pattern: Vec<bool>,
    at: usize,
}

impl Depuncturer {
    #[must_use]
    pub fn new(pattern: &[bool]) -> Self {
        Self {
            pattern: pattern.to_vec(),
            at: 0,
        }
    }

    pub fn set_pattern(&mut self, pattern: &[bool]) {
        self.pattern.clear();
        self.pattern.extend_from_slice(pattern);
        self.at = 0;
    }

    pub fn reset(&mut self) {
        self.at = 0;
    }

    pub fn process(&mut self, received: &[Soft], out: &mut Vec<Soft>) {
        if !self.pattern.iter().any(|&keep| keep) {
            return;
        }
        for &value in received {
            while !self.pattern[self.at] {
                out.push(ERASURE);
                self.at = (self.at + 1) % self.pattern.len();
            }
            out.push(value);
            self.at = (self.at + 1) % self.pattern.len();
        }
    }
}

struct Trellis {
    metrics: [i32; STATES],
    next: [i32; STATES],
}

impl Trellis {
    const FLOOR: i32 = -(1 << 24);
    const CEILING: i32 = 1 << 28;

    fn new() -> Self {
        Self {
            metrics: [Self::FLOOR; STATES],
            next: [Self::FLOOR; STATES],
        }
    }

    fn restart(&mut self) {
        self.metrics = [Self::FLOOR; STATES];
        self.metrics[0] = 0;
    }

    fn open(&mut self) {
        self.metrics = [0; STATES];
    }

    fn step(&mut self, code: &ConvCode, symbol: &[Soft]) -> u64 {
        let mut decisions = 0u64;
        for state in 0..STATES {
            let input = state >> 5 == 1;
            let low = (state & 0x1F) << 1;
            let mut best = (i32::MIN, false);
            for lsb in [false, true] {
                let previous = low | usize::from(lsb);
                let mask = code.branch[ConvCode::register(previous, input)];
                let mut branch = 0i32;
                for (index, &value) in symbol.iter().enumerate() {
                    let soft = i32::from(value);
                    branch += if mask >> index & 1 == 1 { soft } else { -soft };
                }
                let metric = self.metrics[previous].saturating_add(branch);
                if metric > best.0 {
                    best = (metric, lsb);
                }
            }
            self.next[state] = best.0;
            decisions |= u64::from(best.1) << state;
        }
        self.metrics = self.next;
        decisions
    }

    fn normalize(&mut self) {
        let peak = self.metrics.iter().copied().max().unwrap_or(0);
        if peak > Self::CEILING {
            for metric in &mut self.metrics {
                *metric -= peak;
            }
        }
    }

    fn best_state(&self) -> usize {
        (0..STATES)
            .max_by_key(|&state| self.metrics[state])
            .unwrap_or_default()
    }
}

fn trace(decisions: &[u64], from: usize, mut visit: impl FnMut(bool)) -> usize {
    let mut state = from;
    for &word in decisions.iter().rev() {
        visit(state >> 5 == 1);
        state = (state & 0x1F) << 1 | usize::from(word >> state & 1 == 1);
    }
    state & STATE_MASK
}

pub struct ViterbiK7 {
    code: ConvCode,
    trellis: Trellis,
    decisions: Vec<u64>,
}

impl ViterbiK7 {
    #[must_use]
    pub fn new(code: ConvCode) -> Self {
        Self {
            code,
            trellis: Trellis::new(),
            decisions: Vec::new(),
        }
    }

    #[must_use]
    pub const fn outputs(&self) -> usize {
        self.code.outputs()
    }

    pub fn decode(&mut self, coded: &[Soft], out: &mut Vec<bool>) -> i32 {
        self.run(coded, out, None)
    }

    pub fn decode_tailed(&mut self, coded: &[Soft], out: &mut Vec<bool>) -> i32 {
        self.run(coded, out, Some(0))
    }

    fn run(&mut self, coded: &[Soft], out: &mut Vec<bool>, end: Option<usize>) -> i32 {
        let outputs = self.code.outputs();
        let steps = coded.len() / outputs;
        self.decisions.clear();
        self.decisions.reserve(steps);
        self.trellis.restart();
        for symbol in coded[..steps * outputs].chunks_exact(outputs) {
            let word = self.trellis.step(&self.code, symbol);
            self.decisions.push(word);
            self.trellis.normalize();
        }
        let state = end.unwrap_or_else(|| self.trellis.best_state());
        let metric = self.trellis.metrics[state];
        let start = out.len();
        trace(&self.decisions, state, |bit| out.push(bit));
        out[start..].reverse();
        metric
    }
}

pub struct StreamViterbiK7 {
    code: ConvCode,
    trellis: Trellis,
    decisions: Vec<u64>,
    depth: usize,
    pending: Vec<bool>,
    carry: [Soft; MAX_OUTPUTS],
    carried: usize,
}

impl StreamViterbiK7 {
    #[must_use]
    pub fn new(code: ConvCode, depth: usize) -> Self {
        let depth = depth.max(8);
        let mut trellis = Trellis::new();
        trellis.open();
        Self {
            code,
            trellis,
            decisions: Vec::with_capacity(2 * depth),
            depth,
            pending: Vec::with_capacity(depth),
            carry: [ERASURE; MAX_OUTPUTS],
            carried: 0,
        }
    }

    #[must_use]
    pub const fn outputs(&self) -> usize {
        self.code.outputs()
    }

    pub fn reset(&mut self) {
        self.trellis = Trellis::new();
        self.trellis.open();
        self.decisions.clear();
        self.pending.clear();
        self.carried = 0;
    }

    fn step(&mut self, symbol: &[Soft], out: &mut Vec<bool>) {
        let word = self.trellis.step(&self.code, symbol);
        self.decisions.push(word);
        self.trellis.normalize();
        if self.decisions.len() >= 2 * self.depth {
            self.release(out);
        }
    }

    pub fn push(&mut self, coded: &[Soft], out: &mut Vec<bool>) {
        let outputs = self.code.outputs();
        let mut source = coded;
        if self.carried > 0 {
            let take = (outputs - self.carried).min(source.len());
            self.carry[self.carried..self.carried + take].copy_from_slice(&source[..take]);
            self.carried += take;
            source = &source[take..];
            if self.carried < outputs {
                return;
            }
            self.carried = 0;
            let symbol = self.carry;
            self.step(&symbol[..outputs], out);
        }
        let usable = source.len() / outputs * outputs;
        for index in (0..usable).step_by(outputs) {
            let mut symbol = [ERASURE; MAX_OUTPUTS];
            symbol[..outputs].copy_from_slice(&source[index..index + outputs]);
            self.step(&symbol[..outputs], out);
        }
        self.carried = source.len() - usable;
        self.carry[..self.carried].copy_from_slice(&source[usable..]);
    }

    fn release(&mut self, out: &mut Vec<bool>) {
        let survivor = self.trellis.best_state();
        let state = trace(&self.decisions[self.depth..], survivor, |_| {});
        self.pending.clear();
        let pending = &mut self.pending;
        trace(&self.decisions[..self.depth], state, |bit| pending.push(bit));
        out.extend(self.pending.iter().rev().copied());
        self.decisions.copy_within(self.depth.., 0);
        self.decisions.truncate(self.depth);
    }

    pub fn flush(&mut self, out: &mut Vec<bool>) {
        if self.decisions.is_empty() {
            return;
        }
        let survivor = self.trellis.best_state();
        self.pending.clear();
        let pending = &mut self.pending;
        trace(&self.decisions, survivor, |bit| pending.push(bit));
        out.extend(self.pending.iter().rev().copied());
        self.decisions.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::super::conv::{CONFIDENT, soft};
    use super::*;

    const DVB_S: [u16; 2] = [0o171, 0o133];
    const DAB: [u16; 4] = [0o133, 0o171, 0o145, 0o133];

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

    fn softs(bits: &[bool]) -> Vec<Soft> {
        bits.iter().copied().map(soft).collect()
    }

    fn tailed(len: usize, seed: u32) -> Vec<bool> {
        let mut bits = message(len, seed);
        bits.extend([false; 6]);
        bits
    }

    #[test]
    fn the_generator_taps_match_the_dvb_s_reference_response() {
        let code = ConvCode::new(&DVB_S);
        let mut coded = Vec::new();
        let mut impulse = vec![true];
        impulse.extend([false; 6]);
        code.encode(&impulse, &mut coded);
        let first: Vec<bool> = coded.iter().step_by(2).copied().collect();
        let second: Vec<bool> = coded.iter().skip(1).step_by(2).copied().collect();
        assert_eq!(first, [true, true, true, true, false, false, true]);
        assert_eq!(second, [true, false, true, true, false, true, true]);
    }

    #[test]
    fn a_terminated_rate_one_half_frame_round_trips() {
        let code = ConvCode::new(&DVB_S);
        let bits = tailed(128, 7);
        let mut coded = Vec::new();
        code.encode(&bits, &mut coded);
        let mut out = Vec::new();
        ViterbiK7::new(code).decode_tailed(&softs(&coded), &mut out);
        assert_eq!(out, bits);
    }

    #[test]
    fn the_rate_one_quarter_mother_code_repairs_scattered_errors() {
        let code = ConvCode::new(&DAB);
        let bits = tailed(96, 11);
        let mut coded = Vec::new();
        code.encode(&bits, &mut coded);
        let mut received = softs(&coded);
        for position in (7..received.len()).step_by(11) {
            received[position] = -received[position];
        }
        let mut out = Vec::new();
        ViterbiK7::new(code).decode_tailed(&received, &mut out);
        assert_eq!(out, bits);
    }

    #[test]
    fn puncturing_to_rate_three_quarters_still_decodes() {
        let pattern = [true, true, true, false, false, true];
        let code = ConvCode::new(&DVB_S);
        let bits = tailed(192, 13);
        let mut coded = Vec::new();
        code.encode(&bits, &mut coded);
        let mut sent = Vec::new();
        puncture(&coded, &pattern, &mut sent);
        assert_eq!(sent.len() * 3, coded.len() * 2);
        let mut received = Vec::new();
        depuncture(&softs(&sent), &pattern, &mut received);
        assert_eq!(received.len(), coded.len());
        let mut out = Vec::new();
        ViterbiK7::new(code).decode_tailed(&received, &mut out);
        assert_eq!(out, bits);
    }

    #[test]
    fn depuncturing_marks_the_dropped_positions_as_erasures() {
        let pattern = [true, false];
        let mut out = Vec::new();
        depuncture(&[CONFIDENT, -CONFIDENT], &pattern, &mut out);
        assert_eq!(out, [CONFIDENT, ERASURE, -CONFIDENT, ERASURE]);
    }

    #[test]
    fn the_stateful_depuncturer_matches_the_block_one_across_chunks() {
        let pattern = [true, true, false, true, true, false];
        let sent: Vec<Soft> = (0..96).map(|index| soft(index % 3 == 0)).collect();
        let mut block = Vec::new();
        depuncture(&sent, &pattern, &mut block);
        let mut streamed = Vec::new();
        let mut depuncturer = Depuncturer::new(&pattern);
        for chunk in sent.chunks(7) {
            depuncturer.process(chunk, &mut streamed);
        }
        assert_eq!(streamed, block[..streamed.len()]);
        assert!(block[streamed.len()..].iter().all(|&value| value == ERASURE));
    }

    #[test]
    fn the_streaming_decoder_matches_the_block_decoder() {
        let code = ConvCode::new(&DVB_S);
        let bits = message(4_000, 29);
        let mut coded = Vec::new();
        code.encode(&bits, &mut coded);
        let received = softs(&coded);
        let mut stream = StreamViterbiK7::new(code, 96);
        let mut out = Vec::new();
        for block in received.chunks(512) {
            stream.push(block, &mut out);
        }
        stream.flush(&mut out);
        assert_eq!(out.len(), bits.len());
        assert_eq!(out[64..3_900], bits[64..3_900]);
    }

    #[test]
    fn the_streaming_decoder_carries_a_split_symbol_across_chunks() {
        let code = ConvCode::new(&DAB);
        let bits = message(2_000, 37);
        let mut coded = Vec::new();
        code.encode(&bits, &mut coded);
        let received = softs(&coded);
        let mut whole = StreamViterbiK7::new(ConvCode::new(&DAB), 96);
        let mut expected = Vec::new();
        whole.push(&received, &mut expected);
        whole.flush(&mut expected);
        let mut split = StreamViterbiK7::new(code, 96);
        let mut out = Vec::new();
        for chunk in received.chunks(37) {
            split.push(chunk, &mut out);
        }
        split.flush(&mut out);
        assert_eq!(out, expected);
        assert_eq!(out[16..1_900], bits[16..1_900]);
    }

    #[test]
    fn the_streaming_decoder_recovers_after_a_burst() {
        let code = ConvCode::new(&DVB_S);
        let bits = message(4_000, 31);
        let mut coded = Vec::new();
        code.encode(&bits, &mut coded);
        let mut received = softs(&coded);
        for value in &mut received[1_000..1_040] {
            *value = -*value;
        }
        let mut stream = StreamViterbiK7::new(code, 96);
        let mut out = Vec::new();
        stream.push(&received, &mut out);
        stream.flush(&mut out);
        assert_eq!(out[2_000..3_900], bits[2_000..3_900]);
    }
}
