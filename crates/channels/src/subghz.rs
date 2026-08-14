//! Sub-GHz OOK/ASK/FSK capture and decode (, §13 P2) — the garage remotes, doorbells,
//! weather stations and TPMS sensors that live at 315 / 433.92 / 868 / 915 MHz.
//!
//! Two front ends produce the same thing: a keyed on/off stream. OOK takes the envelope
//! through an adaptive slicer; FSK discriminates and slices the tone pair, gated by the same
//! carrier detector. Above that everything is shared — edge timing, a base-period estimate,
//! and a classifier that recognises the pulse-width family every cheap remote speaks and
//! Manchester, and otherwise reports the raw edge timings so an unknown signal is still
//! something you can look at.
//!
//! The channel is deliberately wide (150 kHz by default). These transmitters are SAW-
//! controlled and routinely sit tens of kHz off their nominal frequency; a filter narrow
//! enough to look correct on paper would simply not hear them.
//!
//! No chip is named. An EV1527's 24 data bits and a PT2262's 12 tri-state symbols are the
//! *same* pulse train — so both readings ride along on a frame that fits, and the operator
//! decides which device they are holding.

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{
    Decimator, Envelope, FmDemod, KeyingSlicer, KeyingTiming, design_lowpass, flat_bandwidth_hz,
    one_pole_coeff,
};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, SubghzEncoding, SubghzFrame,
    SubghzModulation, SubghzParams,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

/// The passband is a third of the sample rate, so the transition band is enormous and a short
/// filter is enough — which matters at 250 kHz, where every tap is 250 k multiplies a second.
const CHANNEL_TAPS: usize = 63;

/// Envelope smoothing. One time constant, short against the shortest symbol the channel
/// accepts, just enough to take the fizz off the magnitude.
const ENVELOPE_TAU_S: f64 = 20e-6;

/// Nominal FSK deviation. Only sets the discriminator's output scale — the decision is made
/// against a tracked mean, so a sensor deviating more or less still keys correctly.
const FSK_DEVIATION_HZ: f64 = 25_000.0;

/// Time constant of the FSK slicing level. It has to ride through the longest run of one tone
/// inside a frame — the ~10 ms sync gap — without drifting into it, which rules out the fixed
/// `DcBlocker` pole (a ~1 ms corner at this rate) entirely.
const FSK_LEVEL_TAU_S: f64 = 50e-3;

/// Edges kept for one frame. A remote sends a few hundred; a signal that keeps keying past
/// this is a carrier, not a frame, and is dropped rather than truncated into a lie.
const MAX_EDGES: usize = 600;

/// Raw edge timings carried in the event. Enough to see the shape of an unknown signal,
/// bounded so a decoder log row never becomes a recording.
const MAX_REPORTED_TIMINGS: usize = 128;

/// Repeats arriving inside this window collapse into one event. Every one of these devices
/// transmits its payload several times per button press.
const COLLAPSE_S: f64 = 0.5;

/// Shortest run of bits a classified frame must carry. Below this a "decode" is a coincidence.
const MIN_BITS: usize = 8;

/// How far a duration may sit from a whole multiple of the base period and still count as
/// that multiple. Absolute in units of the base period, so a 3× symbol is held to a tighter
/// relative tolerance than a 1× one — which is what clock-derived timing actually looks like.
const QUANTIZE_TOLERANCE: f64 = 0.3;
/// Longest multiple of the base period a symbol may be. The frame-ending gap is far longer
/// than this, which is exactly why it ends the frame.
const MAX_MULTIPLE: u32 = 4;

