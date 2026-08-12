//! RTTY decoder (PLAN §13 P2): two-tone FSK into the Baudot/ITA2 alphabet.
//!
//! The waveform is the catalog's plain-CPFSK entry (`cell_params`): the reference modulator
//! in `testgen` transmits it through the library's own `CpmMod`, and the matched filter below
//! comes from the shared pulse library. The receive side deliberately keeps its per-sample
//! start/stop framing instead of riding `cpm/`'s demodulator, for two measured reasons. RTTY
//! is *character-asynchronous* — a sender may pause between characters for any time at all,
//! and the standard 1.5-bit stop element shifts the bit lattice half a period per character —
//! so timing re-anchors on every start-bit edge and one clean edge decodes the very next
//! character with no acquisition run-in; the engine's one continuous-clock timing stack
//! (`SymbolSync`, MODEM-PLAN §3.2) cannot re-phase per character. And the traffic is
//! mark-biased (idle is *continuous* mark), which statically biases `CpmDemod`'s
//! data-mean-learning centre estimate — the failure NAVTEX measured on its milder 4-of-7
//! bias is documented at `navtex::cpm_params`.

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, FmDemod, RealDecimator, design_lowpass};
#[cfg(any(test, feature = "test-signals"))]
use sdrmm_modem::cpm::{CpmParams, Mapping};
use sdrmm_modem::pulse::{self, Norm};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, RttyParams, RttyText,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const CHANNEL_TAPS: usize = 257;

/// Post-detection filter length is one symbol, which at 45.45 baud is 176 taps; the cap keeps
/// a hypothetical very slow setting from turning the filter into a per-sample cost blowup.
const MAX_POST_TAPS: usize = 511;

/// Every bit of a frame — start, data and stop — must reach this fraction of full deviation,
/// or the frame is dropped. A real transmission sits at ±1 on all seven; band-limited noise
/// reaches that level only by chance, so requiring it seven times over is what keeps an idle
/// band from printing the garbage a sign-only slicer would.
const MIN_LEVEL: f32 = 0.5;

/// Flush after this many characters even without a line ending: a teleprinter line is 69
/// characters, so a sender that never sends CR still produces one event per line's worth.
const FLUSH_CHARS: usize = 72;

/// Flush a partial line after this many bit periods without a character — the carrier dropped
/// or the operator stopped, and held text must not wait for the next transmission.
const IDLE_FLUSH_BITS: f64 = 24.0;

/// Positions sampled per character: the start bit, five data bits, and the first stop bit.
const FRAME_BITS: usize = 7;
const STOP_INDEX: usize = FRAME_BITS - 1;

/// ITA2 (Baudot) letters row indexed by the 5-bit code. `'\0'` marks a code that is not text:
/// NUL, and the two shift codes, which are handled before the lookup.
pub(crate) const LETTERS: [char; 32] = [
    '\0', 'E', '\n', 'A', ' ', 'S', 'I', 'U', '\r', 'D', 'R', 'J', 'N', 'F', 'C', 'K', 'T', 'Z',
    'L', 'W', 'H', 'Y', 'P', 'Q', 'O', 'B', 'G', '\0', 'M', 'X', 'V', '\0',
];

/// Figures row in the US teleprinter variant of ITA2 — `$ ' ! #` sit where the international
/// table has WRU/BELL/undefined. Code 0x05 is BELL, a signal rather than text, so it is
/// dropped like the other non-text codes.
pub(crate) const FIGURES: [char; 32] = [
    '\0', '3', '\n', '-', ' ', '\0', '8', '7', '\r', '$', '4', '\'', ',', '!', ':', '(', '5', '"',
    ')', '2', '#', '6', '0', '1', '9', '?', '&', '\0', '.', '/', ';', '\0',
];

