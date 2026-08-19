use num_complex::Complex;
use sdrmm_dsp::{
    CONFIDENT, ConvCode, DVB_DISPERSAL, DVB_PRIMITIVE, Depuncturer, Prbs, ReedSolomon, Soft,
    ERASURE, StreamViterbiK7, ViterbiK7,
};
use sdrmm_wire::DatvCodeRate;

pub const PACKET: usize = 188;
pub const CODEWORD: usize = 204;
pub const SYNC: u8 = 0x47;
pub const INVERTED_SYNC: u8 = 0xB8;
pub const GROUP: usize = 8;
pub const GENERATORS: [u16; 2] = [0o171, 0o133];

const BRANCHES: usize = 12;
const BRANCH_DEPTH: usize = 17;
const TRACEBACK: usize = 96;
const TRIAL_SYMBOLS: usize = 512;
const SYNC_PACKETS: usize = 5;
const ACCEPT_METRIC: f32 = 0.86;
const MAX_SYNC_LOSS: u32 = 16;
const ALIGN_WINDOW: usize = CODEWORD * 8 * (SYNC_PACKETS + 3);
const ALIGN_BUDGET: usize = CODEWORD * 8 * 40;

pub const RATES: [DatvCodeRate; 5] = [
    DatvCodeRate::Half,
    DatvCodeRate::TwoThirds,
    DatvCodeRate::ThreeQuarters,
    DatvCodeRate::FiveSixths,
    DatvCodeRate::SevenEighths,
];

