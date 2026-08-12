//! NAVTEX / SITOR-B decoder (PLAN §13 P2): 100 baud FSK at a 170 Hz shift carrying the CCIR
//! 476 seven-unit code with mode-B time diversity (ITU-R M.540, M.625).
//!
//! Three layers sit on the discriminator. The *alphabet* is a constant-ratio code — exactly
//! four of seven bits are mark — so a corrupted character is detectable without a checksum.
//! The *diversity* sends every character twice, five character periods apart, which is what
//! turns detection into correction. The *framing* is `ZCZC B1B2B3B4 … NNNN`, and only text
//! between those markers is emitted: a broadcast station idles for minutes at a time, and a
//! decoder that logged everything it sliced would bury the messages in phasing signal.
//!
//! Everything about the physical layer is fixed by the standard, so the only setting is which
//! way round the sideband is.
//!
//! The waveform is the catalog's plain-CPFSK entry (`cpm_params`), and the reference
//! modulator in `testgen` transmits it through the library's own `CpmMod`; the receive side
//! still runs its discriminator + `BitSync` chain because the `cpm/` demodulator's centre
//! estimate cannot yet carry this alphabet — the measured defect is documented at
//! `cpm_params`.

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{BitSync, Decimator, FmDemod, RealDecimator, design_lowpass};
#[cfg(any(test, feature = "test-signals"))]
use sdrmm_modem::cpm::{CpmParams, Mapping};
use sdrmm_modem::pulse::{self, Norm};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, NavtexMessage, NavtexParams,
};

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate,
    rtty::{FIGS_CODE, FIGURES, LETTERS, LTRS_CODE},
};

/// Fixed by ITU-R M.540: NAVTEX is 100 baud, 170 Hz shift, on 518 / 490 / 4209.5 kHz.
const BAUD: f64 = 100.0;
const SHIFT_HZ: f64 = 170.0;

const CHANNEL_TAPS: usize = 257;

/// Bits per CCIR 476 character.
const CHAR_BITS: usize = 7;

/// The repeat (RX) copy of a character is transmitted five character periods after the first
/// (DX) copy — four other characters lie between them, the 280 ms of time diversity mode B is
/// built on (ITU-R M.625 §2).
const FEC_SLOTS: usize = 5;

/// Soft bit values kept: enough to reach from the character just completed back to its DX
/// copy, which is the whole point of the buffer.
const HISTORY: usize = (FEC_SLOTS + 1) * CHAR_BITS;
/// Where the character that just completed starts in [`Decoder::bits`].
const RX_BASE: usize = FEC_SLOTS * CHAR_BITS;
/// …and where its DX copy starts.
const DX_BASE: usize = 0;
/// The slot before the one that just completed — the other half of the phasing pattern.
const PREV_BASE: usize = RX_BASE - CHAR_BITS;

/// SITOR control signals: the three code points CCIR 476 has beyond ITA2's alphabet, plus the
/// signal-repetition character. None of them is text.
const ALPHA: u8 = 0x0F;
const BETA: u8 = 0x33;
const REP: u8 = 0x66;
const CHAR32: u8 = 0x6A;

/// CCIR 476 → ITA2. The 7-bit code is the index a receiver builds LSB-first from the wire; the
/// value is the ITA2 code carrying the same character, so the *alphabet* stays defined once,
/// in [`crate::rtty`], and NAVTEX inherits the shift tables RTTY already proves.
pub(crate) const CCIR476: [(u8, u8); 31] = [
    (0x17, 0x0B), // J
    (0x1B, 0x0D), // F
    (0x1D, 0x0E), // C
    (0x1E, 0x0F), // K
    (0x27, 0x13), // W
    (0x2B, 0x15), // Y
    (0x2D, 0x16), // P
    (0x2E, 0x17), // Q
    (0x35, 0x1A), // G
    (0x36, 0x1B), // FIGS
    (0x39, 0x1C), // M
    (0x3A, 0x1D), // X
    (0x3C, 0x1E), // V
    (0x47, 0x03), // A
    (0x4B, 0x05), // S
    (0x4D, 0x06), // I
    (0x4E, 0x07), // U
    (0x53, 0x09), // D
    (0x55, 0x0A), // R
    (0x56, 0x01), // E
    (0x59, 0x0C), // N
    (0x5A, 0x1F), // LTRS
    (0x5C, 0x04), // space
    (0x63, 0x11), // Z
    (0x65, 0x12), // L
    (0x69, 0x14), // H
    (0x6C, 0x02), // LF
    (0x71, 0x18), // O
    (0x72, 0x19), // B
    (0x74, 0x10), // T
    (0x78, 0x08), // CR
];

