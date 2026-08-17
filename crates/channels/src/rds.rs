use std::f64::consts::FRAC_1_SQRT_2;

use num_complex::Complex;
use sdrmm_dsp::{
    ComplexOnePole, Costas, Decimator, Nco, Pll, RdsOffset, SymbolSync, design_lowpass,
    design_rds_biphase, rds_check_block, rds_correct_block,
};
use sdrmm_modem::{
    constellation::{Constellation, tables},
    symbolcode::DifferentialDecoder,
};
use sdrmm_wire::{DecoderEvent, RdsUpdate};

const BIT_RATE: f64 = 1_187.5;
const PILOT_HZ: f64 = 19_000.0;
const DATA_EDGE_HZ: f64 = 2.0 * BIT_RATE;
const TARGET_BASEBAND_HZ: f64 = 9_600.0;
const MIN_BASEBAND_HZ: f64 = 3.0 * DATA_EDGE_HZ;
const BLACKMAN_TRANSITION: f64 = 5.5;
const SHAPING_SPAN_SYMBOLS: usize = 4;

const PILOT_CUTOFF_HZ: f64 = 100.0;
const PILOT_STAGES: usize = 3;
const PILOT_LOOP_BW_HZ: f64 = 2.0;
const PILOT_RANGE_HZ: f64 = 25.0;
const PILOT_LOCK_ON: f32 = 0.7;
const TIMING_LOOP_BW: f64 = 0.01;
const PHASE_LOOP_BW: f64 = 0.02;
const PHASE_RANGE_HZ: f64 = 25.0;

const BLOCK_BITS: usize = 26;
const BLOCK_MASK: u32 = (1 << BLOCK_BITS) - 1;
const BLOCKS_PER_GROUP: usize = 4;
const A_SLOT: usize = 0;
const B_SLOT: usize = 1;
const C_SLOT: usize = 2;
const LAST_SLOT: usize = BLOCKS_PER_GROUP - 1;
const SYNC_WINDOW_BLOCKS: u32 = 50;
const SYNC_WINDOW_LIMIT: u32 = 20;

const PS_LEN: usize = 8;
const RT_LEN: usize = 64;
const RT_TERMINATOR: u8 = 0x0D;

const AF_COUNT_BASE: u8 = 224;
const AF_COUNT_TOP: u8 = 249;
const AF_MAX_CODE: u8 = 204;
const AF_BASE_HZ: f64 = 87_500_000.0;
const AF_STEP_HZ: f64 = 100_000.0;

const FIRST_GLYPH: usize = 0x20;

const EBU_LATIN: [char; 224] = [
    ' ', '!', '"', '#', '¤', '%', '&', '\'', '(', ')', '*', '+', ',', '-', '.', '/', //
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', ':', ';', '<', '=', '>', '?', //
    '@', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', //
    'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '[', '\\', ']', '―', '_', //
    '‖', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', //
    'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', '{', '|', '}', '¯', '«', //
    'á', 'à', 'é', 'è', 'í', 'ì', 'ó', 'ò', 'ú', 'ù', 'Ñ', 'Ç', 'Ş', 'ß', '¡', 'Ĳ', //
    'â', 'ä', 'ê', 'ë', 'î', 'ï', 'ô', 'ö', 'û', 'ü', 'ñ', 'ç', 'ş', 'ğ', 'ı', 'ĳ', //
    'ª', 'α', '©', '‰', 'Ğ', 'ě', 'ň', 'ő', 'π', '€', '£', '$', '←', '↑', '→', '↓', //
    'º', '¹', '²', '³', '±', 'İ', 'ń', 'ű', 'µ', '¿', '÷', '°', '¼', '½', '¾', '§', //
    'Á', 'À', 'É', 'È', 'Í', 'Ì', 'Ó', 'Ò', 'Ú', 'Ù', 'Ř', 'Č', 'Š', 'Ž', 'Ð', 'Ŀ', //
    'Â', 'Ä', 'Ê', 'Ë', 'Î', 'Ï', 'Ô', 'Ö', 'Û', 'Ü', 'ř', 'č', 'š', 'ž', 'đ', 'ŀ', //
    'Ã', 'Å', 'Æ', 'Œ', 'ŷ', 'Ý', 'Õ', 'Ø', 'Þ', 'Ŋ', 'Ŕ', 'Ć', 'Ś', 'Ź', 'Ŧ', 'ð', //
    'ã', 'å', 'æ', 'œ', 'ŵ', 'ý', 'õ', 'ø', 'þ', 'ŋ', 'ŕ', 'ć', 'ś', 'ź', 'ŧ', 'ť', //
];

const PTY_NAMES: [&str; 32] = [
    "None",
    "News",
    "Current Affairs",
    "Information",
    "Sport",
    "Education",
    "Drama",
    "Culture",
    "Science",
    "Varied",
    "Pop Music",
    "Rock Music",
    "Easy Listening",
    "Light Classical",
    "Serious Classical",
    "Other Music",
    "Weather",
    "Finance",
    "Children's Programmes",
    "Social Affairs",
    "Religion",
    "Phone In",
    "Travel",
    "Leisure",
    "Jazz Music",
    "Country Music",
    "National Music",
    "Oldies Music",
    "Folk Music",
    "Documentary",
    "Alarm Test",
    "Alarm",
];

