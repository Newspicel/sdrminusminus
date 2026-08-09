//! RDS decoder (PLAN §13 P2): the 57 kHz DBPSK subcarrier on the FM composite, per
//! EN 50067 / IEC 62106.
//!
//! Carrier recovery rides on the 19 kHz pilot rather than on the data: the pilot is several dB
//! stronger than the subcarrier and always present, and the subcarrier is locked to its third
//! harmonic (§1.2.1), so cubing the pilot phasor hands the data path a carrier that needs no
//! acquisition. What is left over is a fixed rotation — the standard also permits the
//! subcarrier to sit in quadrature with that harmonic — which a decision-directed loop on the
//! recovered symbols removes; differential decoding then makes its residual 180° ambiguity
//! irrelevant.
//!
//! The composite is mixed before it is filtered. A real bandpass at the composite rate
//! followed by a mixer is the same filter as a mixer followed by the translated lowpass, but
//! the polyphase decimator evaluates the latter only at the instants it keeps — the same
//! response for a fraction of the multiplies — and it also rejects the −114 kHz mixer image
//! that a bandpass-first chain would fold straight back onto the data.

use std::f64::consts::FRAC_1_SQRT_2;

use num_complex::Complex;
use sdrmm_dsp::{
    Costas, Decimator, DifferentialDecoder, Nco, Pll, RdsOffset, SymbolSync, design_lowpass,
    rds_check_block,
};
use sdrmm_wire::{DecoderEvent, RdsUpdate};

/// Bit rate: the subcarrier divided by 48 (EN 50067 §1.2.2).
const BIT_RATE: f64 = 1_187.5;
/// Stereo pilot; the subcarrier is its third harmonic (EN 50067 §1.2.1).
const PILOT_HZ: f64 = 19_000.0;
/// The shaped data spectrum ends at twice the bit rate (EN 50067 §1.2.4).
const DATA_EDGE_HZ: f64 = 2.0 * BIT_RATE;
/// Nearest composite neighbour of the subcarrier: the stereo difference signal ends at
/// 53 kHz, 4 kHz below it.
const NEIGHBOUR_HZ: f64 = 4_000.0;
/// Symbol-loop rate aimed for; the decimation factor is the integer that lands nearest it.
const TARGET_BASEBAND_HZ: f64 = 9_600.0;
/// Floor for that rate: three times the data edge keeps the anti-alias filter realisable and
/// the timing loop above its two-samples-per-symbol minimum.
const MIN_BASEBAND_HZ: f64 = 3.0 * DATA_EDGE_HZ;
/// The pilot lands at DC after its mixer, so its filter only has to hold off the nearest
/// composite neighbours — audio ends 4 kHz below 19 kHz, the stereo subcarrier starts 4 kHz
/// above it.
const PILOT_CUTOFF_HZ: f64 = 1_000.0;
/// Blackman transition width, in taps·cycles-per-sample (see `sdrmm_dsp::fir`).
const BLACKMAN_TRANSITION: f64 = 5.5;

/// Pilot loop bandwidth, normalised to the baseband rate (~20 Hz). Narrow, because every
/// radian of pilot phase error is tripled onto the data carrier.
const PILOT_LOOP_BW: f64 = 0.002;
/// Pull-in range around the pilot, as a fraction of the baseband rate.
const PILOT_RANGE: f64 = 0.001;
/// Timing loop bandwidth, in cycles per symbol.
const TIMING_LOOP_BW: f64 = 0.01;
/// Residual-phase loop, in cycles per symbol. It has only a fixed rotation to remove, so it
/// may be fast and its frequency integrator stays nearly pinned.
const PHASE_LOOP_BW: f64 = 0.02;
const PHASE_RANGE: f64 = 0.005;

const BLOCK_BITS: usize = 26;
const BLOCK_MASK: u32 = (1 << BLOCK_BITS) - 1;
const BLOCKS_PER_GROUP: usize = 4;
/// Slot of block C — the one sent with either the C or the C′ offset word.
const C_SLOT: usize = 2;
const LAST_SLOT: usize = BLOCKS_PER_GROUP - 1;
/// Bad blocks tolerated before the block clock is abandoned and the offset words are hunted
/// again. Three groups' worth: long enough to ride out a flutter fade, short enough that a
/// clock stolen by a chance syndrome hit is dropped within a quarter of a second.
const MAX_BLOCK_MISSES: u32 = 12;