#[must_use]
pub fn puncturing(rate: DatvCodeRate) -> &'static [bool] {
    const HALF: [bool; 2] = [true, true];
    const TWO_THIRDS: [bool; 4] = [true, true, false, true];
    const THREE_QUARTERS: [bool; 6] = [true, true, false, true, true, false];
    const FIVE_SIXTHS: [bool; 10] = [
        true, true, false, true, true, false, false, true, true, false,
    ];
    const SEVEN_EIGHTHS: [bool; 14] = [
        true, true, false, true, false, true, false, true, true, false, false, true, true, false,
    ];
    match rate {
        DatvCodeRate::Auto | DatvCodeRate::Half => &HALF,
        DatvCodeRate::TwoThirds => &TWO_THIRDS,
        DatvCodeRate::ThreeQuarters => &THREE_QUARTERS,
        DatvCodeRate::FiveSixths => &FIVE_SIXTHS,
        DatvCodeRate::SevenEighths => &SEVEN_EIGHTHS,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PuncturePhase {
    pub pattern: Vec<bool>,
    pub prefix: usize,
}

#[must_use]
pub fn puncture_phases(rate: DatvCodeRate) -> Vec<PuncturePhase> {
    let pattern = puncturing(rate);
    pattern
        .iter()
        .enumerate()
        .filter_map(|(index, &kept)| {
            kept.then(|| PuncturePhase {
                pattern: pattern[index..]
                    .iter()
                    .chain(&pattern[..index])
                    .copied()
                    .collect(),
                prefix: index % 2,
            })
        })
        .collect()
}

#[must_use]
pub fn soft_pair(symbol: Complex<f32>, rotation: u8, scale: f32) -> [Soft; 2] {
    let (i, q) = (symbol.re * scale, symbol.im * scale);
    let (a, b) = match rotation & 3 {
        0 => (-i, -q),
        1 => (q, -i),
        2 => (i, q),
        _ => (-q, i),
    };
    [clamp(a), clamp(b)]
}

fn clamp(value: f32) -> Soft {
    let limit = f32::from(CONFIDENT);
    (value * limit).clamp(-limit, limit) as Soft
}

fn normalize(symbols: &[Complex<f32>]) -> f32 {
    let power: f64 = symbols
        .iter()
        .map(|symbol| f64::from(symbol.norm_sqr()))
        .sum();
    let mean = (power / symbols.len().max(1) as f64).max(1e-12);
    (1.0 / mean.sqrt()) as f32
}

struct DelayBank {
    lines: Vec<Vec<u8>>,
    at: Vec<usize>,
    branch: usize,
}

impl DelayBank {
    fn new(depth: impl Fn(usize) -> usize) -> Self {
        Self {
            lines: (0..BRANCHES).map(|branch| vec![0u8; depth(branch)]).collect(),
            at: vec![0; BRANCHES],
            branch: 0,
        }
    }

    fn reset(&mut self) {
        for line in &mut self.lines {
            line.fill(0);
        }
        self.at.fill(0);
        self.branch = 0;
    }

    fn push(&mut self, byte: u8) -> u8 {
        let branch = self.branch;
        self.branch = (self.branch + 1) % BRANCHES;
        let line = &mut self.lines[branch];
        if line.is_empty() {
            return byte;
        }
        let slot = &mut self.at[branch];
        let delayed = line[*slot];
        line[*slot] = byte;
        *slot = (*slot + 1) % line.len();
        delayed
    }
}

pub struct Interleaver(DelayBank);

impl Interleaver {
    #[must_use]
    pub fn new() -> Self {
        Self(DelayBank::new(|branch| branch * BRANCH_DEPTH))
    }

    pub fn push(&mut self, byte: u8) -> u8 {
        self.0.push(byte)
    }
}

impl Default for Interleaver {
    fn default() -> Self {
        Self::new()
    }
}

struct Deinterleaver(DelayBank);

impl Deinterleaver {
    fn new() -> Self {
        Self(DelayBank::new(|branch| {
            (BRANCHES - 1 - branch) * BRANCH_DEPTH
        }))
    }
}

pub struct Dispersal {
    prbs: Prbs,
    packets: usize,
}

impl Dispersal {
    #[must_use]
    pub fn new() -> Self {
        Self {
            prbs: Prbs::new(DVB_DISPERSAL),
            packets: 0,
        }
    }

    pub fn reset(&mut self) {
        self.prbs.reset();
        self.packets = 0;
    }

    pub fn scramble(&mut self, packet: &mut [u8; PACKET]) {
        if self.packets == 0 {
            self.prbs.reset();
            packet[0] = INVERTED_SYNC;
        } else {
            packet[0] = SYNC;
            self.prbs.skip_bytes(1);
        }
        self.prbs.apply_bytes(&mut packet[1..]);
        self.packets = (self.packets + 1) % GROUP;
    }
}

impl Default for Dispersal {
    fn default() -> Self {
        Self::new()
    }
}

struct Derandomizer {
    prbs: Prbs,
    started: bool,
}

impl Derandomizer {
    fn new() -> Self {
        Self {
            prbs: Prbs::new(DVB_DISPERSAL),
            started: false,
        }
    }

    fn reset(&mut self) {
        self.prbs.reset();
        self.started = false;
    }

    fn apply(&mut self, packet: &mut [u8; PACKET]) -> bool {
        if packet[0] == INVERTED_SYNC {
            self.prbs.reset();
            self.started = true;
        } else if self.started {
            self.prbs.skip_bytes(1);
        } else {
            return false;
        }
        packet[0] = SYNC;
        self.prbs.apply_bytes(&mut packet[1..]);
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DvbsLock {
    pub rate: DatvCodeRate,
    pub rotation: u8,
    pub phase: usize,
    pub inverted: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DvbsMetrics {
    pub packets_ok: u32,
    pub packets_bad: u32,
    pub corrected_symbols: u32,
}

impl DvbsMetrics {
    #[must_use]
    pub fn byte_error_rate(&self) -> Option<f32> {
        let seen = (self.packets_ok + self.packets_bad) as f32 * CODEWORD as f32;
        (seen > 0.0).then(|| {
            (self.corrected_symbols as f32
                + self.packets_bad as f32 * (CODEWORD - PACKET) as f32 / 2.0)
                / seen
        })
    }
}

fn pack_byte(bits: &[bool], start: usize) -> u8 {
    bits[start..start + 8]
        .iter()
        .fold(0u8, |byte, &bit| byte << 1 | u8::from(bit))
}

fn sync_offset(bits: &[bool]) -> Option<(usize, bool)> {
    let span = CODEWORD * 8 * (SYNC_PACKETS - 1) + 8;
    if bits.len() < span {
        return None;
    }
    for offset in 0..=bits.len() - span {
        let mut normal = 0usize;
        let mut inverted = 0usize;
        for packet in 0..SYNC_PACKETS {
            match pack_byte(bits, offset + packet * CODEWORD * 8) {
                SYNC => normal += 1,
                INVERTED_SYNC => inverted += 1,
                _ => break,
            }
        }
        if normal + inverted == SYNC_PACKETS {
            return Some((offset, inverted > normal));
        }
    }
    None
}

struct Trial {
    viterbi: ViterbiK7,
    mother: Vec<Soft>,
    decoded: Vec<bool>,
}

impl Trial {
    fn new() -> Self {
        Self {
            viterbi: ViterbiK7::new(ConvCode::new(&GENERATORS)),
            mother: Vec::new(),
            decoded: Vec::new(),
        }
    }

    fn score(&mut self, received: &[Soft], phase: &PuncturePhase) -> f32 {
        self.mother.clear();
        self.mother.resize(phase.prefix, ERASURE);
        Depuncturer::new(&phase.pattern).process(received, &mut self.mother);
        self.decoded.clear();
        let metric = self.viterbi.decode(&self.mother, &mut self.decoded);
        let energy: i64 = received.iter().map(|&value| i64::from(value.abs())).sum();
        if energy == 0 {
            return 0.0;
        }
        (metric as f64 / energy as f64) as f32
    }
}

enum Stage {
    Search,
    Align,
    Run,
}

pub struct DvbsDecoder {
    requested: DatvCodeRate,
    cooldown: usize,
    since_attempt: usize,
    stage: Stage,
    lock: Option<DvbsLock>,
    stream: StreamViterbiK7,
    depuncturer: Depuncturer,
    prefix: usize,
    trial: Trial,
    symbols: Vec<Complex<f32>>,
    received: Vec<Soft>,
    mother: Vec<Soft>,
    bits: Vec<bool>,
    deinterleaver: Deinterleaver,
    derandomizer: Derandomizer,
    reed_solomon: ReedSolomon,
    codeword: [u8; CODEWORD],
    filled: usize,
    losses: u32,
    aligning: usize,
    metrics: DvbsMetrics,
}

impl DvbsDecoder {
    #[must_use]
    pub fn new(requested: DatvCodeRate, symbol_rate: f64) -> Self {
        Self {
            requested,
            cooldown: (symbol_rate / 4.0).max(TRIAL_SYMBOLS as f64) as usize,
            since_attempt: usize::MAX / 2,
            stage: Stage::Search,
            lock: None,
            stream: StreamViterbiK7::new(ConvCode::new(&GENERATORS), TRACEBACK),
            depuncturer: Depuncturer::new(puncturing(DatvCodeRate::Half)),
            prefix: 0,
            trial: Trial::new(),
            symbols: Vec::new(),
            received: Vec::new(),
            mother: Vec::new(),
            bits: Vec::new(),
            deinterleaver: Deinterleaver::new(),
            derandomizer: Derandomizer::new(),
            reed_solomon: ReedSolomon::new(DVB_PRIMITIVE, 0, CODEWORD - PACKET),
            codeword: [0; CODEWORD],
            filled: 0,
            losses: 0,
            aligning: 0,
            metrics: DvbsMetrics::default(),
        }
    }

    #[must_use]
    pub const fn lock(&self) -> Option<DvbsLock> {
        self.lock
    }

    #[must_use]
    pub const fn metrics(&self) -> DvbsMetrics {
        self.metrics
    }

    #[must_use]
    pub const fn running(&self) -> bool {
        matches!(self.stage, Stage::Run)
    }

    pub fn reset(&mut self) {
        self.stage = Stage::Search;
        self.lock = None;
        self.since_attempt = 0;
        self.aligning = 0;
        self.stream.reset();
        self.symbols.clear();
        self.bits.clear();
        self.filled = 0;
        self.losses = 0;
        self.deinterleaver.0.reset();
        self.derandomizer.reset();
        self.metrics = DvbsMetrics::default();
    }

    fn candidates(&self) -> &'static [DatvCodeRate] {
        match self.requested {
            DatvCodeRate::Auto => &RATES,
            DatvCodeRate::Half => &RATES[..1],
            DatvCodeRate::TwoThirds => &RATES[1..2],
            DatvCodeRate::ThreeQuarters => &RATES[2..3],
            DatvCodeRate::FiveSixths => &RATES[3..4],
            DatvCodeRate::SevenEighths => &RATES[4..],
        }
    }

    fn soften(&mut self, rotation: u8) {
        let scale = normalize(&self.symbols);
        self.received.clear();
        self.received.reserve(2 * self.symbols.len());
        for &symbol in &self.symbols {
            self.received
                .extend_from_slice(&soft_pair(symbol, rotation, scale));
        }
    }

    fn acquire(&mut self) -> bool {
        let mut best: Option<(f32, DatvCodeRate, u8, usize)> = None;
        for rotation in 0..4u8 {
            self.soften(rotation);
            for &rate in self.candidates() {
                for (index, phase) in puncture_phases(rate).into_iter().enumerate() {
                    let score = self.trial.score(&self.received, &phase);
                    if best.is_none_or(|(current, ..)| score > current) {
                        best = Some((score, rate, rotation, index));
                    }
                }
            }
        }
        let Some((_, rate, rotation, phase)) = best.filter(|&(score, ..)| score >= ACCEPT_METRIC)
        else {
            return false;
        };
        let selected = puncture_phases(rate).swap_remove(phase);
        self.depuncturer.set_pattern(&selected.pattern);
        self.prefix = selected.prefix;
        self.lock = Some(DvbsLock {
            rate,
            rotation,
            phase,
            inverted: false,
        });
        self.stage = Stage::Align;
        self.aligning = 0;
        self.stream.reset();
        self.deinterleaver.0.reset();
        self.derandomizer.reset();
        self.bits.clear();
        self.filled = 0;
        self.losses = 0;
        self.metrics = DvbsMetrics::default();
        true
    }

    pub fn push(&mut self, symbols: &[Complex<f32>], packets: &mut Vec<[u8; PACKET]>) {
        self.symbols.extend_from_slice(symbols);
        self.since_attempt += symbols.len();
        if self.lock.is_none() {
            if self.symbols.len() < TRIAL_SYMBOLS || self.since_attempt < self.cooldown {
                if self.symbols.len() > 4 * TRIAL_SYMBOLS {
                    self.symbols.drain(..self.symbols.len() - TRIAL_SYMBOLS);
                }
                return;
            }
            self.since_attempt = 0;
            if !self.acquire() {
                self.symbols.clear();
                return;
            }
        }
        let Some(lock) = self.lock else { return };
        self.soften(lock.rotation);
        self.symbols.clear();
        self.mother.clear();
        self.mother.resize(std::mem::take(&mut self.prefix), ERASURE);
        let received = std::mem::take(&mut self.received);
        self.depuncturer.process(&received, &mut self.mother);
        self.received = received;
        let mother = std::mem::take(&mut self.mother);
        let before = self.bits.len();
        self.stream.push(&mother, &mut self.bits);
        let produced = self.bits.len() - before;
        self.mother = mother;
        self.drain(produced, packets);
    }

    fn drain(&mut self, produced: usize, packets: &mut Vec<[u8; PACKET]>) {
        if matches!(self.stage, Stage::Align) {
            match sync_offset(&self.bits) {
                Some((offset, inverted)) => {
                    if let Some(lock) = &mut self.lock {
                        lock.inverted = inverted;
                    }
                    self.bits.drain(..offset);
                    self.aligning = 0;
                    self.stage = Stage::Run;
                }
                None => {
                    self.aligning += produced;
                    if self.bits.len() > ALIGN_WINDOW {
                        let excess = self.bits.len() - ALIGN_WINDOW;
                        self.bits.drain(..excess);
                    }
                    if self.aligning > ALIGN_BUDGET {
                        self.reset();
                    }
                    return;
                }
            }
        }
        let mask = if self.lock.is_some_and(|lock| lock.inverted) {
            0xFF
        } else {
            0x00
        };
        let whole = self.bits.len() / 8;
        for index in 0..whole {
            let byte = pack_byte(&self.bits, index * 8) ^ mask;
            self.codeword[self.filled] = self.deinterleaver.0.push(byte);
            self.filled += 1;
            if self.filled == CODEWORD {
                self.filled = 0;
                self.finish_codeword(packets);
            }
        }
        self.bits.drain(..whole * 8);
        if self.losses >= MAX_SYNC_LOSS {
            self.reset();
        }
    }

    fn finish_codeword(&mut self, packets: &mut Vec<[u8; PACKET]>) {
        let mut codeword = self.codeword;
        match self.reed_solomon.decode(&mut codeword) {
            Some(corrected) => {
                self.metrics.packets_ok += 1;
                self.metrics.corrected_symbols += corrected;
                self.losses = 0;
                let mut packet = [0u8; PACKET];
                packet.copy_from_slice(&codeword[..PACKET]);
                if self.derandomizer.apply(&mut packet) {
                    packets.push(packet);
                }
            }
            None => {
                self.metrics.packets_bad += 1;
                self.losses += 1;
            }
        }
    }
}

const SQRT_HALF: f32 = std::f32::consts::FRAC_1_SQRT_2;

#[must_use]
pub fn map_qpsk(first: bool, second: bool) -> Complex<f32> {
    Complex::new(
        if first { -SQRT_HALF } else { SQRT_HALF },
        if second { -SQRT_HALF } else { SQRT_HALF },
    )
}

pub struct DvbsEncoder {
    code: ConvCode,
    pattern: &'static [bool],
    at: usize,
    state: Vec<bool>,
    interleaver: Interleaver,
    dispersal: Dispersal,
    reed_solomon: ReedSolomon,
    pending: Option<bool>,
    coded: Vec<bool>,
    codeword: Vec<u8>,
}

impl DvbsEncoder {
    #[must_use]
    pub fn new(rate: DatvCodeRate) -> Self {
        Self {
            code: ConvCode::new(&GENERATORS),
            pattern: puncturing(rate),
            at: 0,
            state: Vec::new(),
            interleaver: Interleaver::new(),
            dispersal: Dispersal::new(),
            reed_solomon: ReedSolomon::new(DVB_PRIMITIVE, 0, CODEWORD - PACKET),
            pending: None,
            coded: Vec::new(),
            codeword: Vec::new(),
        }
    }

    pub fn packet(&mut self, packet: &[u8; PACKET], out: &mut Vec<Complex<f32>>) {
        let mut scrambled = *packet;
        self.dispersal.scramble(&mut scrambled);
        self.codeword.clear();
        self.reed_solomon.encode(&scrambled, &mut self.codeword);
        self.state.clear();
        for index in 0..self.codeword.len() {
            let byte = self.interleaver.push(self.codeword[index]);
            for shift in (0..8).rev() {
                self.state.push(byte >> shift & 1 == 1);
            }
        }
        self.coded.clear();
        self.code.encode(&self.state, &mut self.coded);
        for &bit in &self.coded {
            let kept = self.pattern[self.at];
            self.at = (self.at + 1) % self.pattern.len();
            if !kept {
                continue;
            }
            match self.pending.take() {
                Some(first) => out.push(map_qpsk(first, bit)),
                None => self.pending = Some(bit),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transport(count: usize, seed: u32) -> Vec<[u8; PACKET]> {
        let mut state = seed | 1;
        (0..count)
            .map(|index| {
                let mut packet = [0u8; PACKET];
                packet[0] = SYNC;
                packet[1] = 0x40 | (index as u8 >> 4);
                packet[2] = index as u8;
                packet[3] = 0x10 | (index as u8 & 0x0F);
                for byte in &mut packet[4..] {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    *byte = state as u8;
                }
                packet
            })
            .collect()
    }

    #[test]
    fn every_puncture_pattern_has_the_rate_it_names() {
        for (rate, (inputs, kept)) in RATES
            .into_iter()
            .zip([(1usize, 2usize), (2, 3), (3, 4), (5, 6), (7, 8)])
        {
            let pattern = puncturing(rate);
            assert_eq!(pattern.len(), 2 * inputs, "{rate:?}");
            assert_eq!(
                pattern.iter().filter(|&&keep| keep).count(),
                kept,
                "{rate:?}"
            );
            assert_eq!(puncture_phases(rate).len(), kept, "{rate:?}");
        }
    }

    #[test]
    fn the_interleaver_and_deinterleaver_restore_the_byte_order() {
        let mut interleaver = Interleaver::new();
        let mut deinterleaver = Deinterleaver::new();
        let source: Vec<u8> = (0..CODEWORD * 20).map(|index| (index % 251) as u8).collect();
        let restored: Vec<u8> = source
            .iter()
            .map(|&byte| deinterleaver.0.push(interleaver.push(byte)))
            .collect();
        let delay = (BRANCHES - 1) * BRANCH_DEPTH * BRANCHES;
        assert_eq!(restored[delay..], source[..source.len() - delay]);
    }

    #[test]
    fn dispersal_marks_every_eighth_packet_and_is_reversible() {
        let mut dispersal = Dispersal::new();
        let mut derandomizer = Derandomizer::new();
        let packets = transport(24, 3);
        for (index, packet) in packets.iter().enumerate() {
            let mut scrambled = *packet;
            dispersal.scramble(&mut scrambled);
            assert_eq!(
                scrambled[0],
                if index % GROUP == 0 { INVERTED_SYNC } else { SYNC }
            );
            assert!(derandomizer.apply(&mut scrambled));
            assert_eq!(scrambled, *packet);
        }
    }

    #[test]
    fn the_sync_search_finds_the_packet_boundary_and_its_polarity() {
        let mut bits = vec![true, false, true, true, false, false, false];
        for packet in 0..SYNC_PACKETS {
            let sync = if packet == 0 { INVERTED_SYNC } else { SYNC };
            for shift in (0..8).rev() {
                bits.push(sync >> shift & 1 == 1);
            }
            bits.extend(std::iter::repeat_n(false, (CODEWORD - 1) * 8));
        }
        assert_eq!(sync_offset(&bits), Some((7, false)));
        let flipped: Vec<bool> = bits.iter().map(|&bit| !bit).collect();
        assert_eq!(sync_offset(&flipped), Some((7, true)));
    }

    fn round_trip(rate: DatvCodeRate, requested: DatvCodeRate) -> (Vec<[u8; PACKET]>, Vec<[u8; PACKET]>) {
        let sent = transport(220, 11);
        let mut encoder = DvbsEncoder::new(rate);
        let mut symbols = Vec::new();
        for packet in &sent {
            encoder.packet(packet, &mut symbols);
        }
        let mut decoder = DvbsDecoder::new(requested, 250_000.0);
        let mut received = Vec::new();
        for chunk in symbols.chunks(997) {
            decoder.push(chunk, &mut received);
        }
        (sent, received)
    }

    #[test]
    fn a_rate_one_half_transmission_round_trips_through_the_whole_chain() {
        let (sent, received) = round_trip(DatvCodeRate::Half, DatvCodeRate::Half);
        assert!(received.len() > 100, "only {} packets", received.len());
        let start = sent
            .iter()
            .position(|packet| packet[..4] == received[0][..4])
            .expect("a decoded packet must come from the transmission");
        assert_eq!(received[..], sent[start..start + received.len()]);
    }

    #[test]
    fn every_punctured_rate_round_trips() {
        for rate in RATES {
            let (sent, received) = round_trip(rate, rate);
            assert!(received.len() > 50, "{rate:?}: {} packets", received.len());
            let start = sent
                .iter()
                .position(|packet| packet[..4] == received[0][..4])
                .unwrap_or_else(|| panic!("{rate:?} decoded a packet nothing sent"));
            assert_eq!(received[..], sent[start..start + received.len()], "{rate:?}");
        }
    }

    #[test]
    fn the_code_rate_is_recovered_when_it_is_not_configured() {
        for rate in RATES {
            let (_, received) = round_trip(rate, DatvCodeRate::Auto);
            assert!(received.len() > 50, "{rate:?}: {} packets", received.len());
        }
    }

    #[test]
    fn a_quarter_turn_of_the_constellation_is_recovered() {
        let sent = transport(200, 19);
        let mut encoder = DvbsEncoder::new(DatvCodeRate::ThreeQuarters);
        let mut symbols = Vec::new();
        for packet in &sent {
            encoder.packet(packet, &mut symbols);
        }
        for rotation in 0..4u8 {
            let turned: Vec<Complex<f32>> = symbols
                .iter()
                .map(|&symbol| symbol * Complex::from_polar(1.0, rotation as f32 * std::f32::consts::FRAC_PI_2))
                .collect();
            let mut decoder = DvbsDecoder::new(DatvCodeRate::ThreeQuarters, 250_000.0);
            let mut received = Vec::new();
            decoder.push(&turned, &mut received);
            assert!(received.len() > 50, "rotation {rotation}: {} packets", received.len());
            let start = sent
                .iter()
                .position(|packet| packet[..4] == received[0][..4])
                .unwrap_or_else(|| panic!("rotation {rotation} decoded a stray packet"));
            assert_eq!(received[..], sent[start..start + received.len()]);
        }
    }

    #[test]
    fn noise_never_reports_a_lock() {
        let mut decoder = DvbsDecoder::new(DatvCodeRate::Auto, 250_000.0);
        let mut state = 0x1234_5678u32;
        let noise: Vec<Complex<f32>> = (0..200_000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let a = (state >> 16) as f32 / 32_768.0 - 1.0;
                let b = (state & 0xFFFF) as f32 / 32_768.0 - 1.0;
                Complex::new(a, b)
            })
            .collect();
        let mut received = Vec::new();
        decoder.push(&noise, &mut received);
        assert!(decoder.lock().is_none());
        assert!(received.is_empty());
    }
}