pub(crate) struct RdsDecoder {
    mpx_rate: f64,
    nco: Nco,
    pilot_decim: Decimator,
    data_decim: Decimator,
    pilot_narrow: ComplexOnePole,
    pll: Pll,
    pilot_locked: bool,
    matched: Decimator,
    timing: SymbolSync,
    phase: Costas,
    alphabet: Constellation,
    differential: DifferentialDecoder,
    frames: GroupDecoder,
    pilot_mix: Vec<Complex<f32>>,
    data_mix: Vec<Complex<f32>>,
    pilot_bb: Vec<Complex<f32>>,
    data_bb: Vec<Complex<f32>>,
    carrier_free: Vec<Complex<f32>>,
    shaped: Vec<Complex<f32>>,
    symbols: Vec<Complex<f32>>,
}

impl RdsDecoder {
    pub(crate) fn new(mpx_rate: f64) -> Self {
        let factor = decimation(mpx_rate);
        let baseband_rate = mpx_rate / factor as f64;
        let sps = baseband_rate / BIT_RATE;
        let anti_alias = anti_alias(mpx_rate, baseband_rate, factor);
        Self {
            mpx_rate,
            nco: Nco::new(PILOT_HZ as f32, mpx_rate as f32),
            pilot_decim: Decimator::new(&anti_alias, factor),
            data_decim: Decimator::new(&anti_alias, factor),
            pilot_narrow: ComplexOnePole::new(baseband_rate, PILOT_CUTOFF_HZ, PILOT_STAGES),
            pll: Pll::new(
                PILOT_LOOP_BW_HZ / baseband_rate,
                FRAC_1_SQRT_2,
                0.0,
                PILOT_RANGE_HZ / baseband_rate,
            ),
            pilot_locked: false,
            matched: Decimator::new(&design_rds_biphase(sps, SHAPING_SPAN_SYMBOLS), 1),
            timing: SymbolSync::new(sps, TIMING_LOOP_BW),
            phase: Costas::new(PHASE_LOOP_BW, FRAC_1_SQRT_2, 0.0, PHASE_RANGE_HZ / BIT_RATE),
            alphabet: tables::bpsk(),
            differential: DifferentialDecoder::new(),
            frames: GroupDecoder::default(),
            pilot_mix: Vec::new(),
            data_mix: Vec::new(),
            pilot_bb: Vec::new(),
            data_bb: Vec::new(),
            carrier_free: Vec::new(),
            shaped: Vec::new(),
            symbols: Vec::new(),
        }
    }

    pub(crate) fn process(&mut self, mpx: &[f32], out: &mut Vec<DecoderEvent>) {
        self.pilot_mix.clear();
        self.data_mix.clear();
        for &sample in mpx {
            let pilot = self.nco.next_sample();
            let subcarrier = pilot * pilot * pilot;
            self.pilot_mix.push(pilot.conj() * sample);
            self.data_mix.push(subcarrier.conj() * sample);
        }
        self.pilot_decim
            .process(&self.pilot_mix, &mut self.pilot_bb);
        self.data_decim.process(&self.data_mix, &mut self.data_bb);
        debug_assert_eq!(
            self.pilot_bb.len(),
            self.data_bb.len(),
            "the pilot and data paths drifted apart"
        );

        self.carrier_free.clear();
        for (&pilot, &data) in self.pilot_bb.iter().zip(&self.data_bb) {
            let _ = self.pll.process(self.pilot_narrow.process(pilot));
            self.pilot_locked |= self.pll.lock() > PILOT_LOCK_ON;
            self.carrier_free.push(if self.pilot_locked {
                data * self.pll.harmonic(3.0).conj()
            } else {
                data
            });
        }
        self.matched.process(&self.carrier_free, &mut self.shaped);

        self.symbols.clear();
        self.timing.process(&self.shaped, &mut self.symbols);
        for &symbol in &self.symbols {
            let level = self.alphabet.hard_slice(self.phase.process(symbol)) == 1;
            let bit = self.differential.decode(level);
            self.frames.push_bit(bit, out);
        }
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::new(self.mpx_rate);
    }
}

fn decimation(mpx_rate: f64) -> usize {
    let by_target = (mpx_rate / TARGET_BASEBAND_HZ).round();
    let ceiling = (mpx_rate / MIN_BASEBAND_HZ).floor();
    by_target.min(ceiling).max(1.0) as usize
}

fn anti_alias(mpx_rate: f64, baseband_rate: f64, factor: usize) -> Vec<f32> {
    let alias_hz = baseband_rate - DATA_EDGE_HZ;
    let taps = (BLACKMAN_TRANSITION * mpx_rate / (alias_hz - DATA_EDGE_HZ)).ceil() as usize;
    design_lowpass(
        taps.max(factor).max(3) | 1,
        0.5 * (DATA_EDGE_HZ + alias_hz) / mpx_rate,
    )
}