const PS_LEN: usize = 8;
/// Every PS segment seen.
const PS_COMPLETE: u8 = u8::MAX;
const RT_LEN: usize = 64;
/// Ends a RadioText message shorter than 64 characters (EN 50067 §3.1.5.3).
const RT_TERMINATOR: u8 = 0x0D;

/// AF coding (EN 50067 §3.2.1.6.1): 224+n announces n alternative frequencies and 1..=204 are
/// 87.5 MHz + 100 kHz·code. Everything else — 0 "not used", 205..=223 filler, 250 "LF/MF
/// follows", 251..=255 spare — carries no VHF frequency.
const AF_COUNT_BASE: u8 = 224;
const AF_COUNT_TOP: u8 = 249;
const AF_MAX_CODE: u8 = 204;
const AF_BASE_HZ: f64 = 87_500_000.0;
const AF_STEP_HZ: f64 = 100_000.0;

/// Programme Type names, RDS (Europe) variant — EN 50067 Annex F table F.1. The RBDS (North
/// America) table gives the same codes different names and is deliberately not used here.
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

/// Receiver for the RDS subcarrier of an FM composite. `new` expects a composite rate high
/// enough to carry 57 kHz at all — the WFM channel's 240 kHz is the only one in the tree.
pub(crate) struct RdsDecoder {
    mpx_rate: f64,
    nco: Nco,
    pilot_decim: Decimator,
    data_decim: Decimator,
    pll: Pll,
    matched: Decimator,
    timing: SymbolSync,
    phase: Costas,
    differential: DifferentialDecoder,
    frames: GroupDecoder,
    /// Scratch reused across calls: after warm-up `process` allocates nothing.
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
        let data_lp = anti_alias(mpx_rate, baseband_rate, factor);
        // Same length as the data filter, so both paths carry the same group delay and the
        // pilot correction stays aligned with the samples it corrects.
        let pilot_lp = design_lowpass(data_lp.len(), PILOT_CUTOFF_HZ / mpx_rate);
        Self {
            mpx_rate,
            nco: Nco::new(PILOT_HZ as f32, mpx_rate as f32),
            pilot_decim: Decimator::new(&pilot_lp, factor),
            data_decim: Decimator::new(&data_lp, factor),
            pll: Pll::new(PILOT_LOOP_BW, FRAC_1_SQRT_2, 0.0, PILOT_RANGE),
            matched: Decimator::new(&matched_taps(sps, baseband_rate), 1),
            timing: SymbolSync::new(sps, TIMING_LOOP_BW),
            phase: Costas::new(PHASE_LOOP_BW, FRAC_1_SQRT_2, 0.0, PHASE_RANGE),
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

    /// Feed a block of composite samples; push an [`RdsUpdate`] whenever a decoded group
    /// actually changed the station picture.
    pub(crate) fn process(&mut self, mpx: &[f32], out: &mut Vec<DecoderEvent>) {
        self.pilot_mix.clear();
        self.data_mix.clear();
        for &sample in mpx {
            let pilot = self.nco.next_sample();
            // The subcarrier is the pilot's third harmonic, so cubing the same phasor keeps
            // the two mixers exactly locked whatever the oscillator's own rounding does.
            let subcarrier = pilot * pilot * pilot;
            self.pilot_mix.push(pilot.conj() * sample);
            self.data_mix.push(subcarrier.conj() * sample);
        }
        self.pilot_decim
            .process(&self.pilot_mix, &mut self.pilot_bb);
        self.data_decim.process(&self.data_mix, &mut self.data_bb);

        self.carrier_free.clear();
        for (&pilot, &data) in self.pilot_bb.iter().zip(&self.data_bb) {
            let _ = self.pll.process(pilot);
            self.carrier_free.push(data * self.pll.harmonic(3.0).conj());
        }
        self.matched.process(&self.carrier_free, &mut self.shaped);

        self.symbols.clear();
        self.timing.process(&self.shaped, &mut self.symbols);
        for &symbol in &self.symbols {
            let level = self.phase.process(symbol).re >= 0.0;
            let bit = self.differential.decode(level);
            self.frames.push_bit(bit, out);
        }
    }

    /// Drop everything the decoder knows, the station picture included — what the channel
    /// needs when it is retuned, so one station's PS name can never accrete onto another's.
    /// Rebuilding is the only way to clear the analog loops, which have no reset of their own.
    pub(crate) fn reset(&mut self) {
        *self = Self::new(self.mpx_rate);
    }
}

fn decimation(mpx_rate: f64) -> usize {
    let by_target = (mpx_rate / TARGET_BASEBAND_HZ).round();
    let ceiling = (mpx_rate / MIN_BASEBAND_HZ).floor();
    by_target.min(ceiling).max(1.0) as usize
}

/// Anti-alias lowpass for the data path: flat to the data edge, in full stopband before the
/// lowest frequency that would fold onto it, and the −6 dB point midway between the two.
fn anti_alias(mpx_rate: f64, baseband_rate: f64, factor: usize) -> Vec<f32> {
    let alias_hz = baseband_rate - DATA_EDGE_HZ;
    let taps = (BLACKMAN_TRANSITION * mpx_rate / (alias_hz - DATA_EDGE_HZ)).ceil() as usize;
    design_lowpass(
        taps.max(factor).max(3) | 1,
        0.5 * (DATA_EDGE_HZ + alias_hz) / mpx_rate,
    )
}

/// Receive filter: the biphase symbol correlator — `+1` over the first half of a bit, `−1`
/// over the second — convolved with a lowpass that trims the composite-rate decimator's wide
/// passband down to the data spectrum, so a stereo subcarrier's 53 kHz edge cannot reach the
/// slicer.
fn matched_taps(sps: f64, baseband_rate: f64) -> Vec<f32> {
    let span = sps.round().max(2.0) as usize;
    let biphase: Vec<f32> = (0..span)
        .map(|k| {
            if (k as f64 + 0.5) < 0.5 * sps {
                1.0
            } else {
                -1.0
            }
        })
        .collect();
    let taps =
        (BLACKMAN_TRANSITION * baseband_rate / (NEIGHBOUR_HZ - DATA_EDGE_HZ)).ceil() as usize;
    let band = design_lowpass(
        taps.max(3) | 1,
        0.5 * (DATA_EDGE_HZ + NEIGHBOUR_HZ) / baseband_rate,
    );
    convolve(&biphase, &band)
}

fn convolve(a: &[f32], b: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0f32; a.len() + b.len() - 1];
    for (i, &x) in a.iter().enumerate() {
        for (slot, &y) in out[i..].iter_mut().zip(b) {
            *slot += x * y;
        }
    }
    out
}

