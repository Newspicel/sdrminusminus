//! POCSAG pager decoder (PLAN §13 P2): two-level FSK at 512/1200/2400 bit/s carrying
//! BCH(31,21) codewords (ITU-R M.584).
//!
//! Quadrature discriminator → slow slicing-level tracker → one integrate-and-dump filter and
//! bit clock per candidate bit rate. A candidate that finds the frame sync codeword takes the
//! lock and the others are reset; losing frame sync releases it, so the rate is re-detected on
//! the next transmission. The channel produces decoder events only — no audio.

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{
    BitSync, Decimator, FmDemod, SyncDetector, design_lowpass, hamming_distance, one_pole_coeff,
    pocsag_bch_decode,
};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, PocsagBaud, PocsagMessage,
    PocsagParams, PocsagPayload,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const CHANNEL_TAPS: usize = 129;

/// Nominal deviation of a POCSAG transmitter (ITU-R M.584 §2). Only sets the discriminator's
/// output scale — every decision downstream is on the sign, not the magnitude.
const NOMINAL_DEVIATION_HZ: f64 = 4_500.0;

/// Frame synchronisation codeword (ITU-R M.584 §2).
const FRAME_SYNC: u32 = 0x7CD2_15D8;
/// Idle codeword — fills unused slots and terminates a message.
const IDLE: u32 = 0x7A89_C197;
const CODEWORD_BITS: u32 = 32;
const BATCH_CODEWORDS: usize = 16;
/// Payload bits a message codeword carries (32 minus the flag, BCH check bits and parity).
const PAYLOAD_BITS: usize = 20;
const ALPHA_BITS: usize = 7;
const NUMERIC_BITS: usize = 4;

/// The 32-bit sync word survives a couple of channel errors; beyond that the batch boundary
/// would be a guess, and a wrong boundary shreds every codeword behind it.
const SYNC_TOLERANCE: u32 = 2;

/// Longest message accepted, in payload bits (128 codewords ≈ 365 alphanumeric characters).
/// A "message" longer than this is a decoder that has lost the thread, not a page; emitting a
/// truncated one would be worse than dropping it.
const MAX_PAYLOAD_BITS: usize = 128 * PAYLOAD_BITS;

/// Time constant of the slicing-level tracker. Long enough that no run of like bits at 512
/// bit/s pulls it into the data, short enough to absorb a receiver tuning error within a
/// preamble.
const LEVEL_TAU_S: f64 = 0.1;

/// BCD alphabet used when the function bits are 0 (ITU-R M.584 §2).
const NUMERIC_ALPHABET: [char; 16] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '*', 'U', ' ', '-', ')', '(',
];
/// Codes a transmitter pads the last alphanumeric codeword with; they end the message.
const ALPHA_PADDING: [u8; 3] = [0x00, 0x03, 0x04];

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "pocsag".to_owned(),
    name: "POCSAG".to_owned(),
    bandwidth_hz: 12_500.0,
    input_rate_hz: 48_000.0,
    has_audio: false,
    decoder_kind: Some("pocsag".to_owned()),
});

pub struct PocsagChannel {
    demod: FmDemod,
    demod_buf: Vec<f32>,
    /// Slicing level tracked out of the discriminator output.
    level: f32,
    level_coeff: f32,
    invert: bool,
    baud: PocsagBaud,
    candidates: Vec<Candidate>,
    /// Index into `candidates` of the rate currently holding frame sync.
    locked: Option<usize>,
}

fn params(settings: &ChannelSettings) -> Result<&PocsagParams, ChannelError> {
    match &settings.params {
        ChannelParams::Pocsag(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "pocsag channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_params(p: &PocsagParams) -> Result<(), ChannelError> {
    let rate = DESCRIPTOR.input_rate_hz;
    if p.bandwidth_hz.is_finite() && p.bandwidth_hz > 0.0 && p.bandwidth_hz < rate {
        Ok(())
    } else {
        Err(ChannelError::InvalidSettings(format!(
            "pocsag bandwidth must be in (0, {rate}) Hz, got {}",
            p.bandwidth_hz
        )))
    }
}

/// Occupied RF band relative to the channel offset, in Hz.
pub(crate) fn occupied_band(p: &PocsagParams) -> (f64, f64) {
    let half = p.bandwidth_hz / 2.0;
    (-half, half)
}

pub(crate) fn channel_filter(p: &PocsagParams) -> Result<ChannelFilter, ChannelError> {
    check_params(p)?;
    let (_, half) = occupied_band(p);
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, half / DESCRIPTOR.input_rate_hz),
        1,
    )))
}

