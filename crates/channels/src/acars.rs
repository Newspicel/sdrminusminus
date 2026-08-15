use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{DcBlocker, Decimator, crc16_ccitt, design_lowpass};
use sdrmm_modem::{
    cpm::{CpmDemod, CpmParams, Mapping, RealDetector, TIMING_BW_BURST},
    pulse::{self, Norm},
};
use sdrmm_wire::{
    AcarsMessage, AcarsParams, ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const CHANNEL_TAPS: usize = 129;

const BAUD: f64 = 2_400.0;
const CENTRE_HZ: f64 = 1_800.0;
const DEVIATION_HZ: f64 = 600.0;

const SYN: u8 = 0x16;
const SOH: u8 = 0x01;
const STX: u8 = 0x02;
const ETX: u8 = 0x83;
const ETB: u8 = 0x97;
const ETB_DATA: u8 = ETB & 0x7F;
const NAK: u8 = 0x15;

const MIN_BLOCK: usize = 13;
const MAX_BLOCK: usize = 240;

const ADDRESS: std::ops::Range<usize> = 1..8;
const ACK: usize = 8;
const LABEL: std::ops::Range<usize> = 9..11;
const BLOCK_ID: usize = 11;
const BLOCK_START: usize = 12;
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
    demod: CpmDemod,
    soft: Vec<f32>,
    mapping: Mapping,
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
        let sps = rate / BAUD;
        let cpm = CpmParams::from_deviation(
            Mapping::natural(2),
            DEVIATION_HZ,
            BAUD,
            pulse::rect(sps, Norm::Area),
            sps,
        );
        let demod = CpmDemod::real(
            &cpm,
            &pulse::rect(sps, Norm::Area),
            TIMING_BW_BURST,
            rate,
            RealDetector::Discriminator {
                centre_hz: CENTRE_HZ,
            },
        );
        Ok(Self {
            envelope: Vec::new(),
            dc: DcBlocker::new(),
            demod,
            soft: Vec::new(),
            mapping: cpm.mapping().clone(),
            framer: Framer::new(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        check_params(params(&settings)?)
    }

    fn retuned(&mut self) {
        self.framer = Framer::new();
        self.demod.reset();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.envelope.clear();
        self.envelope.extend(iq.iter().map(|s| s.norm()));
        self.dc.process(&mut self.envelope);

        self.soft.clear();
        self.demod.process_real(&self.envelope, &mut self.soft);
        for &s in &self.soft {
            self.framer.push(self.mapping.slice(s) == 1, out);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Hunt,
    Sync2,
    Soh,
    Text,
    Crc,
}

struct Framer {
    reg: u8,
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

    fn decode(&self) -> Option<AcarsMessage> {
        if self.block.len() < MIN_BLOCK {
            return None;
        }
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

fn field(raw: &[u8]) -> String {
    printable(raw).trim().to_owned()
}

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
        let mut chan = AcarsChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Acars(AcarsParams::default())),
        )
        .unwrap();
        let mut out = ChannelOutputs::default();
        chan.process(&complex_noise(0x0b5e_11e5, 0.01, RATE as usize), &mut out);
        assert!(out.events.is_empty(), "the dead channel decoded something");
        chan
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