/// Where the block clock stands.
#[derive(Clone, Copy, Debug, Default)]
enum BlockSync {
    /// Sliding the 26-bit window past every offset word, looking for a block boundary.
    #[default]
    Hunt,
    /// Following the clock. `confirmed` turns on once a second block lands where the first
    /// predicted it, which is what stops a 1-in-1024 chance syndrome hit from stealing it.
    Track {
        /// Slot the next block boundary belongs to.
        slot: usize,
        bits: usize,
        misses: u32,
        confirmed: bool,
    },
}

/// Bits in, groups out: block synchronisation, group interpretation, and the emit-on-change
/// rule that keeps a station repeating itself eleven times a second off the event log.
#[derive(Default)]
struct GroupDecoder {
    window: u32,
    filled: usize,
    sync: BlockSync,
    blocks: [Option<u16>; BLOCKS_PER_GROUP],
    station: Station,
    groups: u64,
    block_errors: u64,
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
            BlockSync::Track {
                slot,
                bits,
                misses,
                confirmed,
            } => {
                let bits = bits + 1;
                if bits < BLOCK_BITS {
                    self.sync = BlockSync::Track {
                        slot,
                        bits,
                        misses,
                        confirmed,
                    };
                } else {
                    self.close_block(slot, misses, confirmed, out);
                }
            }
        }
    }

    fn hunt(&mut self) {
        for slot in 0..BLOCKS_PER_GROUP {
            if let Some(data) = check_slot(self.window, slot) {
                self.blocks = [None; BLOCKS_PER_GROUP];
                self.store(slot, Some(data));
                self.sync = BlockSync::Track {
                    slot: next_slot(slot),
                    bits: 0,
                    misses: 0,
                    confirmed: false,
                };
                return;
            }
        }
    }

    fn close_block(
        &mut self,
        slot: usize,
        misses: u32,
        confirmed: bool,
        out: &mut Vec<DecoderEvent>,
    ) {
        let (misses, confirmed) = match check_slot(self.window, slot) {
            Some(data) => {
                self.store(slot, Some(data));
                (0, true)
            }
            None => {
                self.store(slot, None);
                self.block_errors = self.block_errors.saturating_add(1);
                let misses = misses + 1;
                if !confirmed || misses > MAX_BLOCK_MISSES {
                    self.blocks = [None; BLOCKS_PER_GROUP];
                    self.sync = BlockSync::Hunt;
                    return;
                }
                (misses, confirmed)
            }
        };
        if slot == LAST_SLOT {
            self.close_group(out);
        }
        self.sync = BlockSync::Track {
            slot: next_slot(slot),
            bits: 0,
            misses,
            confirmed,
        };
    }

    /// A group is only interpreted with all four blocks intact. Salvaging the good blocks of
    /// a damaged group would buy a few extra characters at the price of letting a mis-keyed
    /// segment address rewrite the PS name.
    fn close_group(&mut self, out: &mut Vec<DecoderEvent>) {
        if let [Some(a), Some(b), Some(c), Some(d)] = self.blocks {
            self.groups = self.groups.saturating_add(1);
            if self.station.apply(a, b, c, d) {
                let update = self.station.update(self.groups, self.block_errors);
                out.push(DecoderEvent::Rds(update));
            }
        }
        self.blocks = [None; BLOCKS_PER_GROUP];
    }

    fn store(&mut self, slot: usize, data: Option<u16>) {
        if let Some(cell) = self.blocks.get_mut(slot) {
            *cell = data;
        }
    }
}