fn candidates(rate: f64, baud: PocsagBaud) -> Vec<Candidate> {
    baud.rates()
        .iter()
        .map(|&b| Candidate::new(rate, b))
        .collect()
}

/// What feeding one bit did to a candidate's framing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Framing {
    Held,
    /// A frame sync codeword matched: this candidate is decoding batches.
    Acquired,
    /// The codeword after a batch was not a frame sync word — the transmission is over.
    Lost,
}

/// Where a candidate sits inside the batch structure (ITU-R M.584 §2: a batch is one frame
/// sync codeword followed by 16 codewords).
#[derive(Clone, Copy, Debug)]
enum Batching {
    /// Sliding search for the frame sync codeword.
    Hunt,
    Codeword {
        index: usize,
        bits: u32,
    },
    /// The 32 bits after a batch must be the next frame sync codeword.
    Resync {
        bits: u32,
    },
}

/// The page being assembled: an address codeword and the message codewords behind it.
#[derive(Clone, Copy, Debug)]
struct Pending {
    address: u32,
    function: u8,
    errors: u32,
    /// An uncorrectable codeword landed inside this message, so it may never be emitted.
    poisoned: bool,
}

/// Integrate-and-dump matched filter for NRZ keying: a boxcar exactly one bit long, so the
/// value at a bit centre is that bit's own energy and nothing else. `dsp` has no running-mean
/// primitive, and a FIR would cost one multiply per tap where this costs one add and one
/// subtract per sample — at 512 bit/s the window is 94 samples wide.
struct BitIntegrator {
    ring: Vec<f32>,
    pos: usize,
    sum: f64,
    since_rebuild: usize,
}

impl BitIntegrator {
    fn new(len: usize) -> Self {
        Self {
            ring: vec![0.0; len.max(1)],
            pos: 0,
            sum: 0.0,
            since_rebuild: 0,
        }
    }

    fn push(&mut self, sample: f32) -> f32 {
        let leaving = self.ring[self.pos];
        self.ring[self.pos] = sample;
        self.pos = (self.pos + 1) % self.ring.len();
        self.sum += f64::from(sample) - f64::from(leaving);
        self.since_rebuild += 1;
        if self.since_rebuild >= self.ring.len() {
            self.rebuild();
        }
        (self.sum / self.ring.len() as f64) as f32
    }

    /// A running sum accumulates rounding error without bound and would latch a non-finite
    /// sample forever. Recomputing straight from the ring once per window bounds both at O(1)
    /// amortized cost.
    fn rebuild(&mut self) {
        self.sum = self.ring.iter().copied().map(f64::from).sum();
        self.since_rebuild = 0;
    }

    fn reset(&mut self) {
        self.ring.fill(0.0);
        self.pos = 0;
        self.sum = 0.0;
        self.since_rebuild = 0;
    }
}

/// One candidate bit rate: its own matched filter, bit clock, sync correlator and framing.
struct Candidate {
    baud: u16,
    integrator: BitIntegrator,
    clock: BitSync,
    detector: SyncDetector,
    batching: Batching,
    pending: Option<Pending>,
    /// Message payload bits of `pending`, reused across messages.
    payload: Vec<bool>,
}

impl Candidate {
    fn new(rate: f64, baud: u16) -> Self {
        Self {
            baud,
            integrator: BitIntegrator::new((rate / f64::from(baud)).round() as usize),
            clock: BitSync::new(rate, f64::from(baud)),
            detector: SyncDetector::new(u64::from(FRAME_SYNC), CODEWORD_BITS, SYNC_TOLERANCE),
            batching: Batching::Hunt,
            pending: None,
            payload: Vec::new(),
        }
    }

    fn reset(&mut self) {
        self.integrator.reset();
        self.clock.reset();
        self.detector.reset();
        self.batching = Batching::Hunt;
        self.pending = None;
        self.payload.clear();
    }

    /// Feed one DC-corrected discriminator sample.
    fn push(&mut self, level: f32, out: &mut Vec<DecoderEvent>) -> Framing {
        let Some(high) = self.clock.push(self.integrator.push(level)) else {
            return Framing::Held;
        };
        // Mark — the higher of the two frequencies — carries a 0 bit (ITU-R M.584 §2).
        self.bit(!high, out)
    }

