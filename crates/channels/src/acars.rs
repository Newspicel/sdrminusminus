//! ACARS decoder (PLAN §13 P2): MSK at 2400 bit/s amplitude-modulated onto a VHF carrier,
//! carrying the character-oriented ARINC 618 block format.
//!
//! Chain: envelope detect the AM (the data rides on the carrier's amplitude, so the magnitude
//! *is* the audio) → shift the 1200/2400 Hz tone pair down to ±600 Hz and decimate → quadrature
//! discriminator → one-symbol matched filter → bit clock → byte framing.
//!
//! MSK at h = 0.5 means the bit is carried by the instantaneous frequency alone, so a
//! discriminator recovers it without a phase reference — and the sideband the receiver happens
//! to be on cannot break the decode, because a mirrored spectrum simply inverts every bit and
//! the sync byte is recognised in both polarities (the same trick `acarsdec` uses).
//!
//! Every block is checked twice over: odd parity on each character *and* the ARINC 618
//! CRC-16 across the whole block. Nothing is repaired — a block that fails either test is
//! dropped, because an ACARS message is free text and a plausible-looking wrong one is worse
//! than a missing one.

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{
    BitSync, DcBlocker, Decimator, FmDemod, Nco, RealDecimator, crc16_ccitt, design_lowpass,
};
use sdrmm_wire::{
    AcarsMessage, AcarsParams, ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const CHANNEL_TAPS: usize = 129;

/// ARINC 618 physical layer: MSK, 2400 bit/s, tones 1200 and 2400 Hz about an 1800 Hz centre.
const BAUD: f64 = 2_400.0;
const CENTRE_HZ: f64 = 1_800.0;
const DEVIATION_HZ: f64 = 600.0;

/// The tone pair is brought to baseband and decimated before the discriminator: ±600 Hz needs
/// nothing like 48 kHz, and every tap below runs at the reduced rate.
const DECIMATION: usize = 2;
/// Taps for the image-rejecting lowpass after the mixer. The wanted band ends at 600 Hz and
/// the mirror image starts at 3000 Hz, so the transition has 2.4 kHz to work in.
const BASEBAND_TAPS: usize = 127;
const BASEBAND_CUTOFF_HZ: f64 = 1_000.0;

const SYN: u8 = 0x16;
const SOH: u8 = 0x01;
const STX: u8 = 0x02;
/// End of text / end of block, as they appear on the wire — 0x03 and 0x17 with their odd
/// parity bit set, so matching on these validates the terminator's parity too.
const ETX: u8 = 0x83;
const ETB: u8 = 0x97;
/// ETB with its parity bit stripped, which is what the parsed block carries.
const ETB_DATA: u8 = ETB & 0x7F;
/// Acknowledgement field value meaning "not acknowledged".
const NAK: u8 = 0x15;

/// Shortest legal block: mode, seven address characters, ack, two label characters, block id
/// and the block-start character (ARINC 618 §4.3).
const MIN_BLOCK: usize = 13;
/// Longest block ARINC 618 permits, plus the terminator. Anything beyond this is a decoder
/// that lost the framing, not a message.
const MAX_BLOCK: usize = 240;

/// Byte offsets inside a block, counted from the character after SOH.
const ADDRESS: std::ops::Range<usize> = 1..8;
const ACK: usize = 8;
const LABEL: std::ops::Range<usize> = 9..11;
const BLOCK_ID: usize = 11;
const BLOCK_START: usize = 12;
/// A downlink block prefixes its text with a four-character sequence number and a
/// six-character flight number.
const SEQ_LEN: usize = 4;
const FLIGHT_LEN: usize = 6;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "acars".to_owned(),
    name: "ACARS".to_owned(),
    bandwidth_hz: 12_500.0,
    input_rate_hz: 48_000.0,
    has_audio: false,
    decoder_kind: Some("acars".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct AcarsChannel {
    envelope: Vec<f32>,
    dc: DcBlocker,
    mixer: Nco,
    mixed: Vec<Complex<f32>>,
    baseband: Decimator,
    filtered: Vec<Complex<f32>>,
    demod: FmDemod,
    demod_buf: Vec<f32>,
    matched: RealDecimator,
    sliced: Vec<f32>,
    sync: BitSync,
    framer: Framer,
}

fn params(settings: &ChannelSettings) -> Result<&AcarsParams, ChannelError> {
    match &settings.params {
        ChannelParams::Acars(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "acars channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_params(p: &AcarsParams) -> Result<(), ChannelError> {
    let rate = DESCRIPTOR.input_rate_hz;
    // The tone pair reaches 2400 Hz, so a filter narrower than that would remove the data.
    if p.bandwidth_hz.is_finite() && p.bandwidth_hz > 2.0 * BAUD && p.bandwidth_hz < rate {
        Ok(())
    } else {
        Err(ChannelError::InvalidSettings(format!(
            "acars bandwidth must be in ({}, {rate}) Hz, got {}",
            2.0 * BAUD,
            p.bandwidth_hz
        )))
    }
}

/// Occupied RF band relative to the channel offset, in Hz.
pub(crate) fn occupied_band(p: &AcarsParams) -> (f64, f64) {
    let half = p.bandwidth_hz / 2.0;
    (-half, half)
}

pub(crate) fn channel_filter(p: &AcarsParams) -> Result<ChannelFilter, ChannelError> {
    check_params(p)?;
    let (_, half) = occupied_band(p);
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, half / DESCRIPTOR.input_rate_hz),
        1,
    )))
}