const fn next_slot(slot: usize) -> usize {
    (slot + 1) % BLOCKS_PER_GROUP
}

/// Block C is sent with offset C in a version-A group and C′ in a version-B one, so slot 2
/// accepts either; which one arrived is redundant with the version bit of block B.
fn check_slot(window: u32, slot: usize) -> Option<u16> {
    match slot {
        0 => rds_check_block(window, RdsOffset::A),
        1 => rds_check_block(window, RdsOffset::B),
        C_SLOT => rds_check_block(window, RdsOffset::C)
            .or_else(|| rds_check_block(window, RdsOffset::CPrime)),
        LAST_SLOT => rds_check_block(window, RdsOffset::D),
        _ => None,
    }
}

/// Everything the received groups say about the station. A field only becomes `Some` once a
/// group has actually carried it, so a fresh channel reports what it knows and nothing else.
struct Station {
    pi: Option<u16>,
    pty: Option<u8>,
    tp: Option<bool>,
    ta: Option<bool>,
    music: Option<bool>,
    ps: [u8; PS_LEN],
    ps_seen: u8,
    ps_text: Option<String>,
    rt: [u8; RT_LEN],
    rt_seen: u64,
    rt_flag: Option<bool>,
    rt_text: Option<String>,
    /// Alternative frequencies as the codes they were sent as, plus how many the station said
    /// it would send.
    af: Vec<u8>,
    af_expected: usize,
}

// `[u8; 64]` is past the array sizes `Default` is derived for.
impl Default for Station {
    fn default() -> Self {
        Self {
            pi: None,
            pty: None,
            tp: None,
            ta: None,
            music: None,
            ps: [0; PS_LEN],
            ps_seen: 0,
            ps_text: None,
            rt: [0; RT_LEN],
            rt_seen: 0,
            rt_flag: None,
            rt_text: None,
            af: Vec::new(),
            af_expected: 0,
        }
    }
}

impl Station {
    /// Fold one intact group into the picture; true when a published field moved.
    fn apply(&mut self, a: u16, b: u16, c: u16, d: u16) -> bool {
        let mut changed = set(&mut self.pi, a);
        changed |= set(&mut self.tp, b & 0x0400 != 0);
        changed |= set(&mut self.pty, ((b >> 5) & 0x1F) as u8);
        let version_b = b & 0x0800 != 0;
        match b >> 12 {
            // 0A/0B: a PS name segment, the traffic and music flags, and — version A only —
            // two bytes of the alternative frequency list.
            0 => {
                changed |= set(&mut self.ta, b & 0x0010 != 0);
                changed |= set(&mut self.music, b & 0x0008 != 0);
                changed |= self.set_ps(2 * usize::from(b & 0x0003), d);
                if !version_b {
                    changed |= self.push_af((c >> 8) as u8);
                    changed |= self.push_af(c as u8);
                }
            }
            // 2A/2B: RadioText. Version A carries four characters per group, version B two,
            // with block C repeating the PI code instead.
            2 => {
                let flag = b & 0x0010 != 0;
                if self.rt_flag.is_some_and(|previous| previous != flag) {
                    self.rt = [0; RT_LEN];
                    self.rt_seen = 0;
                }
                self.rt_flag = Some(flag);
                let segment = usize::from(b & 0x000F);
                changed |= if version_b {
                    self.set_rt(2 * segment, d)
                } else {
                    self.set_rt(4 * segment, c) | self.set_rt(4 * segment + 2, d)
                };
            }
            _ => {}
        }
        changed
    }