    fn bit(&mut self, bit: bool, out: &mut Vec<DecoderEvent>) -> Framing {
        let synced = self.detector.push(bit);
        match self.batching {
            Batching::Hunt => {
                if synced {
                    self.batching = Batching::Codeword { index: 0, bits: 0 };
                    return Framing::Acquired;
                }
            }
            Batching::Codeword { index, bits } => {
                let bits = bits + 1;
                if bits < CODEWORD_BITS {
                    self.batching = Batching::Codeword { index, bits };
                } else {
                    self.codeword(index, self.detector.register() as u32, out);
                    let index = index + 1;
                    self.batching = if index == BATCH_CODEWORDS {
                        Batching::Resync { bits: 0 }
                    } else {
                        Batching::Codeword { index, bits: 0 }
                    };
                }
            }
            Batching::Resync { bits } => {
                let bits = bits + 1;
                if bits < CODEWORD_BITS {
                    self.batching = Batching::Resync { bits };
                } else if hamming_distance(self.detector.register(), u64::from(FRAME_SYNC))
                    <= SYNC_TOLERANCE
                {
                    self.batching = Batching::Codeword { index: 0, bits: 0 };
                    return Framing::Acquired;
                } else {
                    self.flush(out);
                    self.batching = Batching::Hunt;
                    return Framing::Lost;
                }
            }
        }
        Framing::Held
    }

    fn codeword(&mut self, index: usize, word: u32, out: &mut Vec<DecoderEvent>) {
        let Some((word, errors)) = pocsag_bch_decode(word) else {
            // The damage could have been the address codeword that ends the message in
            // progress or a payload codeword inside it — either way what we hold is not the
            // message that was sent.
            if let Some(pending) = &mut self.pending {
                pending.poisoned = true;
            }
            return;
        };
        if word == IDLE {
            self.flush(out);
            return;
        }
        // Bit 31 is 0 for an address codeword, 1 for a message codeword (ITU-R M.584 §2).
        if word >> 31 == 0 {
            self.flush(out);
            self.payload.clear();
            self.pending = Some(Pending {
                // The address codeword carries the 18 high bits; the 3 low bits are the index
                // of the frame it arrived in.
                address: ((word >> 13) & 0x3_FFFF) << 3 | (index / 2) as u32,
                function: ((word >> 11) & 3) as u8,
                errors,
                poisoned: false,
            });
        } else if let Some(pending) = &mut self.pending {
            pending.errors += errors;
            if self.payload.len() >= MAX_PAYLOAD_BITS {
                pending.poisoned = true;
                return;
            }
            self.payload
                .extend((0..PAYLOAD_BITS).rev().map(|i| word >> (11 + i) & 1 == 1));
        }
    }

    fn flush(&mut self, out: &mut Vec<DecoderEvent>) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if pending.poisoned {
            return;
        }
        let (payload, text) = if self.payload.is_empty() {
            (PocsagPayload::Tone, String::new())
        } else if pending.function == 0 {
            (PocsagPayload::Numeric, numeric_text(&self.payload))
        } else {
            (PocsagPayload::Alpha, alpha_text(&self.payload))
        };
        out.push(DecoderEvent::Pocsag(PocsagMessage {
            address: pending.address,
            function: pending.function,
            baud: self.baud,
            payload,
            text,
            errors_corrected: pending.errors,
        }));
    }
}

/// Characters are packed least-significant-bit first into the codeword bit stream, for both
/// the 7-bit alphanumeric and the 4-bit BCD alphabets (ITU-R M.584 §2).
fn character(bits: &[bool]) -> u8 {
    bits.iter()
        .enumerate()
        .fold(0, |acc, (i, &bit)| acc | u8::from(bit) << i)
}

fn alpha_text(bits: &[bool]) -> String {
    bits.as_chunks::<ALPHA_BITS>()
        .0
        .iter()
        .map(|chunk| character(chunk))
        .take_while(|c| !ALPHA_PADDING.contains(c))
        .map(char::from)
        .collect()
}

fn numeric_text(bits: &[bool]) -> String {
    let mut text: String = bits
        .as_chunks::<NUMERIC_BITS>()
        .0
        .iter()
        // The mask is what keeps the index inside the alphabet; four bits already are.
        .map(|chunk| NUMERIC_ALPHABET[usize::from(character(chunk) & 0x0F)])
        .collect();
    // The last codeword is padded with the space code.
    text.truncate(text.trim_end_matches(' ').len());
    text
}