/// The ITA2 code a CCIR 476 character carries, or `None` for a control signal or an invalid
/// code.
pub(crate) fn ita2_for(code: u8) -> Option<u8> {
    CCIR476
        .iter()
        .find_map(|&(ccir, ita2)| (ccir == code).then_some(ita2))
}

/// The CCIR 476 code for an ITA2 code — the encoder's direction, so the reference modulator
/// reads the same chart the decoder does instead of carrying a second copy of it.
#[cfg(any(test, feature = "test-signals"))]
pub(crate) fn ccir_for(ita2: u8) -> Option<u8> {
    CCIR476
        .iter()
        .find_map(|&(ccir, code)| (code == ita2).then_some(ccir))
}

/// A character survives the wire only if exactly four of its seven bits are mark. This is the
/// whole error *detection* mechanism of the alphabet.
fn valid(code: u8) -> bool {
    code.count_ones() == 4
}

/// Consecutive undecodable characters tolerated before the phasing is presumed lost. Each
/// failure costs two and each success refunds one, so a burst of noise resyncs quickly while
/// an occasional hit on a good signal does not.
const MAX_ERROR_RUN: u32 = 6;

/// Bit periods without a decodable character before a message in progress is given up on and
/// emitted incomplete — the carrier dropped mid-broadcast.
const IDLE_FLUSH_BITS: usize = 300;

/// Characters accepted in one broadcast. A real NAVTEX message is a few hundred; ten thousand
/// means the framing was lost and `NNNN` will never arrive.
const MAX_BODY_CHARS: usize = 10_000;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "navtex".to_owned(),
    name: "NAVTEX".to_owned(),
    bandwidth_hz: 600.0,
    input_rate_hz: 8_000.0,
    has_audio: false,
    decoder_kind: Some("navtex".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct NavtexChannel {
    demod: FmDemod,
    post: RealDecimator,
    /// `-1.0` when `invert` swaps mark and space (equivalent to reversing the sideband).
    polarity: f32,
    sync: BitSync,
    decoder: Decoder,
    demod_buf: Vec<f32>,
    filtered: Vec<f32>,
}

fn params(settings: &ChannelSettings) -> Result<&NavtexParams, ChannelError> {
    match &settings.params {
        ChannelParams::Navtex(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "navtex channel got {} params",
            other.type_id()
        ))),
    }
}

/// Occupied RF band relative to the channel offset, in Hz.
pub(crate) fn occupied_band() -> (f64, f64) {
    let half = SHIFT_HZ / 2.0 + 2.0 * BAUD;
    (-half, half)
}

pub(crate) fn channel_filter() -> ChannelFilter {
    let (_, half) = occupied_band();
    ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, half / DESCRIPTOR.input_rate_hz),
        1,
    ))
}