#[derive(Clone, Copy, Debug, Default)]
enum BlockSync {
    #[default]
    Hunt,
    Confirm {
        slot: usize,
        bits: usize,
    },
    Track {
        slot: usize,
        bits: usize,
        recent: u64,
    },
}

#[derive(Clone, Copy, Debug)]
struct Block {
    data: u16,
    trusted: bool,
}

#[derive(Default)]
struct GroupDecoder {
    window: u32,
    filled: usize,
    sync: BlockSync,
    blocks: [Option<Block>; BLOCKS_PER_GROUP],
    station: Station,
    groups: u64,
    blocks_read: u64,
    block_errors: u64,
    reported_errors: u64,
}

impl GroupDecoder {
    fn push_bit(&mut self, bit: bool, out: &mut Vec<DecoderEvent>) {
        self.window = ((self.window << 1) | u32::from(bit)) & BLOCK_MASK;
        self.filled = self.filled.saturating_add(1);
        if self.filled < BLOCK_BITS {
            return;
        }
        match self.sync {
            BlockSync::Hunt => self.hunt(),
            BlockSync::Confirm { slot, bits } if bits + 1 < BLOCK_BITS => {
                self.sync = BlockSync::Confirm {
                    slot,
                    bits: bits + 1,
                };
            }
            BlockSync::Confirm { slot, .. } => self.confirm(slot, out),
            BlockSync::Track { slot, bits, recent } if bits + 1 < BLOCK_BITS => {
                self.sync = BlockSync::Track {
                    slot,
                    bits: bits + 1,
                    recent,
                };
            }
            BlockSync::Track { slot, recent, .. } => self.close_block(slot, recent, out),
        }
    }

    fn hunt(&mut self) {
        for slot in 0..BLOCKS_PER_GROUP {
            if let Some(block) = check_slot(self.window, slot, false, None) {
                self.blocks = [None; BLOCKS_PER_GROUP];
                self.store(slot, Some(block));
                self.sync = BlockSync::Confirm {
                    slot: next_slot(slot),
                    bits: 0,
                };
                return;
            }
        }
    }

    fn confirm(&mut self, slot: usize, out: &mut Vec<DecoderEvent>) {
        let version_b = self.blocks[B_SLOT].map(|b| b.data & 0x0800 != 0);
        let Some(block) = check_slot(self.window, slot, false, version_b) else {
            self.blocks = [None; BLOCKS_PER_GROUP];
            self.sync = BlockSync::Hunt;
            return;
        };
        self.blocks_read = self.blocks_read.saturating_add(2);
        self.store(slot, Some(block));
        if slot == LAST_SLOT {
            self.close_group(out);
        }
        self.sync = BlockSync::Track {
            slot: next_slot(slot),
            bits: 0,
            recent: 0,
        };
    }

    fn close_block(&mut self, slot: usize, recent: u64, out: &mut Vec<DecoderEvent>) {
        let version_b = self.blocks[B_SLOT].map(|b| b.data & 0x0800 != 0);
        self.blocks_read = self.blocks_read.saturating_add(1);
        let block = check_slot(self.window, slot, true, version_b);
        if block.is_none() {
            self.block_errors = self.block_errors.saturating_add(1);
        }
        self.store(slot, block);
        let recent = recent_misses(recent, block.is_none());
        if recent.count_ones() > SYNC_WINDOW_LIMIT {
            self.blocks = [None; BLOCKS_PER_GROUP];
            self.sync = BlockSync::Hunt;
            return;
        }
        if slot == LAST_SLOT {
            self.close_group(out);
        }
        self.sync = BlockSync::Track {
            slot: next_slot(slot),
            bits: 0,
            recent,
        };
    }

    fn close_group(&mut self, out: &mut Vec<DecoderEvent>) {
        if self.blocks.iter().all(Option::is_some) {
            self.groups = self.groups.saturating_add(1);
        }
        let changed = self.station.apply(&self.blocks);
        if changed || self.block_errors != self.reported_errors {
            self.reported_errors = self.block_errors;
            out.push(DecoderEvent::Rds(self.station.update(
                self.groups,
                self.blocks_read,
                self.block_errors,
            )));
        }
        self.blocks = [None; BLOCKS_PER_GROUP];
    }

    fn store(&mut self, slot: usize, block: Option<Block>) {
        if let Some(cell) = self.blocks.get_mut(slot) {
            *cell = block;
        }
    }
}

const fn recent_misses(history: u64, missed: bool) -> u64 {
    ((history << 1) | missed as u64) & ((1 << SYNC_WINDOW_BLOCKS) - 1)
}

const fn next_slot(slot: usize) -> usize {
    (slot + 1) % BLOCKS_PER_GROUP
}