impl ChannelRx for PocsagChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        check_params(p)?;
        Ok(Self {
            demod: FmDemod::new(ctx.input_rate, NOMINAL_DEVIATION_HZ),
            demod_buf: Vec::new(),
            level: 0.0,
            level_coeff: one_pole_coeff(ctx.input_rate, LEVEL_TAU_S),
            invert: p.invert,
            baud: p.baud,
            candidates: candidates(ctx.input_rate, p.baud),
            locked: None,
        })
    }

    /// A new rate set restarts detection — a batch half-decoded at the old rate cannot be
    /// continued at a new one — but any other settings change leaves a transmission in
    /// progress alone.
    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        check_params(p)?;
        self.invert = p.invert;
        if p.baud != self.baud {
            self.baud = p.baud;
            self.candidates = candidates(DESCRIPTOR.input_rate_hz, p.baud);
            self.locked = None;
        }
        Ok(())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        let mut buf = std::mem::take(&mut self.demod_buf);
        self.demod.process(iq, &mut buf);
        // A single non-finite sample would latch the level tracker forever; healing per block
        // bounds the damage from a driver glitch to one block.
        if !self.level.is_finite() {
            self.level = 0.0;
        }
        for &sample in &buf {
            self.sample(sample, &mut out.events);
        }
        self.demod_buf = buf;
    }
}