/// A PT2262 tri-state symbol is two PWM bits, and a 24-bit frame is twelve of them.
const TRI_STATE_BITS: usize = 24;
/// EV1527 splits the same 24 bits into a 20-bit transmitter address and 4 button bits.
const EV1527_ADDRESS_BITS: usize = 20;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "subghz".to_owned(),
    name: "Sub-GHz".to_owned(),
    bandwidth_hz: 150_000.0,
    input_rate_hz: 250_000.0,
    has_audio: false,
    decoder_kind: Some("subghz".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct SubghzChannel {
    detector: Detector,
    timing: Timing,
    collapse: Collapse,
    rate: f64,
}

fn params(settings: &ChannelSettings) -> Result<&SubghzParams, ChannelError> {
    match &settings.params {
        ChannelParams::Subghz(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "subghz channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_params(p: &SubghzParams) -> Result<(), ChannelError> {
    let rate = DESCRIPTOR.input_rate_hz;
    // Pulse widths are measured off an envelope, so the whole band has to arrive at the same
    // level — the DDC's flat passband, not the wider band it merely keeps free of aliases.
    let widest = flat_bandwidth_hz(rate);
    if !(p.bandwidth_hz.is_finite() && p.bandwidth_hz > 0.0 && p.bandwidth_hz < widest) {
        return Err(ChannelError::InvalidSettings(format!(
            "subghz bandwidth must be in (0, {widest}) Hz, got {}",
            p.bandwidth_hz
        )));
    }
    if p.min_pulse_us == 0 || p.frame_gap_us <= p.min_pulse_us {
        return Err(ChannelError::InvalidSettings(format!(
            "subghz frame gap ({} µs) must exceed the minimum pulse ({} µs), and neither may be zero",
            p.frame_gap_us, p.min_pulse_us
        )));
    }
    Ok(())
}

/// Occupied RF band relative to the channel offset, in Hz.
pub(crate) fn occupied_band(p: &SubghzParams) -> (f64, f64) {
    let half = p.bandwidth_hz / 2.0;
    (-half, half)
}

pub(crate) fn channel_filter(p: &SubghzParams) -> Result<ChannelFilter, ChannelError> {
    check_params(p)?;
    let (_, half) = occupied_band(p);
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, half / DESCRIPTOR.input_rate_hz),
        1,
    )))
}

/// Turns IQ into a keyed on/off stream. Both arms share the carrier detector — the FSK one
/// needs it too, because a discriminator with no carrier to discriminate produces noise that
/// looks exactly like data.
///
/// The FSK arm deliberately does not ride a `cpm/` front end even though  §3.1
/// lists subghz under the CPFSK row: a remote's symbol rate is a per-frame *measurement* —
/// `base_period` reads it off the decoded edges after the fact — and frames are edge-timed at
/// sample resolution, so there is no symbol clock for `SymbolSync` to recover and no symbol
/// stream to slice. The transmit side does ride the library (`testgen::subghz::pwm_fsk` keys
/// `CpmMod`); the receive side needs a clockless sample-domain detector the engine does not
/// offer.
///
/// **Why the OOK arm did not migrate to the library's envelope tier in phase 4.** That tier
/// (`sdrmm_modem::linear::EnvelopeDemod`) is symbol-synchronous: it takes an oversampling, runs a
/// symbol clock, and emits one soft amplitude per symbol period. This decoder cannot give it one —
/// it *measures* the symbol rate per frame from the keyed edges, because a garage remote's clock is
/// whatever its RC oscillator happened to be that day and two units of the same model differ by
/// tens of percent. What the library entry and this front end share is therefore the alphabet and
/// the adaptive threshold, not the chain: the OOK row's committed bundle characterises magnitude
/// detection of a *clocked* keyed carrier, and the clockless edge-timed tier this needs is the
/// follow-on  §7 already lists for subghz.
enum Detector {
    Ook {
        envelope: Envelope,
        slicer: KeyingSlicer,
    },
    Fsk {
        envelope: Envelope,
        slicer: KeyingSlicer,
        demod: FmDemod,
        demod_buf: Vec<f32>,
        /// Slicing level, tracked only while a carrier is present — a gap between frames
        /// carries no information about where the tone pair sits.
        level: f32,
        level_coeff: f32,
    },
}