impl ChannelRx for AcarsChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        check_params(params(&settings)?)?;
        let rate = ctx.input_rate;
        let baseband_rate = rate / DECIMATION as f64;
        let matched_taps = ((baseband_rate / BAUD).round() as usize).max(3) | 1;
        Ok(Self {
            envelope: Vec::new(),
            dc: DcBlocker::new(),
            mixer: Nco::new(-CENTRE_HZ as f32, rate as f32),
            mixed: Vec::new(),
            baseband: Decimator::new(
                &design_lowpass(BASEBAND_TAPS, BASEBAND_CUTOFF_HZ / rate),
                DECIMATION,
            ),
            filtered: Vec::new(),
            demod: FmDemod::new(baseband_rate, DEVIATION_HZ),
            demod_buf: Vec::new(),
            matched: RealDecimator::new(&design_lowpass(matched_taps, BAUD / baseband_rate), 1),
            sliced: Vec::new(),
            sync: BitSync::new(baseband_rate, BAUD),
            framer: Framer::new(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        // Bandwidth is the host's channel filter, not ours; there is nothing else to set.
        check_params(params(&settings)?)
    }

    fn retuned(&mut self) {
        self.framer = Framer::new();
        self.sync.reset();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        // The data is the carrier's amplitude, so the envelope is the audio a legacy receiver
        // would hand to a modem.
        self.envelope.clear();
        self.envelope.extend(iq.iter().map(|s| s.norm()));
        self.dc.process(&mut self.envelope);

        // Real audio → complex baseband about the 1800 Hz centre in one pass; the lowpass
        // below then discards the mirror image the real signal also produced.
        let mixer = &mut self.mixer;
        let mixed = &mut self.mixed;
        mixed.clear();
        mixed.extend(
            self.envelope
                .iter()
                .map(|&s| Complex::new(s, 0.0) * mixer.next_sample()),
        );
        self.baseband.process(mixed, &mut self.filtered);

        self.demod.process(&self.filtered, &mut self.demod_buf);
        for s in &mut self.demod_buf {
            *s = if s.is_finite() {
                s.clamp(-1.5, 1.5)
            } else {
                0.0
            };
        }
        self.matched.process(&self.demod_buf, &mut self.sliced);
        for &level in &self.sliced {
            if let Some(bit) = self.sync.push(level) {
                self.framer.push(bit, out);
            }
        }
    }
}

/// Where the byte machine is in an ARINC 618 block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Sliding, one bit at a time, for the first sync character.
    Hunt,
    /// Byte-aligned from here on; the field names the byte expected next.
    Sync2,
    Soh,
    Text,
    Crc,
}

struct Framer {
    /// Bits arrive least-significant first, so a new bit enters at the top.
    reg: u8,
    /// A mirrored spectrum inverts every bit; the sync character reveals it and every bit
    /// after is corrected on the way in.
    inverted: bool,
    state: State,
    bits_to_byte: u32,
    block: Vec<u8>,
    crc: [u8; 2],
    crc_len: usize,
}