/// The NAVTEX waveform as `cpm/` entry data (MODEM-PLAN §3.3): two-level CPFSK, NRZ (rect)
/// frequency pulse, ±85 Hz deviation at 100 baud. Mark — the upper tone — carries the 1 bit
/// (ITU-R M.476), so index 1 transmits +1 and a soft symbol's sign is the soft bit the SITOR
/// combiner wants. The reference modulator in `testgen` transmits this entry (MODEM-PLAN
/// §1.2).
///
/// The receiver does **not** ride `CpmDemod` yet, and the block is measured, not stylistic:
/// its centre estimate learns the *data mean*, and SITOR's constant-ratio alphabet is
/// mark-biased forever (4 of 7 bits, +1/7 in level units — no averaging length fixes a static
/// bias). The learned bias de-antipodalises transitions into the Gardner detector, whose
/// S-curve then grows a *stable* false equilibrium half a symbol off: on the band-limited
/// broadcast, initial timing phases in a ≈12 %-wide zone lock there persistently — 27–202 bit
/// errors on a clean signal, identical from 0.003 to 0.08 cycles/symbol of loop bandwidth,
/// zero errors at every phase with the same chain minus the centre estimate. Until the centre
/// is per-entry data (off, or with the alphabet's expected mean), `BitSync`'s zero-crossing
/// clock — which needs no centre — stays.
#[cfg(any(test, feature = "test-signals"))]
pub(crate) fn cpm_params(rate: f64) -> CpmParams {
    let sps = rate / BAUD;
    CpmParams::from_deviation(
        Mapping::new(vec![-1.0, 1.0]),
        SHIFT_HZ / 2.0,
        BAUD,
        pulse::rect(sps, Norm::Area),
        sps,
    )
}

/// One bit of integrate-and-dump — NRZ keying's own matched filter, from the shared pulse
/// library (MODEM-PLAN §3.1), the same shape RTTY builds at its own rate.
fn post_filter(rate: f64) -> RealDecimator {
    RealDecimator::new(&pulse::rect(rate / BAUD, Norm::Area), 1)
}

fn polarity(p: &NavtexParams) -> f32 {
    if p.invert { -1.0 } else { 1.0 }
}