impl Detector {
    fn new(modulation: SubghzModulation, rate: f64) -> Self {
        let envelope = Envelope::new(rate, ENVELOPE_TAU_S, ENVELOPE_TAU_S);
        let slicer = KeyingSlicer::with_timing(rate, KeyingTiming::BURST);
        match modulation {
            SubghzModulation::Ook => Self::Ook { envelope, slicer },
            SubghzModulation::Fsk => Self::Fsk {
                envelope,
                slicer,
                demod: FmDemod::new(rate, FSK_DEVIATION_HZ),
                demod_buf: Vec::new(),
                level: 0.0,
                level_coeff: one_pole_coeff(rate, FSK_LEVEL_TAU_S),
            },
        }
    }

    /// Replace `keyed` with one on/off decision per input sample.
    fn process(&mut self, iq: &[Complex<f32>], keyed: &mut Vec<bool>) {
        keyed.clear();
        match self {
            Self::Ook { envelope, slicer } => {
                for s in iq {
                    keyed.push(slicer.push(envelope.push(s.norm())));
                }
            }
            Self::Fsk {
                envelope,
                slicer,
                demod,
                demod_buf,
                level,
                level_coeff,
            } => {
                demod.process(iq, demod_buf);
                // One non-finite sample would latch the tracker forever; healing per block
                // bounds the damage from a driver glitch to a block.
                if !level.is_finite() {
                    *level = 0.0;
                }
                for (s, &tone) in iq.iter().zip(demod_buf.iter()) {
                    let carrier = slicer.push(envelope.push(s.norm()));
                    if !tone.is_finite() {
                        keyed.push(false);
                        continue;
                    }
                    if carrier {
                        *level += *level_coeff * (tone - *level);
                    }
                    keyed.push(carrier && tone > *level);
                }
            }
        }
    }
}

/// Edge timing with a debounce: a state change only counts once it has held for the minimum
/// pulse width, so one sample of slicer chatter cannot split a symbol into three.
struct Timing {
    key: bool,
    /// Samples in the current stable run, including any excursion still being debounced.
    run: u32,
    /// Samples the opposite state has held for.
    candidate: u32,
    min_pulse: u32,
    frame_gap: u32,
    /// Durations in samples, pulse first. A leading gap is never recorded, so index 0 is
    /// always a pulse and the pair structure below can be trusted.
    edges: Vec<u32>,
    overflowed: bool,
}

impl Timing {
    fn new(p: &SubghzParams, rate: f64) -> Self {
        let samples = |us: u32| ((f64::from(us) * 1e-6 * rate).round() as u32).max(1);
        Self {
            key: false,
            run: 0,
            candidate: 0,
            min_pulse: samples(p.min_pulse_us),
            frame_gap: samples(p.frame_gap_us),
            edges: Vec::new(),
            overflowed: false,
        }
    }

    /// Feed one keying decision; returns the finished frame's edge durations when a gap long
    /// enough to separate frames has elapsed.
    fn push(&mut self, key: bool) -> Option<Vec<u32>> {
        self.run = self.run.saturating_add(1);
        if key == self.key {
            self.candidate = 0;
        } else {
            self.candidate += 1;
            if self.candidate >= self.min_pulse {
                let held = self.run - self.candidate;
                // A gap before the first pulse is the silence we were sitting in, not a symbol.
                if self.key || !self.edges.is_empty() {
                    if self.edges.len() >= MAX_EDGES {
                        self.overflowed = true;
                    } else {
                        self.edges.push(held);
                    }
                }
                self.key = key;
                self.run = self.candidate;
                self.candidate = 0;
            }
            return None;
        }

        if !self.key && self.run >= self.frame_gap && !self.edges.is_empty() {
            let frame = std::mem::take(&mut self.edges);
            let overflowed = std::mem::replace(&mut self.overflowed, false);
            return (!overflowed).then_some(frame);
        }
        None
    }

    fn reset(&mut self) {
        self.key = false;
        self.run = 0;
        self.candidate = 0;
        self.edges.clear();
        self.overflowed = false;
    }
}

/// The held frame awaiting its repeats.
struct Collapse {
    pending: Option<SubghzFrame>,
    since: u32,
    window: u32,
}