fn slot_offsets(slot: usize, version_b: Option<bool>) -> &'static [RdsOffset] {
    match slot {
        A_SLOT => &[RdsOffset::A],
        B_SLOT => &[RdsOffset::B],
        C_SLOT => match version_b {
            Some(false) => &[RdsOffset::C],
            Some(true) => &[RdsOffset::CPrime],
            None => &[RdsOffset::C, RdsOffset::CPrime],
        },
        LAST_SLOT => &[RdsOffset::D],
        _ => &[],
    }
}

fn check_slot(window: u32, slot: usize, correct: bool, version_b: Option<bool>) -> Option<Block> {
    let offsets = slot_offsets(slot, version_b);
    let strict = offsets
        .iter()
        .find_map(|&offset| rds_check_block(window, offset));
    if let Some(data) = strict {
        return Some(Block {
            data,
            trusted: true,
        });
    }
    if !correct {
        return None;
    }
    offsets
        .iter()
        .find_map(|&offset| rds_correct_block(window, offset))
        .map(|(data, _)| Block {
            data,
            trusted: false,
        })
}

struct TextField {
    chars: [u8; RT_LEN],
    seen: u64,
    staged: [u8; RT_LEN],
    staged_seen: u64,
    len: usize,
    terminated: bool,
    text: Option<String>,
}

impl TextField {
    fn new(len: usize, terminated: bool) -> Self {
        Self {
            chars: [0; RT_LEN],
            seen: 0,
            staged: [0; RT_LEN],
            staged_seen: 0,
            len,
            terminated,
            text: None,
        }
    }

    fn clear(&mut self) {
        self.chars = [0; RT_LEN];
        self.seen = 0;
    }

    fn write(&mut self, index: usize, chars: u16, trusted: bool) -> bool {
        for (offset, byte) in [(chars >> 8) as u8, chars as u8].into_iter().enumerate() {
            self.write_byte(index + offset, byte, trusted);
        }
        let complete = self.complete();
        publish(&mut self.text, complete)
    }

    fn write_byte(&mut self, at: usize, byte: u8, trusted: bool) {
        if at >= self.len {
            return;
        }
        let bit = 1u64 << at;
        let corroborated =
            self.staged_seen & bit != 0 && self.staged.get(at).copied() == Some(byte);
        if let Some(slot) = self.staged.get_mut(at) {
            *slot = byte;
        }
        self.staged_seen |= bit;
        if !trusted && !corroborated {
            return;
        }
        if self.seen & bit != 0 {
            if self.chars.get(at).copied() == Some(byte) {
                return;
            }
            self.clear();
        }
        if let Some(slot) = self.chars.get_mut(at) {
            *slot = byte;
        }
        self.seen |= bit;
    }

    fn complete(&self) -> Option<String> {
        let end = self.end();
        let needed = low_mask(end);
        (self.seen & needed == needed).then(|| text(self.chars.get(..end).unwrap_or_default()))
    }

    fn end(&self) -> usize {
        if !self.terminated {
            return self.len;
        }
        (0..self.len)
            .find(|&i| self.seen & (1u64 << i) != 0 && self.chars.get(i) == Some(&RT_TERMINATOR))
            .unwrap_or(self.len)
    }
}

fn low_mask(bits: usize) -> u64 {
    if bits >= u64::BITS as usize {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    }
}

struct Station {
    pi: Option<u16>,
    pty: Option<u8>,
    tp: Option<bool>,
    ta: Option<bool>,
    music: Option<bool>,
    ps: TextField,
    rt: TextField,
    rt_flag: Option<bool>,
    af: Vec<u8>,
    af_expected: usize,
}

impl Default for Station {
    fn default() -> Self {
        Self {
            pi: None,
            pty: None,
            tp: None,
            ta: None,
            music: None,
            ps: TextField::new(PS_LEN, false),
            rt: TextField::new(RT_LEN, true),
            rt_flag: None,
            af: Vec::new(),
            af_expected: 0,
        }
    }
}

impl Station {
    fn apply(&mut self, blocks: &[Option<Block>; BLOCKS_PER_GROUP]) -> bool {
        let Some(b) = blocks[B_SLOT] else {
            return false;
        };
        let header = b.data;
        let mut changed = blocks[A_SLOT]
            .filter(|a| a.trusted)
            .is_some_and(|a| set(&mut self.pi, a.data));
        if b.trusted {
            changed |= set(&mut self.tp, header & 0x0400 != 0);
            changed |= set(&mut self.pty, ((header >> 5) & 0x1F) as u8);
        }
        let version_b = header & 0x0800 != 0;
        let carries = |slot: usize| blocks[slot].map(|x| (x.data, b.trusted && x.trusted));
        match header >> 12 {
            0 => {
                if b.trusted {
                    changed |= set(&mut self.ta, header & 0x0010 != 0);
                    changed |= set(&mut self.music, header & 0x0008 != 0);
                }
                if let Some((d, trusted)) = carries(LAST_SLOT) {
                    changed |= self.ps.write(2 * usize::from(header & 0x0003), d, trusted);
                }
                if let Some((c, true)) = carries(C_SLOT).filter(|_| !version_b) {
                    changed |= self.push_af((c >> 8) as u8);
                    changed |= self.push_af(c as u8);
                }
            }
            2 => {
                let flag = header & 0x0010 != 0;
                if b.trusted {
                    if self.rt_flag.is_some_and(|previous| previous != flag) {
                        self.rt.clear();
                    }
                    self.rt_flag = Some(flag);
                }
                let segment = usize::from(header & 0x000F);
                if version_b {
                    if let Some((d, trusted)) = carries(LAST_SLOT) {
                        changed |= self.rt.write(2 * segment, d, trusted);
                    }
                } else {
                    if let Some((c, trusted)) = carries(C_SLOT) {
                        changed |= self.rt.write(4 * segment, c, trusted);
                    }
                    if let Some((d, trusted)) = carries(LAST_SLOT) {
                        changed |= self.rt.write(4 * segment + 2, d, trusted);
                    }
                }
            }
            _ => {}
        }
        changed
    }