impl Framer {
    fn new() -> Self {
        Self {
            reg: 0,
            inverted: false,
            state: State::Hunt,
            bits_to_byte: 1,
            block: Vec::new(),
            crc: [0; 2],
            crc_len: 0,
        }
    }

    fn restart(&mut self) {
        self.state = State::Hunt;
        self.bits_to_byte = 1;
        self.block.clear();
        self.crc_len = 0;
    }

    fn push(&mut self, bit: bool, out: &mut ChannelOutputs) {
        let bit = bit != self.inverted;
        self.reg = (self.reg >> 1) | (u8::from(bit) << 7);
        self.bits_to_byte -= 1;
        if self.bits_to_byte > 0 {
            return;
        }
        self.bits_to_byte = 8;
        let byte = self.reg;

        match self.state {
            State::Hunt | State::Sync2 => self.sync(byte),
            State::Soh => {
                if byte == SOH {
                    self.state = State::Text;
                    self.block.clear();
                } else {
                    self.restart();
                }
            }
            State::Text => {
                self.block.push(byte);
                if byte == ETX || byte == ETB {
                    self.state = State::Crc;
                    self.crc_len = 0;
                } else if self.block.len() > MAX_BLOCK {
                    self.restart();
                }
            }
            State::Crc => {
                self.crc[self.crc_len] = byte;
                self.crc_len += 1;
                if self.crc_len == 2 {
                    if let Some(message) = self.decode() {
                        out.events.push(DecoderEvent::Acars(message));
                    }
                    self.restart();
                }
            }
        }
    }

    /// Accept the sync character in either polarity: seeing its complement means the receiver
    /// is on the wrong sideband, which is a fact about the whole stream, not this byte.
    fn sync(&mut self, byte: u8) {
        if byte == !SYN {
            self.inverted = !self.inverted;
            self.state = match self.state {
                State::Hunt => State::Sync2,
                _ => self.state,
            };
            self.bits_to_byte = 8;
            return;
        }
        if byte != SYN {
            if self.state == State::Hunt {
                // Still hunting: shift one bit and try again rather than skipping a byte.
                self.bits_to_byte = 1;
            } else {
                self.restart();
            }
            return;
        }
        self.state = match self.state {
            State::Hunt => State::Sync2,
            _ => State::Soh,
        };
    }

    /// Validate the block and split it into ARINC 618 fields. `None` drops it.
    fn decode(&self) -> Option<AcarsMessage> {
        if self.block.len() < MIN_BLOCK {
            return None;
        }
        // Every character carries odd parity, and the block carries a CRC-16 over exactly the
        // bytes as received. Both must hold: parity catches the common single-bit hit, the CRC
        // catches everything parity is blind to.
        if self.block.iter().any(|b| b.count_ones() % 2 == 0) {
            return None;
        }
        let mut checked = self.block.clone();
        checked.extend_from_slice(&self.crc);
        if crc16_ccitt(&checked) != 0 {
            return None;
        }

        let text: Vec<u8> = self.block.iter().map(|b| b & 0x7F).collect();
        let block_id = char::from(text[BLOCK_ID]);
        let downlink = block_id.is_ascii_digit();
        let terminator = *text.last()?;

        // A block whose "start" character is already the terminator is an empty
        // acknowledgement and has no body at all.
        let mut body = text
            .get(BLOCK_START + 1..text.len() - 1)
            .unwrap_or_default();
        let (mut seq_no, mut flight) = (None, None);
        if text[BLOCK_START] == STX && downlink && body.len() >= SEQ_LEN + FLIGHT_LEN {
            seq_no = Some(field(&body[..SEQ_LEN]));
            flight = Some(field(&body[SEQ_LEN..SEQ_LEN + FLIGHT_LEN]));
            body = &body[SEQ_LEN + FLIGHT_LEN..];
        }

        Some(AcarsMessage {
            mode: char::from(text[0]),
            registration: field(&text[ADDRESS]).replace('.', ""),
            ack: (text[ACK] != NAK).then(|| char::from(text[ACK])),
            label: field(&text[LABEL]),
            block_id,
            downlink,
            seq_no,
            flight,
            text: if text[BLOCK_START] == STX {
                printable(body)
            } else {
                String::new()
            },
            more: terminator == ETB_DATA,
        })
    }
}