impl Collapse {
    fn new(rate: f64) -> Self {
        Self {
            pending: None,
            since: 0,
            window: (COLLAPSE_S * rate) as u32,
        }
    }

    /// A frame that carries more decoded bits supersedes one that carries fewer, but only
    /// while the held frame is still a single sighting: a receiver that tuned in mid-burst
    /// sees a fragment first, and that fragment must not become the log entry. Once a payload
    /// has repeated it is real, and a different one starts its own event.
    fn offer(&mut self, frame: SubghzFrame, out: &mut ChannelOutputs) {
        self.since = 0;
        match &mut self.pending {
            Some(held) if held.data == frame.data && held.encoding == frame.encoding => {
                held.repeats += 1;
            }
            Some(held) if rank(&frame) > rank(held) && held.repeats == 1 => {
                self.pending = Some(frame);
            }
            Some(held) if rank(&frame) <= rank(held) && held.repeats > 1 => {}
            Some(_) => {
                self.flush(out);
                self.pending = Some(frame);
            }
            None => self.pending = Some(frame),
        }
    }

    fn tick(&mut self, samples: u32, out: &mut ChannelOutputs) {
        if self.pending.is_none() {
            return;
        }
        self.since = self.since.saturating_add(samples);
        if self.since >= self.window {
            self.flush(out);
        }
    }

    fn flush(&mut self, out: &mut ChannelOutputs) {
        if let Some(frame) = self.pending.take() {
            out.events.push(DecoderEvent::Subghz(frame));
        }
        self.since = 0;
    }
}

fn rank(frame: &SubghzFrame) -> (u8, u32) {
    let named = u8::from(frame.encoding != SubghzEncoding::Raw);
    (named, frame.bits)
}

impl ChannelRx for SubghzChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        check_params(p)?;
        Ok(Self {
            detector: Detector::new(p.modulation, ctx.input_rate),
            timing: Timing::new(p, ctx.input_rate),
            collapse: Collapse::new(ctx.input_rate),
            rate: ctx.input_rate,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        check_params(p)?;
        // Every setting here changes what an edge *means*, so the frame in flight was measured
        // under rules that no longer apply.
        self.detector = Detector::new(p.modulation, self.rate);
        self.timing = Timing::new(p, self.rate);
        self.timing.reset();
        Ok(())
    }

    fn retuned(&mut self) {
        self.timing.reset();
        self.collapse.pending = None;
        self.collapse.since = 0;
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        let mut keyed = Vec::new();
        self.detector.process(iq, &mut keyed);
        for &key in &keyed {
            if let Some(edges) = self.timing.push(key) {
                let frame = classify(&edges, self.rate, modulation_of(&self.detector));
                self.collapse.offer(frame, out);
            }
        }
        self.collapse.tick(keyed.len() as u32, out);
    }
}

fn modulation_of(detector: &Detector) -> SubghzModulation {
    match detector {
        Detector::Ook { .. } => SubghzModulation::Ook,
        Detector::Fsk { .. } => SubghzModulation::Fsk,
    }
}

/// The base period a frame is built from: the mean of its shortest cluster, rather than the
/// single shortest edge, so one clipped edge does not scale every other symbol. The cluster
/// stops at 1.5× the shortest edge — Manchester's longest symbol is exactly 2×, and a wider
/// window would pull those into the average and scale the whole frame wrong.
fn base_period(edges: &[u32]) -> Option<u32> {
    let min = edges.iter().copied().min()?;
    let cluster: Vec<u32> = edges
        .iter()
        .copied()
        .filter(|&d| u64::from(d) * 2 < u64::from(min) * 3)
        .collect();
    let sum: u64 = cluster.iter().map(|&d| u64::from(d)).sum();
    u32::try_from(sum / cluster.len().max(1) as u64)
        .ok()
        .filter(|&d| d > 0)
}

/// How many base periods a duration is, or `None` when it is not a whole number of them.
fn multiple(duration: u32, base: u32) -> Option<u32> {
    let ratio = f64::from(duration) / f64::from(base);
    let n = ratio.round();
    ((1.0..=f64::from(MAX_MULTIPLE)).contains(&n) && (ratio - n).abs() <= QUANTIZE_TOLERANCE)
        .then_some(n as u32)
}