pub(crate) const FIGS_CODE: u8 = 0x1B;
pub(crate) const LTRS_CODE: u8 = 0x1F;
pub(crate) const SPACE_CODE: u8 = 0x04;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "rtty".to_owned(),
    name: "RTTY".to_owned(),
    bandwidth_hz: 1_000.0,
    input_rate_hz: 8_000.0,
    has_audio: false,
    decoder_kind: Some("rtty".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct RttyChannel {
    demod: FmDemod,
    post: RealDecimator,
    /// `-1.0` when `invert` swaps mark and space (equivalent to reversing the sideband).
    polarity: f32,
    decoder: Decoder,
    demod_buf: Vec<f32>,
    filtered: Vec<f32>,
}

fn params(settings: &ChannelSettings) -> Result<&RttyParams, ChannelError> {
    match &settings.params {
        ChannelParams::Rtty(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "rtty channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_params(p: &RttyParams) -> Result<(), ChannelError> {
    let rate = DESCRIPTOR.input_rate_hz;
    if !(p.baud.is_finite() && p.baud > 0.0 && p.baud < rate / 4.0) {
        return Err(ChannelError::InvalidSettings(format!(
            "rtty baud must be in (0, {}), got {}",
            rate / 4.0,
            p.baud
        )));
    }
    if !(p.shift_hz.is_finite() && p.shift_hz > 0.0 && p.shift_hz < rate / 4.0) {
        return Err(ChannelError::InvalidSettings(format!(
            "rtty shift must be in (0, {}) Hz, got {}",
            rate / 4.0,
            p.shift_hz
        )));
    }
    Ok(())
}

/// Occupied RF band relative to the channel offset, in Hz.
pub(crate) fn occupied_band(p: &RttyParams) -> (f64, f64) {
    let half = p.shift_hz / 2.0 + 2.0 * p.baud;
    (-half, half)
}

pub(crate) fn channel_filter(p: &RttyParams) -> Result<ChannelFilter, ChannelError> {
    check_params(p)?;
    let (_, half) = occupied_band(p);
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, half / DESCRIPTOR.input_rate_hz),
        1,
    )))
}

/// One bit of integrate-and-dump — NRZ keying's own matched filter, from the shared pulse
/// library (MODEM-PLAN §3.1), sized from the current baud so 45.45 and 75 get the same shape.
fn post_filter(p: &RttyParams) -> RealDecimator {
    let sps = (DESCRIPTOR.input_rate_hz / p.baud).min(MAX_POST_TAPS as f64);
    RealDecimator::new(&pulse::rect(sps, Norm::Area), 1)
}

/// The RTTY waveform as `cpm/` entry data (MODEM-PLAN §3.3), stated at the half-bit *cell*
/// rate: 1.5 stop bits has no whole-bit representation, and at two cells per bit every
/// element of the start/stop frame is a whole number of symbols. Mark — the upper tone — is
/// index 1, so a cell's symbol index is its keyed level. Only the reference modulator rides
/// this entry (the module docs say why the receiver cannot); building the test signals from
/// it keeps the transmitted waveform and the receiver's numbers from drifting apart
/// (MODEM-PLAN §1.2).
#[cfg(any(test, feature = "test-signals"))]
pub(crate) fn cell_params(baud: f64, shift_hz: f64, rate: f64) -> CpmParams {
    let cell_baud = 2.0 * baud;
    let sps = rate / cell_baud;
    CpmParams::from_deviation(
        Mapping::new(vec![-1.0, 1.0]),
        shift_hz / 2.0,
        cell_baud,
        pulse::rect(sps, Norm::Area),
        sps,
    )
}