/// A fixed-width ARINC field as text, trimmed of the padding it is written with.
fn field(raw: &[u8]) -> String {
    printable(raw).trim().to_owned()
}

/// ACARS text is free-form and may carry control characters a JSON consumer cannot render.
/// Line endings collapse to one `\n`, tabs survive, everything else non-printable is dropped.
fn printable(raw: &[u8]) -> String {
    let mut out = String::with_capacity(raw.len());
    for &b in raw {
        match b {
            b'\r' | b'\n' => {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            b'\t' => out.push('\t'),
            0x20..=0x7E => out.push(char::from(b)),
            _ => {}
        }
    }
    out.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::NfmParams;

    use super::*;
    use crate::{
        testgen::{
            self,
            acars::{Block, transmission},
        },
        testutil::{complex_noise, settings},
    };

    const RATE: f64 = 48_000.0;
    const BLOCKS: [usize; 7] = [997, 1, 4_096, 65, 2_048, 7, 1_024];

    fn channel() -> AcarsChannel {
        AcarsChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Acars(AcarsParams::default())),
        )
        .unwrap()
    }

    fn decode_blocks(
        chan: &mut AcarsChannel,
        iq: &[Complex<f32>],
        lens: &[usize],
    ) -> Vec<AcarsMessage> {
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
            assert!(out.audio_pcm.is_empty(), "acars must not produce audio");
            for ev in &out.events {
                match ev {
                    DecoderEvent::Acars(m) => messages.push(m.clone()),
                    other => panic!("unexpected event {other:?}"),
                }
            }
            pos = end;
        }
        messages
    }

    fn decode(iq: &[Complex<f32>]) -> Vec<AcarsMessage> {
        decode_blocks(&mut channel(), iq, &BLOCKS)
    }

    fn downlink() -> Block<'static> {
        Block {
            mode: '2',
            registration: ".D-AIBC",
            ack: '\x15',
            label: "H1",
            block_id: '3',
            seq_no: Some("M01A"),
            flight: Some("LH0400"),
            text: "REPORT ENGINE 1 OK",
            more: false,
        }
    }

    fn uplink() -> Block<'static> {
        Block {
            mode: '2',
            registration: ".N123AB",
            ack: 'C',
            label: "5Z",
            block_id: 'K',
            seq_no: None,
            flight: None,
            text: "CLEARANCE DELIVERED",
            more: false,
        }
    }

    #[test]
    fn decodes_a_downlink_block() {
        let messages = decode(&transmission(&downlink(), RATE));
        assert_eq!(messages.len(), 1, "{messages:?}");
        let m = &messages[0];
        assert_eq!(m.mode, '2');
        assert_eq!(m.registration, "D-AIBC");
        assert_eq!(m.ack, None, "0x15 is a NAK, not an acknowledged block");
        assert_eq!(m.label, "H1");
        assert_eq!(m.block_id, '3');
        assert!(m.downlink);
        assert_eq!(m.seq_no.as_deref(), Some("M01A"));
        assert_eq!(m.flight.as_deref(), Some("LH0400"));
        assert_eq!(m.text, "REPORT ENGINE 1 OK");
        assert!(!m.more);
    }

    /// An uplink carries no sequence or flight number, and the same ten characters would be
    /// eaten out of its text by a decoder that did not check the block id.
    #[test]
    fn decodes_an_uplink_block_without_the_downlink_prefix() {
        let messages = decode(&transmission(&uplink(), RATE));
        assert_eq!(messages.len(), 1, "{messages:?}");
        let m = &messages[0];
        assert_eq!(m.registration, "N123AB");
        assert_eq!(m.ack, Some('C'));
        assert_eq!(m.block_id, 'K');
        assert!(!m.downlink);
        assert_eq!(m.seq_no, None);
        assert_eq!(m.flight, None);
        assert_eq!(m.text, "CLEARANCE DELIVERED");
    }

    #[test]
    fn reports_a_block_that_ends_with_etb_as_continued() {
        let block = Block {
            more: true,
            ..downlink()
        };
        let messages = decode(&transmission(&block, RATE));
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(messages[0].more);
    }

    /// The one thing the sideband can do to an ACARS receiver: mirror the spectrum and invert
    /// every bit. It must cost nothing.
    #[test]
    fn a_mirrored_spectrum_decodes_the_same() {
        let normal = transmission(&downlink(), RATE);
        let mirrored: Vec<Complex<f32>> = normal.iter().map(Complex::conj).collect();
        assert_eq!(decode(&mirrored), decode(&normal));
    }

    #[test]
    fn decodes_through_additive_noise() {
        let mut iq = transmission(&downlink(), RATE);
        testgen::add_noise(&mut iq, 0xabad_1dea, 0.15);
        let mut filtered = Vec::new();
        channel_filter(&AcarsParams::default())
            .unwrap()
            .process(&iq, &mut filtered);
        let messages = decode(&filtered);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert_eq!(messages[0].text, "REPORT ENGINE 1 OK");
    }

    #[test]
    fn pure_noise_decodes_to_nothing() {
        for seed in [0x1234_5678, 0xdead_beef, 0x0f0f_0f0f] {
            let noise = complex_noise(seed, 0.2, 480_000);
            assert_eq!(decode(&noise), Vec::new(), "seed {seed:#x}");
        }
    }

    /// The CRC is the only thing standing between a corrupted block and a plausible-looking
    /// message, so a hit anywhere in the text must drop the block outright.
    #[test]
    fn a_corrupted_block_is_dropped_rather_than_guessed() {
        let mut framer = Framer::new();
        let good = testgen::acars::block_bytes(&downlink());
        let mut out = ChannelOutputs::default();
        feed_bytes(&mut framer, &good, &mut out);
        assert_eq!(out.events.len(), 1, "the intact block must decode");

        for corrupt_at in [4usize, 12, 20, good.len() - 3] {
            let mut broken = good.clone();
            broken[corrupt_at] ^= 0x03;
            let mut framer = Framer::new();
            let mut out = ChannelOutputs::default();
            feed_bytes(&mut framer, &broken, &mut out);
            assert!(
                out.events.is_empty(),
                "a block corrupted at {corrupt_at} was accepted"
            );
        }
    }

    fn feed_bytes(framer: &mut Framer, bytes: &[u8], out: &mut ChannelOutputs) {
        for &byte in bytes {
            for i in 0..8 {
                framer.push((byte >> i) & 1 == 1, out);
            }
        }
    }

    #[test]
    fn ragged_block_splits_decode_identically() {
        let iq = transmission(&downlink(), RATE);
        let whole = decode_blocks(&mut channel(), &iq, &[iq.len()]);
        let ragged = decode_blocks(&mut channel(), &iq, &BLOCKS);
        let single = decode_blocks(&mut channel(), &iq, &[1]);
        assert_eq!(whole.len(), 1);
        assert_eq!(ragged, whole);
        assert_eq!(single, whole);
    }

    #[test]
    fn out_of_range_bandwidth_is_rejected() {
        for bandwidth_hz in [0.0, 3_000.0, f64::NAN, 60_000.0] {
            let p = AcarsParams { bandwidth_hz };
            assert!(
                matches!(channel_filter(&p), Err(ChannelError::InvalidSettings(_))),
                "{bandwidth_hz} must be rejected"
            );
            assert!(matches!(
                AcarsChannel::new(
                    ChannelCtx { input_rate: RATE },
                    settings(ChannelParams::Acars(p)),
                ),
                Err(ChannelError::InvalidSettings(_))
            ));
        }
    }

    #[test]
    fn mismatched_params_variant_is_rejected() {
        let mut chan = channel();
        let err = chan.apply(settings(ChannelParams::Nfm(NfmParams::default())));
        assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
    }

    #[test]
    fn wrong_input_rate_is_rejected() {
        let built = AcarsChannel::new(
            ChannelCtx {
                input_rate: 8_000.0,
            },
            settings(ChannelParams::Acars(AcarsParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }
}