    fn push_af(&mut self, code: u8) -> bool {
        match code {
            AF_COUNT_BASE..=AF_COUNT_TOP => {
                let count = usize::from(code - AF_COUNT_BASE);
                if count == self.af_expected {
                    return false;
                }
                self.af_expected = count;
                let had = !self.af.is_empty();
                self.af.clear();
                had
            }
            1..=AF_MAX_CODE => {
                if self.af.len() >= self.af_expected || self.af.contains(&code) {
                    return false;
                }
                self.af.push(code);
                true
            }
            _ => false,
        }
    }

    fn update(&self, groups: u64, blocks: u64, block_errors: u64) -> RdsUpdate {
        RdsUpdate {
            pi: self.pi.map(|pi| format!("{pi:04X}")),
            ps: self.ps.text.clone(),
            radiotext: self.rt.text.clone(),
            pty: self.pty,
            pty_name: self
                .pty
                .and_then(|pty| PTY_NAMES.get(usize::from(pty)))
                .map(|name| (*name).to_owned()),
            tp: self.tp,
            ta: self.ta,
            music: self.music,
            alt_freqs_hz: self
                .af
                .iter()
                .map(|&code| AF_BASE_HZ + AF_STEP_HZ * f64::from(code))
                .collect(),
            groups,
            blocks,
            block_errors,
        }
    }
}

fn set<T: PartialEq>(slot: &mut Option<T>, value: T) -> bool {
    let moved = slot.as_ref() != Some(&value);
    if moved {
        *slot = Some(value);
    }
    moved
}

fn publish(slot: &mut Option<String>, value: Option<String>) -> bool {
    match value {
        Some(text) if slot.as_deref() != Some(text.as_str()) => {
            *slot = Some(text);
            true
        }
        _ => false,
    }
}