fn classify(edges: &[u32], rate: f64, modulation: SubghzModulation) -> SubghzFrame {
    let to_us = |samples: u32| (f64::from(samples) * 1e6 / rate).round() as u32;
    let timings_us: Vec<u32> = edges
        .iter()
        .take(MAX_REPORTED_TIMINGS)
        .map(|&d| to_us(d))
        .collect();
    let base = base_period(edges);
    let raw = SubghzFrame {
        modulation,
        encoding: SubghzEncoding::Raw,
        bits: 0,
        data: String::new(),
        address: None,
        button: None,
        tri_state: None,
        short_us: base.map_or(0, to_us),
        repeats: 1,
        timings_us,
    };
    let Some(base) = base else { return raw };
    let Some(steps) = edges
        .iter()
        .map(|&d| multiple(d, base))
        .collect::<Option<Vec<u32>>>()
    else {
        return raw;
    };

    let (encoding, bits) = match pwm_bits(&steps) {
        Some(bits) => (SubghzEncoding::Pwm, bits),
        None => match manchester_bits(&steps) {
            Some(bits) => (SubghzEncoding::Manchester, bits),
            None => return raw,
        },
    };

    let ev1527 = (bits.len() == TRI_STATE_BITS).then(|| {
        let address = bits[..EV1527_ADDRESS_BITS]
            .iter()
            .fold(0u32, |acc, &b| (acc << 1) | u32::from(b));
        let button = bits[EV1527_ADDRESS_BITS..]
            .iter()
            .fold(0u8, |acc, &b| (acc << 1) | u8::from(b));
        (address, button)
    });

    SubghzFrame {
        encoding,
        bits: bits.len() as u32,
        data: hex_of(&bits),
        address: ev1527.map(|(address, _)| address),
        button: ev1527.map(|(_, button)| button),
        tri_state: tri_state(&bits),
        ..raw
    }
}

/// Pulse-width coding: each bit is one pulse/gap pair whose halves differ in length, the
/// shorter half first for a 0. This is the PT2262 / EV1527 / Princeton family and most of what
/// a 433 MHz remote transmits.
fn pwm_bits(steps: &[u32]) -> Option<Vec<bool>> {
    // The trailing lone pulse is the sync bit whose long gap ended the frame.
    let pairs = steps.len() / 2;
    if pairs < MIN_BITS {
        return None;
    }
    let mut bits = Vec::with_capacity(pairs);
    let (cells, _) = steps[..pairs * 2].as_chunks::<2>();
    for &[pulse, gap] in cells {
        if pulse == gap {
            return None;
        }
        bits.push(pulse > gap);
    }
    Some(bits)
}

/// Manchester: every bit is a transition in the middle of its cell, so the keyed stream is a
/// run of one- and two-cell durations and each bit is one high/low cell pair.
fn manchester_bits(steps: &[u32]) -> Option<Vec<bool>> {
    if steps.iter().any(|&n| n > 2) {
        return None;
    }
    let mut cells = Vec::with_capacity(steps.len() * 2);
    let mut level = true;
    for &n in steps {
        for _ in 0..n {
            cells.push(level);
        }
        level = !level;
    }
    // A capture may start half a cell into the first bit, so both alignments are tried and the
    // one that decodes cleanly wins.
    (0..2)
        .filter_map(|offset| decode_cells(&cells[offset..]))
        .max_by_key(Vec::len)
        .filter(|bits| bits.len() >= MIN_BITS)
}

fn decode_cells(cells: &[bool]) -> Option<Vec<bool>> {
    let mut bits = Vec::with_capacity(cells.len() / 2);
    let (pairs, _) = cells.as_chunks::<2>();
    for &[first, second] in pairs {
        bits.push(sdrmm_dsp::manchester_decode(first, second)?);
    }
    Some(bits)
}