impl ChannelRx for RttyChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        check_params(p)?;
        Ok(Self {
            demod: FmDemod::new(ctx.input_rate, p.shift_hz / 2.0),
            post: post_filter(p),
            polarity: polarity(p),
            decoder: Decoder::new(ctx.input_rate, p),
            demod_buf: Vec::new(),
            filtered: Vec::new(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        check_params(p)?;
        let rate = DESCRIPTOR.input_rate_hz;
        self.demod = FmDemod::new(rate, p.shift_hz / 2.0);
        self.post = post_filter(p);
        self.polarity = polarity(p);
        // Text decoded under the old settings stays queued; the idle timer flushes it.
        self.decoder.configure(rate, p);
        Ok(())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        // Mark and space are symmetric about DC once the host has centred the channel, so one
        // discriminator scaled to ±shift/2 reads the pair directly as ±1 — no correlator pair
        // whose window would have to be re-tuned for every baud/shift combination, and the
        // same code holds from 45.45/170 to 75/850.
        self.demod.process(iq, &mut self.demod_buf);
        for s in &mut self.demod_buf {
            // Full deviation is the whole signal, so hard-limiting there costs a correctly
            // tuned transmission nothing while bounding the ±rate/(2·shift) excursions
            // carrier-free noise produces — that is what lets the filter below average noise
            // down to a level the slicer's gate can reject.
            *s = if s.is_finite() {
                (*s * self.polarity).clamp(-1.0, 1.0)
            } else {
                0.0
            };
        }
        self.post.process(&self.demod_buf, &mut self.filtered);
        for &level in &self.filtered {
            self.decoder.feed(level, out);
        }
    }
}

fn polarity(p: &RttyParams) -> f32 {
    if p.invert { -1.0 } else { 1.0 }
}

/// Character frame in progress, timed from the falling edge that opened its start bit.
#[derive(Clone, Copy, Default)]
struct Frame {
    since_edge: usize,
    next: usize,
    bits: u8,
}

/// Start/stop framing and the ITA2 shift state. Resynchronises on every start bit, so a
/// character lost to noise costs one character rather than the rest of the transmission.
struct Decoder {
    /// Sample offset from the start-bit edge at which each frame bit is sliced.
    slice_at: [usize; FRAME_BITS],
    idle_flush: usize,
    unshift_on_space: bool,
    positive: bool,
    frame: Option<Frame>,
    figs: bool,
    text: String,
    newline: bool,
    since_char: usize,
}

impl Decoder {
    fn new(rate: f64, p: &RttyParams) -> Self {
        let mut decoder = Self {
            slice_at: [0; FRAME_BITS],
            idle_flush: 0,
            unshift_on_space: true,
            positive: true,
            frame: None,
            figs: false,
            text: String::new(),
            newline: false,
            since_char: 0,
        };
        decoder.configure(rate, p);
        decoder
    }

    fn configure(&mut self, rate: f64, p: &RttyParams) {
        let sps = rate / p.baud;
        for (i, at) in self.slice_at.iter_mut().enumerate() {
            *at = ((i as f64 + 0.5) * sps).round() as usize;
        }
        self.idle_flush = (IDLE_FLUSH_BITS * sps) as usize;
        self.unshift_on_space = p.unshift_on_space;
        self.frame = None;
    }

    fn feed(&mut self, level: f32, out: &mut ChannelOutputs) {
        self.since_char = self.since_char.saturating_add(1);
        if self.since_char == self.idle_flush {
            self.flush(out);
        }
        if let Some(mut frame) = self.frame {
            frame.since_edge += 1;
            self.frame = if frame.since_edge == self.slice_at[frame.next] {
                self.slice(frame, level, out)
            } else {
                Some(frame)
            };
        } else if self.positive && level < 0.0 {
            self.frame = Some(Frame::default());
        }
        self.positive = level >= 0.0;
    }

    /// Slice one frame bit; `None` ends the frame, either because it completed or because it
    /// failed the start/stop test and the edge that opened it was noise.
    fn slice(&mut self, mut frame: Frame, level: f32, out: &mut ChannelOutputs) -> Option<Frame> {
        match frame.next {
            0 => {
                if level > -MIN_LEVEL {
                    return None;
                }
            }
            STOP_INDEX => {
                if level > MIN_LEVEL {
                    self.emit(frame.bits, out);
                }
                return None;
            }
            i => {
                if level.abs() < MIN_LEVEL {
                    return None;
                }
                if level > 0.0 {
                    frame.bits |= 1 << (i - 1);
                }
            }
        }
        frame.next += 1;
        Some(frame)
    }

    fn emit(&mut self, code: u8, out: &mut ChannelOutputs) {
        self.since_char = 0;
        match code {
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
        // `code` comes from five sliced bits, so it always indexes inside the 32-entry rows.
        let ch = if self.figs {
            FIGURES[code as usize]
        } else {
            LETTERS[code as usize]
        };
        if code == SPACE_CODE && self.unshift_on_space {
            self.figs = false;
        }
        match ch {
            '\0' => {}
            // A line ends with CR, CR LF or CR CR LF depending on the sender; all three are one
            // line break to a reader.
            '\r' | '\n' => {
                if !self.newline {
                    self.text.push('\n');
                    self.newline = true;
                    self.flush(out);
                }
            }
            c => {
                self.newline = false;
                self.text.push(c);
                if self.text.len() >= FLUSH_CHARS {
                    self.flush(out);
                }
            }
        }
    }

    fn flush(&mut self, out: &mut ChannelOutputs) {
        if self.text.is_empty() {
            return;
        }
        out.events.push(DecoderEvent::Rtty(RttyText {
            text: self.text.clone(),
        }));
        self.text.clear();
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{NfmParams, RttyStopBits};

    use super::*;
    use crate::{
        testgen::{
            self,
            rtty::{encode_codes, modulate, transmission},
        },
        testutil::{complex_noise, settings},
    };

    const RATE: f64 = 8_000.0;
    const BLOCKS: [usize; 7] = [997, 1, 4_096, 65, 2_048, 7, 1_024];

    fn channel(p: RttyParams) -> RttyChannel {
        RttyChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Rtty(p)),
        )
        .unwrap()
    }

    /// Enough dead air after a burst for the idle timer to flush the last partial line.
    fn tail(baud: f64) -> Vec<Complex<f32>> {
        testgen::silence((40.0 * RATE / baud) as usize)
    }

    fn decode_blocks(chan: &mut RttyChannel, iq: &[Complex<f32>], lens: &[usize]) -> Vec<String> {
        let mut out = ChannelOutputs::default();
        let mut texts = Vec::new();
        let mut pos = 0;
        for len in lens.iter().cycle() {
            if pos >= iq.len() {
                break;
            }
            let end = (pos + len).min(iq.len());
            out.reset();
            chan.process(&iq[pos..end], &mut out);
            assert!(out.audio_pcm.is_empty(), "rtty must not produce audio");
            for ev in &out.events {
                match ev {
                    DecoderEvent::Rtty(t) => texts.push(t.text.clone()),
                    other => panic!("unexpected event {other:?}"),
                }
            }
            pos = end;
        }
        texts
    }

    fn decode(p: RttyParams, iq: &[Complex<f32>]) -> String {
        decode_blocks(&mut channel(p), iq, &BLOCKS).concat()
    }

    /// One transmission of `text` with the trailing dead air a decoder needs to flush it.
    fn burst(text: &str, p: &RttyParams) -> Vec<Complex<f32>> {
        let mut iq = transmission(text, p.baud, p.shift_hz, p.stop_bits.periods(), RATE);
        iq.extend_from_slice(&tail(p.baud));
        iq
    }

    #[test]
    fn ita2_rows_match_the_standard_alphabet() {
        // The reference modulator shares these tables, so every round-trip test would still
        // pass with a typo in them. This is the only thing standing between the decoder and a
        // wrong alphabet, so it transcribes the whole chart — ITA2 with the US teleprinter
        // figures row, the variant the table documents — rather than spot-checking it. `\0`
        // marks a code with no printable meaning in that shift (NULL, BELL, FIGS, LTRS).
        #[rustfmt::skip]
        const STANDARD: [(u8, char, char); 32] = [
            (0x00, '\0', '\0'), (0x01, 'E',  '3'),  (0x02, '\n', '\n'), (0x03, 'A',  '-'),
            (0x04, ' ',  ' '),  (0x05, 'S',  '\0'), (0x06, 'I',  '8'),  (0x07, 'U',  '7'),
            (0x08, '\r', '\r'), (0x09, 'D',  '$'),  (0x0A, 'R',  '4'),  (0x0B, 'J',  '\''),
            (0x0C, 'N',  ','),  (0x0D, 'F',  '!'),  (0x0E, 'C',  ':'),  (0x0F, 'K',  '('),
            (0x10, 'T',  '5'),  (0x11, 'Z',  '"'),  (0x12, 'L',  ')'),  (0x13, 'W',  '2'),
            (0x14, 'H',  '#'),  (0x15, 'Y',  '6'),  (0x16, 'P',  '0'),  (0x17, 'Q',  '1'),
            (0x18, 'O',  '9'),  (0x19, 'B',  '?'),  (0x1A, 'G',  '&'),  (0x1B, '\0', '\0'),
            (0x1C, 'M',  '.'),  (0x1D, 'X',  '/'),  (0x1E, 'V',  ';'),  (0x1F, '\0', '\0'),
        ];
        for (code, letter, figure) in STANDARD {
            let index = code as usize;
            assert_eq!(LETTERS[index], letter, "letters row at {code:#04x}");
            assert_eq!(FIGURES[index], figure, "figures row at {code:#04x}");
        }
        assert_eq!(FIGS_CODE, 0x1B);
        assert_eq!(LTRS_CODE, 0x1F);
        assert_eq!(SPACE_CODE, 0x04);
    }

    #[test]
    fn decodes_an_amateur_call_at_45_baud_170_shift() {
        let p = RttyParams::default();
        assert_eq!(p.baud, 45.45);
        assert_eq!(p.shift_hz, 170.0);
        assert_eq!(p.stop_bits, RttyStopBits::OneAndHalf);
        let texts = decode_blocks(
            &mut channel(p.clone()),
            &burst("CQ CQ DE DL1ABC K", &p),
            &BLOCKS,
        );
        assert_eq!(texts, vec!["CQ CQ DE DL1ABC K".to_owned()]);
    }

    #[test]
    fn decodes_figures_through_the_shift_codes() {
        let p = RttyParams::default();
        assert_eq!(decode(p.clone(), &burst("599 001", &p)), "599 001");
    }

    #[test]
    fn decodes_commercial_baud_and_shift_combinations() {
        for (baud, shift_hz, stop_bits) in [
            (50.0, 450.0, RttyStopBits::One),
            (75.0, 850.0, RttyStopBits::Two),
            (45.45, 850.0, RttyStopBits::OneAndHalf),
        ] {
            let p = RttyParams {
                baud,
                shift_hz,
                stop_bits,
                ..RttyParams::default()
            };
            let got = decode(p.clone(), &burst("RYRY DE SDR 123", &p));
            assert_eq!(got, "RYRY DE SDR 123", "{baud} baud / {shift_hz} Hz");
        }
    }

    #[test]
    fn crlf_collapses_to_one_line_break_and_ends_the_event() {
        let p = RttyParams::default();
        let texts = decode_blocks(&mut channel(p.clone()), &burst("AB\r\r\nCD", &p), &BLOCKS);
        assert_eq!(texts, vec!["AB\n".to_owned(), "CD".to_owned()]);
    }

    #[test]
    fn inverted_transmission_needs_the_invert_flag() {
        let p = RttyParams::default();
        let normal = burst("TEST DE RTTY", &p);
        let inverted: Vec<Complex<f32>> = normal.iter().map(Complex::conj).collect();

        let flagged = RttyParams {
            invert: true,
            ..p.clone()
        };
        assert_eq!(decode(flagged, &inverted), "TEST DE RTTY");
        assert_ne!(decode(p, &inverted), "TEST DE RTTY");
    }

    #[test]
    fn unshift_on_space_recovers_a_stream_that_lost_its_letters_shift() {
        // FIGS 1 7 SPACE A B C, with the LTRS that should precede "ABC" missing.
        let codes = [FIGS_CODE, 0x17, 0x07, SPACE_CODE, 0x03, 0x19, 0x0E];
        let p = RttyParams::default();
        let mut iq = modulate(
            &encode_codes(&codes, p.stop_bits.periods()),
            p.baud,
            p.shift_hz,
            RATE,
        );
        iq.extend_from_slice(&tail(p.baud));

        assert_eq!(decode(p.clone(), &iq), "17 ABC");
        let stay = RttyParams {
            unshift_on_space: false,
            ..p
        };
        assert_eq!(decode(stay, &iq), "17 -?:");
    }

    #[test]
    fn pure_noise_decodes_to_nothing() {
        let p = RttyParams::default();
        for seed in [0x1234_5678, 0xdead_beef, 0x0f0f_0f0f] {
            let noise = complex_noise(seed, 0.05, 120_000);
            assert_eq!(decode(p.clone(), &noise), "", "seed {seed:#x} raw");
            // Band-limited noise is the harder case: it varies slowly enough to look like
            // keying, so only the level gate keeps it out.
            let noise = selected(&p, &noise);
            assert_eq!(decode(p.clone(), &noise), "", "seed {seed:#x} filtered");
        }
    }

    #[test]
    fn noise_around_a_transmission_adds_no_characters() {
        let p = RttyParams::default();
        let mut iq = complex_noise(0x5eed_1234, 0.02, 20_000);
        iq.extend_from_slice(&burst("DE DL1ABC", &p));
        iq.extend_from_slice(&complex_noise(0xfeed_face, 0.02, 20_000));
        assert_eq!(decode(p, &iq), "DE DL1ABC");
    }

    /// The engine always runs [`crate::channel_filter`] ahead of the demod, and a narrow-shift
    /// mode lives or dies on that selectivity — so a noise test that skips it would be
    /// measuring the wrong receiver.
    fn selected(p: &RttyParams, iq: &[Complex<f32>]) -> Vec<Complex<f32>> {
        let mut filtered = Vec::new();
        channel_filter(p).unwrap().process(iq, &mut filtered);
        filtered
    }

    #[test]
    fn decodes_through_additive_noise() {
        let p = RttyParams::default();
        let mut iq = burst("CQ DE DL1ABC", &p);
        testgen::add_noise(&mut iq, 0xabad_1dea, 0.5);
        assert_eq!(decode(p.clone(), &selected(&p, &iq)), "CQ DE DL1ABC");
    }

    #[test]
    fn ragged_block_splits_decode_identically() {
        let p = RttyParams::default();
        let iq = burst("THE QUICK BROWN FOX 1234567890", &p);
        let whole = decode_blocks(&mut channel(p.clone()), &iq, &[iq.len()]);
        let ragged = decode_blocks(&mut channel(p.clone()), &iq, &BLOCKS);
        let single = decode_blocks(&mut channel(p), &iq, &[1]);
        assert_eq!(whole, vec!["THE QUICK BROWN FOX 1234567890".to_owned()]);
        assert_eq!(ragged, whole);
        assert_eq!(single, whole);
    }

    #[test]
    fn apply_switches_baud_and_shift_in_place() {
        let mut chan = channel(RttyParams::default());
        let p = RttyParams {
            baud: 75.0,
            shift_hz: 850.0,
            stop_bits: RttyStopBits::Two,
            ..RttyParams::default()
        };
        chan.apply(settings(ChannelParams::Rtty(p.clone())))
            .unwrap();
        let texts = decode_blocks(&mut chan, &burst("QRV 75 BAUD", &p), &BLOCKS);
        assert_eq!(texts, vec!["QRV 75 BAUD".to_owned()]);
    }

    #[test]
    fn out_of_range_params_are_rejected() {
        for p in [
            RttyParams {
                baud: 0.0,
                ..RttyParams::default()
            },
            RttyParams {
                baud: f64::NAN,
                ..RttyParams::default()
            },
            RttyParams {
                baud: 2_000.0,
                ..RttyParams::default()
            },
            RttyParams {
                shift_hz: 0.0,
                ..RttyParams::default()
            },
            RttyParams {
                shift_hz: 4_000.0,
                ..RttyParams::default()
            },
        ] {
            let built = RttyChannel::new(
                ChannelCtx { input_rate: RATE },
                settings(ChannelParams::Rtty(p.clone())),
            );
            assert!(
                matches!(built, Err(ChannelError::InvalidSettings(_))),
                "{p:?} must be rejected"
            );
            assert!(matches!(
                channel_filter(&p),
                Err(ChannelError::InvalidSettings(_))
            ));
        }
    }

    #[test]
    fn mismatched_params_variant_is_rejected() {
        let mut chan = channel(RttyParams::default());
        let err = chan.apply(settings(ChannelParams::Nfm(NfmParams::default())));
        assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
        let built = RttyChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Nfm(NfmParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }

    #[test]
    fn wrong_input_rate_is_rejected() {
        let built = RttyChannel::new(
            ChannelCtx {
                input_rate: 48_000.0,
            },
            settings(ChannelParams::Rtty(RttyParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }
}