impl PocsagChannel {
    fn sample(&mut self, sample: f32, out: &mut Vec<DecoderEvent>) {
        self.level += self.level_coeff * (sample - self.level);
        let sliced = sample - self.level;
        let sliced = if self.invert { -sliced } else { sliced };

        if let Some(index) = self.locked {
            let Some(candidate) = self.candidates.get_mut(index) else {
                return;
            };
            if candidate.push(sliced, out) == Framing::Lost {
                candidate.reset();
                self.locked = None;
            }
            return;
        }
        // A candidate that matched the sync word by chance loses it one batch later and hands
        // the lock back, so a false lock costs a batch rather than the transmission.
        let acquired = self
            .candidates
            .iter_mut()
            .position(|c| c.push(sliced, out) == Framing::Acquired);
        if let Some(index) = acquired {
            self.locked = Some(index);
            for (i, candidate) in self.candidates.iter_mut().enumerate() {
                if i != index {
                    candidate.reset();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::NfmParams;

    use super::*;
    use crate::{
        testgen::{
            add_noise,
            pocsag::{Page, codewords, keyed, transmission},
            silence,
        },
        testutil::settings,
    };

    const RATE: f64 = 48_000.0;
    const DEVIATION_HZ: f64 = 4_500.0;
    /// Addresses are 21 bits; their low 3 bits (7 and 0 here) put the two pages in different
    /// frames of a batch.
    const ALPHA_ADDRESS: u32 = 1_234_567;
    const NUMERIC_ADDRESS: u32 = 1_987_648;

    fn channel_with(params: PocsagParams) -> PocsagChannel {
        PocsagChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Pocsag(params)),
        )
        .unwrap()
    }

    fn channel() -> PocsagChannel {
        channel_with(PocsagParams::default())
    }

    fn pages() -> Vec<Page> {
        vec![
            Page {
                address: ALPHA_ADDRESS,
                function: 3,
                text: "CALL CONTROL 42".to_owned(),
                numeric: false,
            },
            Page {
                address: NUMERIC_ADDRESS,
                function: 0,
                text: "0123456789-U".to_owned(),
                numeric: true,
            },
        ]
    }

    /// A transmission with lead-in and lead-out silence, so every run also exercises the
    /// carrier-absent path.
    fn burst(pages: &[Page], baud: u16) -> Vec<Complex<f32>> {
        let mut iq = silence(4_000);
        iq.extend(transmission(pages, baud, DEVIATION_HZ, RATE));
        iq.extend(silence(4_000));
        iq
    }

    fn run(chan: &mut dyn ChannelRx, iq: &[Complex<f32>]) -> Vec<PocsagMessage> {
        run_chunked(chan, iq, &[iq.len().max(1)])
    }

    fn run_chunked(
        chan: &mut dyn ChannelRx,
        iq: &[Complex<f32>],
        chunks: &[usize],
    ) -> Vec<PocsagMessage> {
        let mut out = ChannelOutputs::default();
        let mut messages = Vec::new();
        let mut pos = 0;
        for len in chunks.iter().cycle() {
            if pos >= iq.len() {
                break;
            }
            let end = (pos + len).min(iq.len());
            out.reset();
            chan.process(&iq[pos..end], &mut out);
            assert!(out.audio_pcm.is_empty(), "pocsag must produce no audio");
            for event in out.events.drain(..) {
                match event {
                    DecoderEvent::Pocsag(m) => messages.push(m),
                    other => panic!("unexpected event {other:?}"),
                }
            }
            pos = end;
        }
        messages
    }

    fn assert_expected_pages(messages: &[PocsagMessage], baud: u16) {
        assert_eq!(messages.len(), 2, "{messages:?}");
        let alpha = &messages[0];
        assert_eq!(alpha.address, ALPHA_ADDRESS);
        assert_eq!(alpha.function, 3);
        assert_eq!(alpha.baud, baud);
        assert_eq!(alpha.payload, PocsagPayload::Alpha);
        assert_eq!(alpha.text, "CALL CONTROL 42");
        let numeric = &messages[1];
        assert_eq!(numeric.address, NUMERIC_ADDRESS);
        assert_eq!(numeric.function, 0);
        assert_eq!(numeric.baud, baud);
        assert_eq!(numeric.payload, PocsagPayload::Numeric);
        assert_eq!(numeric.text, "0123456789-U");
    }

    #[test]
    fn round_trips_two_pages_at_every_rate() {
        for baud in [512u16, 1_200, 2_400] {
            let mut chan = channel_with(PocsagParams {
                baud: match baud {
                    512 => PocsagBaud::B512,
                    1_200 => PocsagBaud::B1200,
                    _ => PocsagBaud::B2400,
                },
                ..PocsagParams::default()
            });
            let messages = run(&mut chan, &burst(&pages(), baud));
            assert_expected_pages(&messages, baud);
            assert!(
                messages.iter().all(|m| m.errors_corrected == 0),
                "a clean signal must need no correction: {messages:?}"
            );
        }
    }

    #[test]
    fn auto_detection_finds_every_rate() {
        for baud in [512u16, 1_200, 2_400] {
            let mut chan = channel();
            let messages = run(&mut chan, &burst(&pages(), baud));
            assert_expected_pages(&messages, baud);
        }
    }

    #[test]
    fn back_to_back_transmissions_at_different_rates_both_decode() {
        let mut chan = channel();
        let mut iq = burst(&pages(), 2_400);
        iq.extend(burst(&pages(), 512));
        let messages = run(&mut chan, &iq);
        assert_eq!(messages.len(), 4, "{messages:?}");
        assert_expected_pages(&messages[..2], 2_400);
        assert_expected_pages(&messages[2..], 512);
    }

    #[test]
    fn apply_switches_the_rate_set() {
        let mut chan = channel();
        chan.apply(settings(ChannelParams::Pocsag(PocsagParams {
            baud: PocsagBaud::B512,
            ..PocsagParams::default()
        })))
        .unwrap();
        assert_expected_pages(&run(&mut chan, &burst(&pages(), 512)), 512);
        assert!(
            run(&mut chan, &burst(&pages(), 2_400)).is_empty(),
            "a pinned rate must not decode another one"
        );

        chan.apply(settings(ChannelParams::Pocsag(PocsagParams {
            baud: PocsagBaud::B2400,
            ..PocsagParams::default()
        })))
        .unwrap();
        assert_expected_pages(&run(&mut chan, &burst(&pages(), 2_400)), 2_400);
    }

    #[test]
    fn a_tone_only_page_decodes_with_an_empty_payload() {
        let page = Page {
            address: 42,
            function: 2,
            text: String::new(),
            numeric: false,
        };
        let mut chan = channel();
        let messages = run(&mut chan, &burst(&[page], 1_200));
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert_eq!(messages[0].address, 42);
        assert_eq!(messages[0].function, 2);
        assert_eq!(messages[0].payload, PocsagPayload::Tone);
        assert!(messages[0].text.is_empty());
    }

    #[test]
    fn polarity_follows_the_invert_setting() {
        let inverted: Vec<Complex<f32>> =
            burst(&pages(), 1_200).iter().map(Complex::conj).collect();

        let mut chan = channel();
        assert!(
            run(&mut chan, &inverted).is_empty(),
            "an inverted transmission must not decode with the default polarity"
        );

        let mut chan = channel_with(PocsagParams {
            invert: true,
            ..PocsagParams::default()
        });
        assert_expected_pages(&run(&mut chan, &inverted), 1_200);

        // The setting is a swap, not a repair: it must break the upright transmission.
        let mut chan = channel_with(PocsagParams {
            invert: true,
            ..PocsagParams::default()
        });
        assert!(run(&mut chan, &burst(&pages(), 1_200)).is_empty());
    }

    /// Noise heavy enough to flip bits in the codewords but not to break the bit clock: the
    /// pages must still come out exactly, repaired by the BCH code.
    #[test]
    fn noise_that_flips_bits_still_decodes_through_the_bch_code() {
        for seed in [0x51d3_2ac1u32, 0x1234_5678, 0xdead_beef, 0x0f0f_0f0f] {
            let mut iq = burst(&pages(), 1_200);
            add_noise(&mut iq, seed, 1.2);
            let mut chan = channel();
            let messages = run(&mut chan, &iq);
            assert_expected_pages(&messages, 1_200);
            let corrected: u32 = messages.iter().map(|m| m.errors_corrected).sum();
            assert!(
                corrected > 0,
                "seed {seed:#x} left the BCH path unexercised"
            );
        }
    }

    #[test]
    fn an_uncorrectable_codeword_drops_only_its_own_message() {
        let mut damaged = codewords(&pages());
        let victim = damaged
            .iter()
            .position(|&w| w >> 31 == 1)
            .expect("the fixture carries message codewords");
        // BCH(31,21) plus the parity bit has distance 6: every weight-3 error is detected and
        // none of them is repairable.
        damaged[victim] ^= 0b0111_0000_0000;
        assert_eq!(
            pocsag_bch_decode(damaged[victim]),
            None,
            "the fixture must be uncorrectable"
        );

        let mut chan = channel();
        let messages = run(&mut chan, &burst_of_codewords(&damaged, 1_200));
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert_eq!(
            messages[0].address, NUMERIC_ADDRESS,
            "the undamaged page must survive"
        );
    }

    fn burst_of_codewords(words: &[u32], baud: u16) -> Vec<Complex<f32>> {
        let mut iq = silence(4_000);
        iq.extend(keyed(words, baud, DEVIATION_HZ, RATE));
        iq.extend(silence(4_000));
        iq
    }

    /// A stream of message codewords longer than any real page means framing has been lost;
    /// what is held is not a message and must not be emitted as one.
    #[test]
    fn an_overlong_message_is_dropped_rather_than_truncated() {
        let long = Page {
            address: ALPHA_ADDRESS,
            function: 3,
            text: "X".repeat(MAX_PAYLOAD_BITS / ALPHA_BITS + 1),
            numeric: false,
        };
        let mut chan = channel();
        assert!(run(&mut chan, &burst(&[long], 2_400)).is_empty());
    }

    #[test]
    fn ragged_blocks_match_one_shot_exactly() {
        let iq = burst(&pages(), 1_200);
        let mut whole = channel();
        let expected = run(&mut whole, &iq);

        let mut ragged = channel();
        let got = run_chunked(&mut ragged, &iq, &[997, 1, 4_096, 65, 2_048, 7, 1_024]);
        assert_eq!(got, expected);
    }

    #[test]
    fn mismatched_params_variant_is_rejected() {
        let mut chan = channel();
        let err = chan.apply(settings(ChannelParams::Nfm(NfmParams::default())));
        assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
        let built = PocsagChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Nfm(NfmParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }

    #[test]
    fn out_of_range_bandwidth_is_rejected() {
        for bad in [0.0, -1.0, 48_000.0, f64::NAN] {
            let built = PocsagChannel::new(
                ChannelCtx { input_rate: RATE },
                settings(ChannelParams::Pocsag(PocsagParams {
                    bandwidth_hz: bad,
                    ..PocsagParams::default()
                })),
            );
            assert!(
                matches!(built, Err(ChannelError::InvalidSettings(_))),
                "bandwidth {bad} must be rejected"
            );
        }
    }

    #[test]
    fn wrong_input_rate_is_rejected() {
        let built = PocsagChannel::new(
            ChannelCtx {
                input_rate: 240_000.0,
            },
            settings(ChannelParams::Pocsag(PocsagParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }
}