impl ChannelRx for NavtexChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        Ok(Self {
            demod: FmDemod::new(ctx.input_rate, SHIFT_HZ / 2.0),
            post: post_filter(ctx.input_rate),
            polarity: polarity(p),
            sync: BitSync::new(ctx.input_rate, BAUD),
            decoder: Decoder::new(),
            demod_buf: Vec::new(),
            filtered: Vec::new(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let wanted = polarity(params(&settings)?);
        // Only a polarity flip matters, and it invalidates everything decoded so far — the
        // same bits read the other way up are a different message, not a continuation.
        if wanted != self.polarity {
            self.polarity = wanted;
            self.reset();
        }
        Ok(())
    }

    fn retuned(&mut self) {
        self.reset();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.demod.process(iq, &mut self.demod_buf);
        for s in &mut self.demod_buf {
            // Full deviation is the whole signal, so clamping there costs a correctly tuned
            // transmission nothing while bounding what carrier-free noise can contribute.
            *s = if s.is_finite() {
                (*s * self.polarity).clamp(-1.0, 1.0)
            } else {
                0.0
            };
        }
        self.post.process(&self.demod_buf, &mut self.filtered);
        for &level in &self.filtered {
            if let Some(soft) = self.sync.push_soft(level) {
                self.decoder.feed(soft, out);
            }
        }
    }
}

impl NavtexChannel {
    fn reset(&mut self) {
        self.sync.reset();
        self.decoder = Decoder::new();
    }
}

/// Where the receiver is in the SITOR-B slot structure.
enum State {
    /// Hunting the phasing signal: REP in a DX slot with ALPHA in the RX slot behind it.
    /// A random 7-bit window is a valid character often enough that one hit is not proof,
    /// so the pattern must repeat at exactly the slot spacing before the lock is taken.
    Phasing { since_hit: usize },
    Reading {
        since_slot: usize,
        /// Whether the slot about to complete is the repeat half of a pair — the one that
        /// carries both copies and is therefore where decoding happens.
        next_is_rx: bool,
    },
}

/// Slot spacing of the phasing signal: one DX and one RX slot.
const PHASING_PERIOD: usize = 2 * CHAR_BITS;

struct Decoder {
    bits: [f32; HISTORY],
    state: State,
    figs: bool,
    error_run: u32,
    since_char: usize,
    /// Text accumulated between `ZCZC` and `NNNN`.
    body: String,
    collecting: bool,
    newline: bool,
    /// Rolling window of the last four printable characters, for the framing markers.
    tail: [char; 4],
    errors_corrected: u32,
}

impl Decoder {
    fn new() -> Self {
        Self {
            bits: [0.0; HISTORY],
            state: State::Phasing {
                since_hit: usize::MAX,
            },
            figs: false,
            error_run: 0,
            since_char: 0,
            body: String::new(),
            collecting: false,
            newline: false,
            tail: ['\0'; 4],
            errors_corrected: 0,
        }
    }

    fn feed(&mut self, soft: f32, out: &mut ChannelOutputs) {
        self.bits.rotate_left(1);
        self.bits[HISTORY - 1] = soft;
        self.since_char = self.since_char.saturating_add(1);
        if self.since_char == IDLE_FLUSH_BITS {
            self.emit_message(false, out);
        }

        match &mut self.state {
            State::Phasing { since_hit } => {
                let hit =
                    code_at(&self.bits, RX_BASE) == ALPHA && code_at(&self.bits, PREV_BASE) == REP;
                *since_hit = since_hit.saturating_add(1);
                if hit {
                    if *since_hit == PHASING_PERIOD {
                        // The ALPHA that just completed sits in an RX slot, so the next slot
                        // to complete is a DX one.
                        self.state = State::Reading {
                            since_slot: 0,
                            next_is_rx: false,
                        };
                        self.error_run = 0;
                    } else {
                        *since_hit = 0;
                    }
                }
            }
            State::Reading {
                since_slot,
                next_is_rx,
            } => {
                *since_slot += 1;
                if *since_slot < CHAR_BITS {
                    return;
                }
                *since_slot = 0;
                let is_rx = *next_is_rx;
                *next_is_rx = !is_rx;
                if is_rx {
                    self.decode_slot(out);
                }
            }
        }
    }

    /// Decide one character from its two transmitted copies (ITU-R M.625 §2 mode B).
    fn decode_slot(&mut self, out: &mut ChannelOutputs) {
        let rx = code_at(&self.bits, RX_BASE);
        let dx = code_at(&self.bits, DX_BASE);
        let decided = if valid(rx) {
            Some((rx, false))
        } else if valid(dx) {
            Some((dx, true))
        } else {
            // Neither copy is a legal character, but the two disagree about *which* bits are
            // marginal; summing the soft values before slicing recovers a character that
            // neither copy carries on its own.
            let mut combined = 0u8;
            for i in 0..CHAR_BITS {
                if self.bits[DX_BASE + i] + self.bits[RX_BASE + i] > 0.0 {
                    combined |= 1 << i;
                }
            }
            valid(combined).then_some((combined, true))
        };

        match decided {
            Some((code, repaired)) => {
                if repaired {
                    self.errors_corrected = self.errors_corrected.saturating_add(1);
                }
                self.error_run = self.error_run.saturating_sub(1);
                self.emit_char(code, out);
            }
            None => {
                self.error_run += 2;
                if self.error_run > MAX_ERROR_RUN {
                    self.emit_message(false, out);
                    self.state = State::Phasing {
                        since_hit: usize::MAX,
                    };
                    self.figs = false;
                }
            }
        }
    }

    fn emit_char(&mut self, code: u8, out: &mut ChannelOutputs) {
        // Control signals carry no text — and the idle timer must keep running through them,
        // because ALPHA *is* what a station sends when it has stopped talking.
        if matches!(code, ALPHA | BETA | REP | CHAR32) {
            return;
        }
        let Some(ita2) = ita2_for(code) else {
            return;
        };
        self.since_char = 0;
        match ita2 {
            FIGS_CODE => {
                self.figs = true;
                return;
            }
            LTRS_CODE => {
                self.figs = false;
                return;
            }
            _ => {}
        }
        // `ita2` comes from the table above, which only holds five-bit codes.
        let ch = if self.figs {
            FIGURES[ita2 as usize]
        } else {
            LETTERS[ita2 as usize]
        };
        match ch {
            '\0' => {}
            // CR, CR LF and CR CR LF are all one line break to a reader.
            '\r' | '\n' => {
                if !self.newline && self.collecting {
                    self.body.push('\n');
                }
                self.newline = true;
                self.tail = ['\0'; 4];
            }
            c => {
                self.newline = false;
                self.tail.rotate_left(1);
                self.tail[3] = c;
                self.push_text(c, out);
            }
        }
    }

    fn push_text(&mut self, ch: char, out: &mut ChannelOutputs) {
        if self.tail == ['Z', 'C', 'Z', 'C'] {
            // A second ZCZC before NNNN means the first message never terminated.
            if self.collecting {
                self.body.truncate(self.body.len() - "ZCZ".len());
                self.emit_message(false, out);
            }
            self.body.clear();
            self.errors_corrected = 0;
            self.collecting = true;
            return;
        }
        if !self.collecting {
            return;
        }
        if self.tail == ['N', 'N', 'N', 'N'] {
            self.body.truncate(self.body.len() - "NNN".len());
            self.emit_message(true, out);
            return;
        }
        self.body.push(ch);
        if self.body.len() >= MAX_BODY_CHARS {
            self.emit_message(false, out);
        }
    }

    fn emit_message(&mut self, complete: bool, out: &mut ChannelOutputs) {
        let body = std::mem::take(&mut self.body);
        let errors_corrected = std::mem::take(&mut self.errors_corrected);
        self.collecting = false;
        self.tail = ['\0'; 4];
        if !complete && body.trim().is_empty() {
            return;
        }
        let (header, text) = split_header(&body);
        out.events.push(DecoderEvent::Navtex(NavtexMessage {
            station: header.map(|h| h.0),
            subject: header.map(|h| h.1),
            subject_name: header
                .and_then(|h| NavtexMessage::subject_name(h.1))
                .map(str::to_owned),
            serial: header.map(|h| h.2),
            text: text.trim().to_owned(),
            errors_corrected,
            complete,
        }));
    }
}

fn code_at(bits: &[f32; HISTORY], base: usize) -> u8 {
    let mut code = 0u8;
    for i in 0..CHAR_BITS {
        // The first bit received is the least significant (ITU-R M.476 transmission order).
        if bits[base + i] > 0.0 {
            code |= 1 << i;
        }
    }
    code
}

/// Split `B1B2B3B4` off the front of a broadcast body. Anything that is not two letters
/// followed by two digits is not a header — a message received from the middle keeps all of
/// its text rather than losing four characters to a guess.
fn split_header(body: &str) -> (Option<(char, char, u8)>, &str) {
    let trimmed = body.trim_start();
    let mut chars = trimmed.chars();
    let group: Vec<char> = chars.by_ref().take(4).collect();
    let [station, subject, tens, units] = group[..] else {
        return (None, trimmed);
    };
    if !station.is_ascii_alphabetic() || !subject.is_ascii_alphabetic() {
        return (None, trimmed);
    }
    let (Some(tens), Some(units)) = (tens.to_digit(10), units.to_digit(10)) else {
        return (None, trimmed);
    };
    let serial = (tens * 10 + units) as u8;
    (Some((station, subject, serial)), chars.as_str())
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::NfmParams;

    use super::*;
    use crate::{
        testgen::{self, navtex::transmission},
        testutil::{complex_noise, settings},
    };

    const RATE: f64 = 8_000.0;
    const BLOCKS: [usize; 7] = [997, 1, 4_096, 65, 2_048, 7, 1_024];

    fn channel(p: NavtexParams) -> NavtexChannel {
        NavtexChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Navtex(p)),
        )
        .unwrap()
    }

    fn decode_blocks(
        chan: &mut NavtexChannel,
        iq: &[Complex<f32>],
        lens: &[usize],
    ) -> Vec<NavtexMessage> {
        let mut out = ChannelOutputs::default();
        let mut messages = Vec::new();
        let mut pos = 0;
        for len in lens.iter().cycle() {
            if pos >= iq.len() {
                break;
            }
            let end = (pos + len).min(iq.len());
            out.reset();
            chan.process(&iq[pos..end], &mut out);
            assert!(out.audio_pcm.is_empty(), "navtex must not produce audio");
            for ev in &out.events {
                match ev {
                    DecoderEvent::Navtex(m) => messages.push(m.clone()),
                    other => panic!("unexpected event {other:?}"),
                }
            }
            pos = end;
        }
        messages
    }

    fn decode(iq: &[Complex<f32>]) -> Vec<NavtexMessage> {
        decode_blocks(&mut channel(NavtexParams::default()), iq, &BLOCKS)
    }

    const BROADCAST: &str = "ZCZC DA07\r\nGALE WARNING\r\nGERMAN BIGHT\r\nNNNN";

    /// The alphabet is the one thing a round trip through our own modulator cannot check —
    /// both sides would have to be wrong the same way. So the whole 35-code chart is
    /// transcribed here from ITU-R M.476 (the table fldigi and every SITOR decoder carries),
    /// character by character, against the ITA2 codes `rtty` already proves.
    #[test]
    fn ccir476_chart_matches_the_standard() {
        #[rustfmt::skip]
        const STANDARD: [(u8, char); 29] = [
            (0x17, 'J'), (0x1B, 'F'), (0x1D, 'C'), (0x1E, 'K'),
            (0x27, 'W'), (0x2B, 'Y'), (0x2D, 'P'), (0x2E, 'Q'),
            (0x35, 'G'), (0x39, 'M'), (0x3A, 'X'), (0x3C, 'V'),
            (0x47, 'A'), (0x4B, 'S'), (0x4D, 'I'), (0x4E, 'U'),
            (0x53, 'D'), (0x55, 'R'), (0x56, 'E'), (0x59, 'N'), (0x5C, ' '),
            (0x63, 'Z'), (0x65, 'L'), (0x69, 'H'), (0x6C, '\n'),
            (0x71, 'O'), (0x72, 'B'), (0x74, 'T'), (0x78, '\r'),
        ];
        for (code, letter) in STANDARD {
            let ita2 = ita2_for(code).unwrap_or_else(|| panic!("{code:#04x} is not in the table"));
            assert_eq!(LETTERS[ita2 as usize], letter, "letters row at {code:#04x}");
            assert_eq!(ccir_for(ita2), Some(code), "reverse lookup for {letter}");
        }
        assert_eq!(ita2_for(0x5A), Some(LTRS_CODE));
        assert_eq!(ita2_for(0x36), Some(FIGS_CODE));

        // Exactly the 35 four-of-seven code points exist: the 31 alphabet entries plus the
        // four SITOR control signals, and nothing else.
        let alphabet: Vec<u8> = (0..=127u8).filter(|&c| ita2_for(c).is_some()).collect();
        let control = [ALPHA, BETA, REP, CHAR32];
        let legal: Vec<u8> = (0..=127u8).filter(|&c| valid(c)).collect();
        assert_eq!(legal.len(), 35, "C(7,4) code points");
        assert_eq!(alphabet.len(), 31);
        for code in &legal {
            assert!(
                alphabet.contains(code) || control.contains(code),
                "{code:#04x} is a legal code with no meaning"
            );
        }
        for code in alphabet.iter().chain(&control) {
            assert!(valid(*code), "{code:#04x} is not four-of-seven");
        }
        // The ITA2 side must be a bijection onto every code but NUL, or a character is
        // unreachable.
        let mut ita2: Vec<u8> = CCIR476.iter().map(|&(_, code)| code).collect();
        ita2.sort_unstable();
        assert_eq!(ita2, (1..=31u8).collect::<Vec<_>>());
    }

    #[test]
    fn decodes_a_broadcast_with_its_header() {
        let messages = decode(&transmission(BROADCAST, RATE));
        assert_eq!(messages.len(), 1, "{messages:?}");
        let m = &messages[0];
        assert_eq!(m.station, Some('D'));
        assert_eq!(m.subject, Some('A'));
        assert_eq!(m.subject_name.as_deref(), Some("Navigational warning"));
        assert_eq!(m.serial, Some(7));
        assert_eq!(m.text, "GALE WARNING\nGERMAN BIGHT");
        assert_eq!(m.errors_corrected, 0);
        assert!(m.complete);
    }

    #[test]
    fn decodes_figures_through_the_shift_codes() {
        let messages = decode(&transmission(
            "ZCZC FA13\r\nWIND 25 KT AT 095 DEG\r\nNNNN",
            RATE,
        ));
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert_eq!(messages[0].text, "WIND 25 KT AT 095 DEG");
        assert_eq!(messages[0].serial, Some(13));
    }

    /// The point of mode B: a burst that destroys one copy of a run of characters must cost
    /// nothing, because the other copy is five characters away in time.
    #[test]
    fn time_diversity_survives_a_burst_that_wipes_one_copy() {
        let mut iq = transmission(BROADCAST, RATE);
        // 120 ms of dead carrier — roughly two characters, and never both copies of one.
        let burst = (0.12 * RATE) as usize;
        let start = iq.len() / 2;
        for s in &mut iq[start..start + burst] {
            *s = Complex::new(0.0, 0.0);
        }
        let messages = decode(&iq);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert_eq!(messages[0].text, "GALE WARNING\nGERMAN BIGHT");
        assert!(
            messages[0].errors_corrected > 0,
            "the burst should have been repaired from the repeat copy, not gone unnoticed"
        );
        assert!(messages[0].complete);
    }

    #[test]
    fn decodes_through_additive_noise() {
        let mut iq = transmission(BROADCAST, RATE);
        testgen::add_noise(&mut iq, 0xabad_1dea, 0.5);
        let mut filtered = Vec::new();
        channel_filter().process(&iq, &mut filtered);
        let messages = decode(&filtered);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert_eq!(messages[0].text, "GALE WARNING\nGERMAN BIGHT");
    }

    /// A transmitter whose bit clock runs 0.3 % slow drifts nearly three bit periods over
    /// this broadcast — routine for the zero-crossing clock, and the tracking bar any future
    /// front-end migration must meet.
    #[test]
    fn tracks_a_sample_clock_error_through_the_broadcast() {
        let iq = testgen::resample(&transmission(BROADCAST, RATE), RATE, RATE * 1.003);
        let messages = decode(&iq);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert_eq!(messages[0].text, "GALE WARNING\nGERMAN BIGHT");
        assert!(messages[0].complete);
    }

    /// Continuous-stream lock over ~9000 bits — NAVTEX never keys off, so the bit clock gets
    /// no dead time to hide in. Clean signal in, zero repairs demanded: one slipped bit
    /// anywhere would shred a character and surface as a repair or a truncated message.
    #[test]
    fn a_long_broadcast_holds_the_bit_clock_end_to_end() {
        let body: String = (0..12)
            .map(|i| format!("LINE {i:02} THE QUICK BROWN FOX JUMPS OVER 13 LAZY DOGS\r\n"))
            .collect();
        let messages = decode(&transmission(&format!("ZCZC DA07\r\n{body}NNNN"), RATE));
        assert_eq!(messages.len(), 1, "{messages:?}");
        let m = &messages[0];
        assert!(m.complete);
        assert_eq!(
            m.errors_corrected, 0,
            "repairs on a clean broadcast: the clock slipped"
        );
        assert_eq!(m.text, body.trim_end().replace("\r\n", "\n"));
    }

    #[test]
    fn pure_noise_decodes_to_nothing() {
        for seed in [0x1234_5678, 0xdead_beef, 0x0f0f_0f0f] {
            let noise = complex_noise(seed, 0.05, 240_000);
            assert_eq!(decode(&noise), Vec::new(), "seed {seed:#x} raw");
            let mut filtered = Vec::new();
            channel_filter().process(&noise, &mut filtered);
            assert_eq!(decode(&filtered), Vec::new(), "seed {seed:#x} filtered");
        }
    }

    /// Text outside `ZCZC … NNNN` is not a message. A station idles for minutes between
    /// broadcasts, and everything it sends in that time must stay out of the log.
    #[test]
    fn phasing_and_idle_produce_no_message() {
        let iq = testgen::navtex::phasing(6.0, RATE);
        assert_eq!(decode(&iq), Vec::new());
    }

    #[test]
    fn a_broadcast_cut_short_is_reported_incomplete() {
        let full = transmission(BROADCAST, RATE);
        // Drop the tail, which takes NNNN with it, then leave dead air for the idle flush.
        let mut iq = full[..full.len() * 3 / 4].to_vec();
        iq.extend(testgen::silence((6.0 * RATE) as usize));
        let messages = decode(&iq);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(!messages[0].complete);
        assert_eq!(messages[0].station, Some('D'));
        assert!(
            messages[0].text.starts_with("GALE WARNING"),
            "kept text: {:?}",
            messages[0].text
        );
    }

    #[test]
    fn inverted_transmission_needs_the_invert_flag() {
        let normal = transmission(BROADCAST, RATE);
        let inverted: Vec<Complex<f32>> = normal.iter().map(Complex::conj).collect();
        let flagged = NavtexParams { invert: true };
        assert_eq!(
            decode_blocks(&mut channel(flagged), &inverted, &BLOCKS)
                .first()
                .map(|m| m.text.clone()),
            Some("GALE WARNING\nGERMAN BIGHT".to_owned())
        );
        assert_eq!(decode(&inverted), Vec::new());
    }

    #[test]
    fn ragged_block_splits_decode_identically() {
        let iq = transmission(BROADCAST, RATE);
        let whole = decode_blocks(&mut channel(NavtexParams::default()), &iq, &[iq.len()]);
        let ragged = decode_blocks(&mut channel(NavtexParams::default()), &iq, &BLOCKS);
        let single = decode_blocks(&mut channel(NavtexParams::default()), &iq, &[1]);
        assert_eq!(whole.len(), 1);
        assert_eq!(ragged, whole);
        assert_eq!(single, whole);
    }

    #[test]
    fn retune_drops_the_message_in_flight() {
        let full = transmission(BROADCAST, RATE);
        let cut = full.len() * 3 / 4;
        let head = &full[..cut];
        let tail = &full[cut..];

        // Control: without the retune, the second half completes the message.
        let mut kept = channel(NavtexParams::default());
        assert!(decode_blocks(&mut kept, head, &BLOCKS).is_empty());
        let finished = decode_blocks(&mut kept, tail, &BLOCKS);
        assert_eq!(finished.len(), 1, "{finished:?}");
        assert_eq!(finished[0].text, "GALE WARNING\nGERMAN BIGHT");

        let mut retuned = channel(NavtexParams::default());
        assert!(decode_blocks(&mut retuned, head, &BLOCKS).is_empty());
        retuned.retuned();
        assert_eq!(
            decode_blocks(&mut retuned, tail, &BLOCKS),
            Vec::new(),
            "text from the station we left must not follow the channel"
        );
    }

    #[test]
    fn split_header_keeps_text_that_has_no_header() {
        assert_eq!(split_header("NO HEADER HERE").0, None);
        assert_eq!(split_header(" DA07\r\nBODY").0, Some(('D', 'A', 7)));
        assert_eq!(split_header("DA7\r\nBODY").0, None);
    }

    #[test]
    fn mismatched_params_variant_is_rejected() {
        let mut chan = channel(NavtexParams::default());
        let err = chan.apply(settings(ChannelParams::Nfm(NfmParams::default())));
        assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
        let built = NavtexChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Nfm(NfmParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }

    #[test]
    fn wrong_input_rate_is_rejected() {
        let built = NavtexChannel::new(
            ChannelCtx {
                input_rate: 48_000.0,
            },
            settings(ChannelParams::Navtex(NavtexParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }
}