fn text(raw: &[u8]) -> String {
    raw.iter()
        .map(|&c| {
            usize::from(c)
                .checked_sub(FIRST_GLYPH)
                .and_then(|i| EBU_LATIN.get(i))
                .copied()
                .unwrap_or('?')
        })
        .collect::<String>()
        .trim_end()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use sdrmm_dsp::rds_encode_block;
    use sdrmm_modem::analog::{AngleDemod, AngleDetector, AngleKind, AngleParams, AngleRx};

    use super::*;
    use crate::testgen::{
        add_noise, fm_modulate,
        rds::{Station as TxStation, composite, groups as tx_groups, monophonic},
    };

    const RATE: f64 = 240_000.0;
    const DEVIATION_HZ: f64 = 75_000.0;
    const AUDIO_EDGE_HZ: f64 = 15_000.0;
    const GROUP_BITS: usize = BLOCK_BITS * BLOCKS_PER_GROUP;

    fn station() -> TxStation {
        TxStation {
            pi: 0xD3C2,
            ps: "SDR--FM".to_owned(),
            radiotext: "sdr-- reference transmission".to_owned(),
            pty: 10,
            tp: true,
            ta: false,
            music: true,
            alt_freqs_hz: vec![89_800_000.0, 95_100_000.0, 103_500_000.0],
        }
    }

    fn bits_of(blocks: &[u32]) -> Vec<bool> {
        let mut bits = Vec::with_capacity(blocks.len() * BLOCK_BITS);
        for &block in blocks {
            for k in (0..BLOCK_BITS).rev() {
                bits.push(block >> k & 1 != 0);
            }
        }
        bits
    }

    fn drive(bits: &[bool]) -> (GroupDecoder, Vec<DecoderEvent>) {
        let mut decoder = GroupDecoder::default();
        let mut events = Vec::new();
        for &bit in bits {
            decoder.push_bit(bit, &mut events);
        }
        (decoder, events)
    }

    fn last_update(events: &[DecoderEvent]) -> RdsUpdate {
        match events.last() {
            Some(DecoderEvent::Rds(update)) => update.clone(),
            other => panic!("expected an rds update, got {other:?}"),
        }
    }

    fn pair(hi: u8, lo: u8) -> u16 {
        u16::from(hi) << 8 | u16::from(lo)
    }

    fn radiotext_groups(pi: u16, message: &str, flag: bool) -> Vec<u32> {
        let mut bytes: Vec<u8> = message.bytes().take(RT_LEN).collect();
        if bytes.len() < RT_LEN {
            bytes.push(RT_TERMINATOR);
        }
        while !bytes.len().is_multiple_of(4) {
            bytes.push(b' ');
        }
        bytes
            .chunks(4)
            .enumerate()
            .flat_map(|(segment, chunk)| {
                let b = (2 << 12) | (u16::from(flag) << 4) | segment as u16;
                [
                    rds_encode_block(pi, RdsOffset::A),
                    rds_encode_block(b, RdsOffset::B),
                    rds_encode_block(pair(chunk[0], chunk[1]), RdsOffset::C),
                    rds_encode_block(pair(chunk[2], chunk[3]), RdsOffset::D),
                ]
            })
            .collect()
    }

    fn radiotexts(events: &[DecoderEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                DecoderEvent::Rds(update) => update.radiotext.clone(),
                _ => None,
            })
            .collect()
    }

    fn over_the_air(mpx: &[f32], noise: f32) -> Vec<f32> {
        let mut iq = fm_modulate(mpx, DEVIATION_HZ, RATE);
        add_noise(&mut iq, 0x5eed_1234, noise);
        let mut fm = AngleDemod::new(
            &AngleParams::new(
                AngleKind::Fm {
                    deviation: DEVIATION_HZ / RATE,
                },
                AUDIO_EDGE_HZ / RATE,
            ),
            &AngleRx::detector_only(AngleDetector::Discriminator),
        );
        let mut out = Vec::new();
        fm.process(&iq, &mut out);
        out
    }

    fn run(decoder: &mut RdsDecoder, mpx: &[f32]) -> Vec<DecoderEvent> {
        let mut events = Vec::new();
        let mut pos = 0;
        for len in [4_096usize, 1, 997, 65, 8_192, 7].iter().cycle() {
            if pos >= mpx.len() {
                break;
            }
            let end = (pos + len).min(mpx.len());
            decoder.process(&mpx[pos..end], &mut events);
            pos = end;
        }
        events
    }

    #[test]
    fn block_sync_finds_the_group_boundary_from_any_starting_offset() {
        let bits = bits_of(&tx_groups(&station(), 24));
        for skip in [0usize, 1, 7, 13, 25, 26, 51, 104, 137] {
            let (decoder, events) = drive(&bits[skip..]);
            assert!(
                decoder.groups >= 18,
                "skip {skip}: only {} groups",
                decoder.groups
            );
            assert!(
                decoder.block_errors <= 2,
                "skip {skip}: {} block errors",
                decoder.block_errors
            );
            let update = last_update(&events);
            assert_eq!(update.pi.as_deref(), Some("D3C2"), "skip {skip}");
            assert_eq!(update.ps.as_deref(), Some("SDR--FM"), "skip {skip}");
        }
    }

    #[test]
    fn a_lone_matching_offset_word_in_noise_never_declares_sync() {
        let mut rng = 0x1234_5678u32;
        let bits: Vec<bool> = (0..40_000)
            .map(|_| {
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                rng & 1 != 0
            })
            .collect();
        let (decoder, events) = drive(&bits);
        assert!(events.is_empty(), "noise produced {} events", events.len());
        assert_eq!(decoder.groups, 0, "noise assembled a group");
        assert_eq!(
            decoder.blocks_read, 0,
            "an unconfirmed offset match was read as a block"
        );
        assert_eq!(
            decoder.block_errors, 0,
            "hunting for sync was charged as block errors"
        );
    }

    #[test]
    fn the_block_counter_covers_every_block_the_error_count_is_measured_against() {
        let (decoder, _) = drive(&bits_of(&tx_groups(&station(), 40)));
        assert_eq!(decoder.block_errors, 0);
        assert_eq!(
            decoder.blocks_read,
            BLOCKS_PER_GROUP as u64 * decoder.groups,
            "{} blocks read for {} groups",
            decoder.blocks_read,
            decoder.groups
        );
    }

    #[test]
    fn text_reads_the_ebu_latin_repertoire_of_annex_e() {
        assert_eq!(text(b"Koeln"), "Koeln");
        assert_eq!(text(&[0x91, 0x9B, 0x97, 0x99, 0x8D]), "äçöüß");
        assert_eq!(text(&[0x24, 0xAB, 0xAA]), "¤$£");
        assert_eq!(text(&[0x00, 0x1F]), "??");
    }

    #[test]
    fn groups_reassemble_the_whole_station_picture() {
        let (_, events) = drive(&bits_of(&tx_groups(&station(), 40)));
        let update = last_update(&events);
        assert_eq!(update.pi.as_deref(), Some("D3C2"));
        assert_eq!(update.ps.as_deref(), Some("SDR--FM"));
        assert_eq!(
            update.radiotext.as_deref(),
            Some("sdr-- reference transmission")
        );
        assert_eq!(update.pty, Some(10));
        assert_eq!(update.pty_name.as_deref(), Some("Pop Music"));
        assert_eq!(update.tp, Some(true));
        assert_eq!(update.ta, Some(false));
        assert_eq!(update.music, Some(true));
        assert_eq!(
            update.alt_freqs_hz,
            vec![89_800_000.0, 95_100_000.0, 103_500_000.0]
        );
        assert_eq!(update.block_errors, 0);
    }

    #[test]
    fn a_radiotext_of_exactly_64_characters_completes_without_a_terminator() {
        let mut tx = station();
        tx.radiotext = "0123456789".repeat(6) + "abcd";
        assert_eq!(tx.radiotext.len(), RT_LEN);
        let (_, events) = drive(&bits_of(&tx_groups(&tx, 60)));
        assert_eq!(
            last_update(&events).radiotext.as_deref(),
            Some(tx.radiotext.as_str())
        );
    }

    #[test]
    fn the_text_ab_flag_starts_a_new_message() {
        let mut bits = bits_of(&tx_groups(&station(), 40));
        let mut second = station();
        second.radiotext = "second message".to_owned();
        let toggled: Vec<u32> = tx_groups(&second, 40)
            .into_iter()
            .enumerate()
            .map(|(i, block)| match rds_check_block(block, RdsOffset::B) {
                Some(data) if i % BLOCKS_PER_GROUP == 1 && data >> 12 == 2 => {
                    rds_encode_block(data ^ 0x0010, RdsOffset::B)
                }
                _ => block,
            })
            .collect();
        bits.extend(bits_of(&toggled));

        let (_, events) = drive(&bits);
        assert_eq!(
            last_update(&events).radiotext.as_deref(),
            Some("second message")
        );
    }

    #[test]
    fn block_errors_are_counted_and_sync_is_regained_after_a_burst() {
        let mut bits = bits_of(&tx_groups(&station(), 60));
        for bit in bits.iter_mut().skip(2 * GROUP_BITS).take(4 * GROUP_BITS) {
            *bit = !*bit;
        }
        let (decoder, events) = drive(&bits);
        assert!(decoder.block_errors > 0, "the damage went uncounted");
        assert!(
            decoder.groups >= 50,
            "sync never came back: {} groups",
            decoder.groups
        );
        let update = last_update(&events);
        assert_eq!(update.ps.as_deref(), Some("SDR--FM"));
        assert_eq!(
            update.radiotext.as_deref(),
            Some("sdr-- reference transmission")
        );
        assert!(
            update.block_errors > 0,
            "the picture completed without ever noticing the damage"
        );
    }

    #[test]
    fn a_long_dead_patch_drops_sync_and_the_group_boundary_is_found_again() {
        const DEAD_BLOCKS: u64 = 40;
        let clean = bits_of(&tx_groups(&station(), 30));
        let carried = BLOCKS_PER_GROUP as u64 * 30;
        let mut bits = clean.clone();
        bits.extend(std::iter::repeat_n(true, DEAD_BLOCKS as usize * BLOCK_BITS));
        bits.extend(&clean);

        let (decoder, _) = drive(&bits);
        assert!(
            decoder.block_errors <= u64::from(SYNC_WINDOW_LIMIT) + 1,
            "{} blocks were charged before sync was given up",
            decoder.block_errors
        );
        assert!(
            decoder.groups >= 50,
            "only {} groups across the gap",
            decoder.groups
        );
        assert!(
            decoder.blocks_read < 2 * carried + DEAD_BLOCKS,
            "{} blocks read: the hunt for sync was counted too",
            decoder.blocks_read
        );
    }

    #[test]
    fn a_station_that_swaps_radiotext_without_the_ab_flag_shows_whole_messages_only() {
        let traffic = "Verkehr: Staus auf der A1 zwischen Koeln und Bonn";
        let promo = "Jetzt: Die WDR 2 Musikwelt mit den besten Songs";
        let mut blocks = Vec::new();
        for round in 0..6 {
            let message = if round % 2 == 0 { traffic } else { promo };
            blocks.extend(radiotext_groups(0xD392, message, false));
        }
        let (_, events) = drive(&bits_of(&blocks));
        let shown = radiotexts(&events);
        for message in &shown {
            assert!(
                message == traffic || message == promo,
                "two messages blended into {message:?}"
            );
        }
        assert!(shown.iter().any(|message| message == traffic));
        assert!(shown.iter().any(|message| message == promo));
    }

    #[test]
    fn a_group_still_carries_its_text_when_block_a_never_arrives() {
        let message = "radiotext without a programme identification";
        let mut blocks = radiotext_groups(0xD392, message, false).repeat(2);
        for slot in blocks.iter_mut().step_by(BLOCKS_PER_GROUP) {
            *slot ^= 0x1A5_3C7B & BLOCK_MASK;
        }
        let (_, events) = drive(&bits_of(&blocks));
        let update = last_update(&events);
        assert_eq!(update.radiotext.as_deref(), Some(message));
        assert_eq!(update.pi, None, "a wrecked block a was read anyway");
    }

    #[test]
    fn adjacent_bit_damage_is_repaired_once_the_group_boundary_is_known() {
        let message = "two neighbouring bits die in every third block";
        let clean = radiotext_groups(0xD392, message, false).repeat(3);
        let damaged: Vec<u32> = clean
            .iter()
            .enumerate()
            .map(|(index, &block)| {
                if index % 3 == 2 {
                    block ^ (0b11 << (index % (BLOCK_BITS - 1)))
                } else {
                    block
                }
            })
            .collect();
        let (decoder, events) = drive(&bits_of(&damaged));
        assert_eq!(last_update(&events).radiotext.as_deref(), Some(message));
        assert!(
            decoder.block_errors <= 2,
            "{} blocks were given up on",
            decoder.block_errors
        );
    }

    #[test]
    fn an_untrusted_character_waits_for_a_second_sighting() {
        let mut field = TextField::new(4, false);
        field.write(0, pair(b'A', b'B'), false);
        field.write(2, pair(b'C', b'D'), true);
        assert_eq!(
            field.text, None,
            "a lone repaired block completed a message"
        );
        field.write(0, pair(b'A', b'B'), false);
        assert_eq!(field.text.as_deref(), Some("ABCD"));
    }

    #[test]
    fn a_character_that_stops_matching_starts_a_new_message() {
        let mut field = TextField::new(4, false);
        field.write(0, pair(b'A', b'B'), true);
        field.write(2, pair(b'C', b'D'), true);
        assert_eq!(field.text.as_deref(), Some("ABCD"));

        field.write(0, pair(b'W', b'X'), true);
        assert_eq!(
            field.text.as_deref(),
            Some("ABCD"),
            "half of the old message was published as a message"
        );
        field.write(2, pair(b'Y', b'Z'), true);
        assert_eq!(field.text.as_deref(), Some("WXYZ"));
    }

    #[test]
    fn a_steady_station_stops_producing_events_once_it_is_known() {
        let (decoder, events) = drive(&bits_of(&tx_groups(&station(), 400)));
        assert_eq!(decoder.groups, 400);
        assert!(
            (1..=8).contains(&events.len()),
            "400 groups produced {} events",
            events.len()
        );
    }

    #[test]
    fn a_full_transmission_decodes_through_the_analog_front_end() {
        let mut decoder = RdsDecoder::new(RATE);
        let events = run(
            &mut decoder,
            &composite(&station(), 3.5, Some(1_000.0), RATE),
        );
        let update = last_update(&events);
        assert_eq!(update.pi.as_deref(), Some("D3C2"));
        assert_eq!(update.ps.as_deref(), Some("SDR--FM"));
        assert_eq!(
            update.radiotext.as_deref(),
            Some("sdr-- reference transmission")
        );
        assert_eq!(update.pty_name.as_deref(), Some("Pop Music"));
        assert_eq!(update.tp, Some(true));
        assert_eq!(update.music, Some(true));
        assert_eq!(
            update.alt_freqs_hz,
            vec![89_800_000.0, 95_100_000.0, 103_500_000.0]
        );
        assert!(
            decoder.frames.groups >= 34,
            "only {} groups",
            decoder.frames.groups
        );
        assert_eq!(decoder.frames.block_errors, 0);
    }

    #[test]
    fn a_monophonic_broadcast_decodes_without_a_pilot_to_lock_to() {
        let mut decoder = RdsDecoder::new(RATE);
        let events = run(
            &mut decoder,
            &monophonic(&station(), 4.0, Some(1_000.0), RATE),
        );
        let update = last_update(&events);
        assert_eq!(update.pi.as_deref(), Some("D3C2"));
        assert_eq!(update.ps.as_deref(), Some("SDR--FM"));
        assert_eq!(
            update.radiotext.as_deref(),
            Some("sdr-- reference transmission")
        );
        assert_eq!(decoder.frames.block_errors, 0);
    }

    #[test]
    fn a_noisy_channel_costs_few_blocks_at_the_recommended_subcarrier_level() {
        let mut decoder = RdsDecoder::new(RATE);
        let mpx = composite(&station(), 12.0, Some(1_000.0), RATE);
        let events = run(&mut decoder, &over_the_air(&mpx, 0.25));
        let update = last_update(&events);
        assert_eq!(update.ps.as_deref(), Some("SDR--FM"));
        assert_eq!(
            update.radiotext.as_deref(),
            Some("sdr-- reference transmission")
        );
        let (blocks, errors) = (decoder.frames.blocks_read, decoder.frames.block_errors);
        assert!(blocks > 500, "only {blocks} blocks read");
        let lost = errors as f64 / blocks as f64;
        assert!(lost < 0.02, "lost {:.1}% of the blocks", 100.0 * lost);
    }

    #[test]
    fn reset_forgets_the_station() {
        let mut decoder = RdsDecoder::new(RATE);
        let events = run(&mut decoder, &composite(&station(), 3.0, None, RATE));
        assert!(!events.is_empty());
        decoder.reset();
        assert_eq!(decoder.frames.groups, 0);
        assert_eq!(decoder.frames.station.ps.text, None);
    }
}