/// PT2262 reading of a 24-bit payload: twelve tri-state symbols, `00` = 0, `11` = 1, `01` = F
/// (floating). `10` is not a symbol the chip emits, so a frame containing one has no tri-state
/// reading at all rather than a partly-invented one.
fn tri_state(bits: &[bool]) -> Option<String> {
    if bits.len() != TRI_STATE_BITS {
        return None;
    }
    let (pairs, _) = bits.as_chunks::<2>();
    pairs
        .iter()
        .map(|pair| match pair {
            [false, false] => Some('0'),
            [true, true] => Some('1'),
            [false, true] => Some('F'),
            [true, false] => None,
        })
        .collect()
}

/// Payload as hex, most significant bit first, left-padded to a whole nibble.
fn hex_of(bits: &[bool]) -> String {
    let pad = (4 - bits.len() % 4) % 4;
    let mut out = String::with_capacity((bits.len() + pad) / 4);
    let mut nibble = 0u32;
    let mut filled = 0;
    for bit in std::iter::repeat_n(&false, pad).chain(bits) {
        nibble = (nibble << 1) | u32::from(*bit);
        filled += 1;
        if filled == 4 {
            out.push(
                char::from_digit(nibble, 16)
                    .unwrap_or('0')
                    .to_ascii_uppercase(),
            );
            nibble = 0;
            filled = 0;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::NfmParams;

    use super::*;
    use crate::{
        testgen::{
            self,
            subghz::{Pwm, manchester, pwm},
        },
        testutil::{complex_noise, settings},
    };

    const RATE: f64 = 250_000.0;
    const BLOCKS: [usize; 7] = [997, 1, 4_096, 65, 2_048, 7, 1_024];

    fn channel(p: SubghzParams) -> SubghzChannel {
        SubghzChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Subghz(p)),
        )
        .unwrap()
    }

    fn decode_blocks(
        chan: &mut SubghzChannel,
        iq: &[Complex<f32>],
        lens: &[usize],
    ) -> Vec<SubghzFrame> {
        let mut out = ChannelOutputs::default();
        let mut frames = Vec::new();
        let mut pos = 0;
        for len in lens.iter().cycle() {
            if pos >= iq.len() {
                break;
            }
            let end = (pos + len).min(iq.len());
            out.reset();
            chan.process(&iq[pos..end], &mut out);
            assert!(out.audio_pcm.is_empty(), "subghz must not produce audio");
            for ev in &out.events {
                match ev {
                    DecoderEvent::Subghz(f) => frames.push(f.clone()),
                    other => panic!("unexpected event {other:?}"),
                }
            }
            pos = end;
        }
        frames
    }

    fn decode(p: SubghzParams, iq: &[Complex<f32>]) -> Vec<SubghzFrame> {
        decode_blocks(&mut channel(p), iq, &BLOCKS)
    }

    /// A 24-bit EV1527 payload: 20 address bits and 4 button bits.
    const REMOTE: u32 = 0x0A_1B_23;

    fn ev1527() -> Pwm {
        Pwm {
            bits: (0..24).map(|i| REMOTE >> (23 - i) & 1 == 1).collect(),
            short_us: 320,
            long_multiple: 3,
            sync_gap_multiple: 31,
            repeats: 6,
        }
    }

    /// A press of a garage remote, end to end: the payload, both readings of it, and the fact
    /// that six transmissions became one log entry.
    #[test]
    fn decodes_an_ook_remote_and_collapses_its_repeats() {
        let frames = decode(SubghzParams::default(), &pwm(&ev1527(), RATE));
        assert_eq!(frames.len(), 1, "{frames:?}");
        let f = &frames[0];
        assert_eq!(f.modulation, SubghzModulation::Ook);
        assert_eq!(f.encoding, SubghzEncoding::Pwm);
        assert_eq!(f.bits, 24);
        assert_eq!(f.data, "0A1B23");
        assert_eq!(f.address, Some(REMOTE >> 4));
        assert_eq!(f.button, Some((REMOTE & 0xF) as u8));
        assert!(
            (300..=340).contains(&f.short_us),
            "base period {} µs",
            f.short_us
        );
        assert!(f.repeats >= 5, "collapsed {} of 6 repeats", f.repeats);
    }

    /// The tri-state reading exists only when every bit pair is a symbol a PT2262 can emit —
    /// a payload with a `10` pair is an EV1527 and must not be dressed up as twelve symbols.
    #[test]
    fn tri_state_is_offered_only_when_every_pair_is_a_symbol() {
        let all_symbols: Vec<bool> = [
            [false, false],
            [true, true],
            [false, true],
            [false, false],
            [true, true],
            [false, true],
            [false, false],
            [true, true],
            [false, true],
            [false, false],
            [true, true],
            [false, true],
        ]
        .concat();
        assert_eq!(tri_state(&all_symbols).as_deref(), Some("01F01F01F01F"));
        let mut with_ten = all_symbols.clone();
        with_ten[0] = true;
        assert_eq!(tri_state(&with_ten), None);
        assert_eq!(tri_state(&all_symbols[..20]), None, "wrong length");
    }

    #[test]
    fn decodes_a_manchester_sensor() {
        let bits: Vec<bool> = (0..32)
            .map(|i| (0xC3A5_96F0u32 >> (31 - i)) & 1 == 1)
            .collect();
        let frames = decode(SubghzParams::default(), &manchester(&bits, 250, 4, RATE));
        assert_eq!(frames.len(), 1, "{frames:?}");
        assert_eq!(frames[0].encoding, SubghzEncoding::Manchester);
        assert_eq!(frames[0].bits, 32);
        assert_eq!(frames[0].data, "C3A596F0");
    }

    #[test]
    fn decodes_an_fsk_remote() {
        let p = SubghzParams {
            modulation: SubghzModulation::Fsk,
            ..SubghzParams::default()
        };
        let frames = decode(p, &testgen::subghz::pwm_fsk(&ev1527(), 40_000.0, RATE));
        assert_eq!(frames.len(), 1, "{frames:?}");
        assert_eq!(frames[0].modulation, SubghzModulation::Fsk);
        assert_eq!(frames[0].data, "0A1B23");
    }

    /// A shape the classifier does not know still has to come back as something an operator
    /// can look at — that is the whole point of the raw capture.
    #[test]
    fn an_unrecognised_burst_is_reported_as_raw_timings() {
        let odd = testgen::subghz::keyed(&[900, 400, 300, 1_700, 250, 260, 1_100, 380, 700], RATE);
        let frames = decode(SubghzParams::default(), &odd);
        assert_eq!(frames.len(), 1, "{frames:?}");
        assert_eq!(frames[0].encoding, SubghzEncoding::Raw);
        assert_eq!(frames[0].bits, 0);
        assert!(frames[0].data.is_empty());
        assert!(
            frames[0].timings_us.len() >= 8,
            "timings {:?}",
            frames[0].timings_us
        );
    }

    /// A press captured from the middle: the first frame is a fragment. It must not become
    /// the log entry the operator sees.
    #[test]
    fn a_fragment_is_superseded_by_the_whole_frames_behind_it() {
        let remote = ev1527();
        let full = pwm(&remote, RATE);
        // Start one and a half frames into the burst, so the first thing the channel sees is
        // the back half of a transmission.
        let frame_us: u32 = testgen::subghz::pwm_timings(&remote).iter().sum();
        let cut = (0.05 * RATE) as usize + (1.5 * f64::from(frame_us) * 1e-6 * RATE) as usize;
        let frames = decode(SubghzParams::default(), &full[cut..]);
        assert_eq!(frames.len(), 1, "{frames:?}");
        assert_eq!(frames[0].data, "0A1B23");
        assert_eq!(frames[0].bits, 24);
    }

    #[test]
    fn decodes_through_additive_noise() {
        let mut iq = pwm(&ev1527(), RATE);
        testgen::add_noise(&mut iq, 0xabad_1dea, 0.1);
        let mut filtered = Vec::new();
        channel_filter(&SubghzParams::default())
            .unwrap()
            .process(&iq, &mut filtered);
        let frames = decode(SubghzParams::default(), &filtered);
        assert_eq!(frames.len(), 1, "{frames:?}");
        assert_eq!(frames[0].data, "0A1B23");
    }

    #[test]
    fn pure_noise_decodes_to_nothing() {
        for seed in [0x1234_5678, 0xdead_beef, 0x0f0f_0f0f] {
            let noise = complex_noise(seed, 0.05, 1_000_000);
            assert_eq!(
                decode(SubghzParams::default(), &noise),
                Vec::new(),
                "seed {seed:#x}"
            );
        }
    }

    #[test]
    fn ragged_block_splits_decode_identically() {
        let iq = pwm(&ev1527(), RATE);
        let whole = decode_blocks(&mut channel(SubghzParams::default()), &iq, &[iq.len()]);
        let ragged = decode_blocks(&mut channel(SubghzParams::default()), &iq, &BLOCKS);
        let single = decode_blocks(&mut channel(SubghzParams::default()), &iq, &[1]);
        assert_eq!(whole.len(), 1);
        assert_eq!(ragged, whole);
        assert_eq!(single, whole);
    }

    #[test]
    fn retune_drops_the_frame_being_held() {
        let iq = pwm(&ev1527(), RATE);
        let mut chan = channel(SubghzParams::default());
        // Everything but the trailing silence: the frame is decoded but still being held for
        // its repeats.
        let held = iq.len() - (0.4 * RATE) as usize;
        assert!(decode_blocks(&mut chan, &iq[..held], &BLOCKS).is_empty());
        chan.retuned();
        assert_eq!(
            decode_blocks(&mut chan, &iq[held..], &BLOCKS),
            Vec::new(),
            "a frame from the frequency we left must not be emitted here"
        );
    }

    #[test]
    fn base_period_ignores_one_clipped_edge() {
        // Nine edges at ~80 samples and one at 240; the estimate must be the cluster, not the
        // minimum, and not dragged by the long one.
        let edges = [80, 79, 81, 240, 80, 78, 82, 240, 80, 80];
        let base = base_period(&edges).unwrap();
        assert!((78..=82).contains(&base), "base {base}");
        assert_eq!(multiple(240, base), Some(3));
        assert_eq!(multiple(80, base), Some(1));
        assert_eq!(multiple(120, base), None, "1.5× is not a whole multiple");
    }

    #[test]
    fn hex_pads_to_whole_nibbles() {
        assert_eq!(hex_of(&[true; 4]), "F");
        assert_eq!(hex_of(&[true, false, true]), "5");
        assert_eq!(hex_of(&[]), "");
    }

    #[test]
    fn out_of_range_params_are_rejected() {
        for p in [
            SubghzParams {
                bandwidth_hz: 0.0,
                ..SubghzParams::default()
            },
            SubghzParams {
                bandwidth_hz: f64::NAN,
                ..SubghzParams::default()
            },
            SubghzParams {
                bandwidth_hz: 240_000.0,
                ..SubghzParams::default()
            },
            SubghzParams {
                min_pulse_us: 0,
                ..SubghzParams::default()
            },
            SubghzParams {
                min_pulse_us: 9_000,
                ..SubghzParams::default()
            },
        ] {
            assert!(
                matches!(channel_filter(&p), Err(ChannelError::InvalidSettings(_))),
                "{p:?} must be rejected"
            );
            assert!(matches!(
                SubghzChannel::new(
                    ChannelCtx { input_rate: RATE },
                    settings(ChannelParams::Subghz(p)),
                ),
                Err(ChannelError::InvalidSettings(_))
            ));
        }
    }

    #[test]
    fn mismatched_params_variant_is_rejected() {
        let mut chan = channel(SubghzParams::default());
        let err = chan.apply(settings(ChannelParams::Nfm(NfmParams::default())));
        assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
    }

    #[test]
    fn wrong_input_rate_is_rejected() {
        let built = SubghzChannel::new(
            ChannelCtx {
                input_rate: 48_000.0,
            },
            settings(ChannelParams::Subghz(SubghzParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }
}