    fn set_ps(&mut self, index: usize, chars: u16) -> bool {
        let seen = self.ps_seen;
        let mut moved = false;
        for (offset, byte) in [(chars >> 8) as u8, chars as u8].into_iter().enumerate() {
            let at = index + offset;
            if let Some(slot) = self.ps.get_mut(at) {
                moved |= *slot != byte;
                *slot = byte;
                self.ps_seen |= 1 << at;
            }
        }
        if !moved && seen == self.ps_seen {
            return false;
        }
        publish(
            &mut self.ps_text,
            (self.ps_seen == PS_COMPLETE).then(|| text(&self.ps)),
        )
    }

    fn set_rt(&mut self, index: usize, chars: u16) -> bool {
        let seen = self.rt_seen;
        let mut moved = false;
        for (offset, byte) in [(chars >> 8) as u8, chars as u8].into_iter().enumerate() {
            let at = index + offset;
            if let Some(slot) = self.rt.get_mut(at) {
                moved |= *slot != byte;
                *slot = byte;
                self.rt_seen |= 1u64 << at;
            }
        }
        if !moved && seen == self.rt_seen {
            return false;
        }
        let complete = self.radiotext();
        publish(&mut self.rt_text, complete)
    }

    /// A message is complete once every character up to its terminator — or all 64, when it
    /// carries none — has been received. Until then nothing is published.
    fn radiotext(&self) -> Option<String> {
        let end = (0..RT_LEN)
            .find(|&i| self.rt_seen & (1u64 << i) != 0 && self.rt.get(i) == Some(&RT_TERMINATOR))
            .unwrap_or(RT_LEN);
        let needed = if end >= RT_LEN {
            u64::MAX
        } else {
            (1u64 << end) - 1
        };
        (self.rt_seen & needed == needed).then(|| text(&self.rt[..end]))
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

    fn update(&self, groups: u64, block_errors: u64) -> RdsUpdate {
        RdsUpdate {
            pi: self.pi.map(|pi| format!("{pi:04X}")),
            ps: self.ps_text.clone(),
            radiotext: self.rt_text.clone(),
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
            block_errors,
        }
    }
}

/// Publish `value` into an optional field; true when it moved.
fn set<T: PartialEq>(slot: &mut Option<T>, value: T) -> bool {
    let moved = slot.as_ref() != Some(&value);
    if moved {
        *slot = Some(value);
    }
    moved
}

/// A completed text never reverts to "unknown": the next message being assembled must not
/// blank the display of the last one that finished.
fn publish(slot: &mut Option<String>, value: Option<String>) -> bool {
    match value {
        Some(text) if slot.as_deref() != Some(text.as_str()) => {
            *slot = Some(text);
            true
        }
        _ => false,
    }
}

/// EN 50067 Annex E code table G0. Only its ASCII-identical range is mapped; the national and
/// symbol code points above 0x7E become `?` rather than silently vanishing.
fn text(raw: &[u8]) -> String {
    raw.iter()
        .map(|&c| {
            if (0x20..=0x7E).contains(&c) {
                char::from(c)
            } else {
                '?'
            }
        })
        .collect::<String>()
        .trim_end()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use sdrmm_dsp::rds_encode_block;

    use super::*;
    use crate::testgen::rds::{Station as TxStation, composite, groups as tx_groups};

    const RATE: f64 = 240_000.0;
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

    /// Run a composite through the whole receiver in deliberately ragged blocks.
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
            // A chance syndrome hit costs one rejected block before the clock is dropped
            // again; anything beyond that is a broken hunt.
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
        // Toggle the RadioText A/B flag on the second run: its characters must replace the
        // first message's, not merge into them.
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
        // Wreck four consecutive groups early enough that the PS name and the RadioText only
        // finish assembling after the damage.
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
        // 3.5 s is 39 groups; the front end costs a fraction of a second to converge and the
        // block clock one group to find, but nothing after that may be dropped.
        assert!(
            decoder.frames.groups >= 34,
            "only {} groups",
            decoder.frames.groups
        );
        assert_eq!(decoder.frames.block_errors, 0);
    }

    #[test]
    fn reset_forgets_the_station() {
        let mut decoder = RdsDecoder::new(RATE);
        let events = run(&mut decoder, &composite(&station(), 3.0, None, RATE));
        assert!(!events.is_empty());
        decoder.reset();
        assert_eq!(decoder.frames.groups, 0);
        assert_eq!(decoder.frames.station.ps_text, None);
    }
}
