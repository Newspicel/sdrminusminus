use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{crc16_x25, hamming_distance};
use sdrmm_modem::{
    cpm::{CoherentCpmStream, CpmDemod, CpmParams, Mapping, TIMING_BW_BURST},
    pulse::{self, Norm},
};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, DecoderFamily, DstarParams,
    DvFrame, DvFrameKind, DvMode,
};

use super::{INPUT_RATE_HZ, tap_symbols, vocoder::DstarVocoder};
use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

pub(crate) const BAUD: f64 = 4_800.0;
pub(crate) const DEVIATION_HZ: f64 = 1_200.0;
pub(crate) const BT: f64 = 0.5;
pub(crate) const PULSE_SPAN: usize = 3;
pub(crate) const MATCHED_SPAN: usize = 3;
pub(crate) const BANDWIDTH_HZ: f64 = 6_250.0;

pub(crate) const SYNC: u32 = 0x0055_2D16;
pub(crate) const SYNC_BITS: u32 = 24;
pub(crate) const SYNC_TOLERANCE: u32 = 2;

const FRAME_BITS: usize = 96;
const DATA_BITS: usize = 24;
const FRAMES_PER_SUPERFRAME: usize = 21;

const COHERENT_LOOP_BW: f64 = 0.01;
const COHERENT_TIMING_BW: f64 = 0.02;
const RELEASE_SYMBOLS: usize = FRAME_BITS;

const SCRAMBLER: [u8; 3] = [0x70, 0x4F, 0x93];

const TYPE_TEXT: u8 = 0x4;
const TYPE_HEADER: u8 = 0x5;

const HEADER_BYTES: usize = 41;
const CALLSIGN_LEN: usize = 8;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "dstar".to_owned(),
    name: "D-STAR".to_owned(),
    summary: "D-STAR amateur digital voice".to_owned(),
    family: DecoderFamily::DigitalVoice,
    bandwidth_hz: BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: true,
    decoder_kind: Some("dv".to_owned()),
    ..ChannelDescriptor::default()
});

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Discriminator,
    Coherent,
}

pub struct DstarChannel {
    demod: CpmDemod,
    coherent: CoherentCpmStream,
    slicer: Mapping,
    decoder: Decoder,
    coherent_decoder: Decoder,
    source: Source,
    unlocked: usize,
    soft: Vec<f32>,
    coherent_soft: Vec<f32>,
    shadow: ChannelOutputs,
}

pub(crate) fn cpm_params(sps: f64) -> CpmParams {
    CpmParams::from_deviation(
        Mapping::natural(2),
        DEVIATION_HZ,
        BAUD,
        pulse::gaussian_freq(sps, BT, PULSE_SPAN, Norm::Area),
        sps,
    )
}

fn params(settings: &ChannelSettings) -> Result<&DstarParams, ChannelError> {
    match &settings.params {
        ChannelParams::Dstar(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "dstar channel got {} params",
            other.type_id()
        ))),
    }
}

pub(crate) fn occupied_band() -> (f64, f64) {
    (-BANDWIDTH_HZ / 2.0, BANDWIDTH_HZ / 2.0)
}

pub(crate) fn channel_filter() -> ChannelFilter {
    super::channel_filter(BANDWIDTH_HZ)
}

impl ChannelRx for DstarChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        params(&settings)?;
        let sps = ctx.input_rate / BAUD;
        let cpm = cpm_params(sps);
        let coherent = CoherentCpmStream::new(&cpm, COHERENT_LOOP_BW, COHERENT_TIMING_BW)
            .map_err(|e| ChannelError::InvalidSettings(e.to_string()))?;
        Ok(Self {
            demod: CpmDemod::new(
                &cpm,
                &pulse::gaussian(sps, BT, MATCHED_SPAN, Norm::Area),
                TIMING_BW_BURST,
            ),
            coherent,
            slicer: cpm.mapping().clone(),
            decoder: Decoder::new(),
            coherent_decoder: Decoder::new(),
            source: Source::Discriminator,
            unlocked: 0,
            soft: Vec::new(),
            coherent_soft: Vec::new(),
            shadow: ChannelOutputs::default(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        params(&settings)?;
        Ok(())
    }

    fn retuned(&mut self) {
        self.demod.reset();
        self.coherent.reset();
        self.decoder.reset();
        self.coherent_decoder.reset();
        self.source = Source::Discriminator;
        self.unlocked = 0;
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.soft.clear();
        self.demod.process(iq, &mut self.soft);
        self.coherent_soft.clear();
        self.coherent.process(iq, &mut self.coherent_soft);
        self.follow_lock();
        self.shadow.reset();
        let (live, shadow) = match self.source {
            Source::Discriminator => (&mut *out, &mut self.shadow),
            Source::Coherent => (&mut self.shadow, &mut *out),
        };
        feed(&mut self.decoder, &self.soft, &self.slicer, live);
        feed(
            &mut self.coherent_decoder,
            &self.coherent_soft,
            &self.slicer,
            shadow,
        );
        self.tap(out);
    }
}

fn feed(decoder: &mut Decoder, soft: &[f32], slicer: &Mapping, out: &mut ChannelOutputs) {
    for &symbol in soft {
        decoder.push(slicer.slice(symbol) == 1, out);
    }
}

impl DstarChannel {
    fn follow_lock(&mut self) {
        for &locked in self.coherent.locked() {
            self.unlocked = if locked { 0 } else { self.unlocked + 1 };
        }
        let next = match self.source {
            Source::Discriminator if self.coherent.is_locked() => Source::Coherent,
            Source::Coherent if self.unlocked >= RELEASE_SYMBOLS => Source::Discriminator,
            current => current,
        };
        if next == self.source {
            return;
        }
        self.source = next;
        match next {
            Source::Coherent => self.coherent_decoder.adopt(&self.decoder),
            Source::Discriminator => self.decoder.adopt(&self.coherent_decoder),
        }
    }

    fn tap(&self, out: &mut ChannelOutputs) {
        match self.source {
            Source::Discriminator => tap_symbols(
                out,
                &self.demod,
                &self.soft,
                &self.slicer,
                BAUD,
                INPUT_RATE_HZ,
            ),
            Source::Coherent => out.symbols.levels(
                &self.coherent_soft,
                self.coherent.locked(),
                &self.slicer,
                BAUD,
                self.coherent.frequency_error_cycles_per_sample() * INPUT_RATE_HZ,
            ),
        }
    }
}

struct Decoder {
    register: u32,
    bit: usize,
    frame: usize,
    synced: bool,
    data: u32,
    voice_bits: [bool; 72],
    packet: Vec<u8>,
    header: Vec<u8>,
    text: [u8; 20],
    reported: Option<String>,
    vocoder: DstarVocoder,
}

impl Decoder {
    fn new() -> Self {
        Self {
            register: 0,
            bit: 0,
            frame: 0,
            synced: false,
            data: 0,
            voice_bits: [false; 72],
            packet: Vec::with_capacity(6),
            header: Vec::with_capacity(HEADER_BYTES),
            text: [b' '; 20],
            reported: None,
            vocoder: DstarVocoder::new(),
        }
    }

    fn adopt(&mut self, other: &Self) {
        self.reported.clone_from(&other.reported);
    }

    fn reset(&mut self) {
        self.register = 0;
        self.synced = false;
        self.bit = 0;
        self.frame = 0;
        self.packet.clear();
        self.header.clear();
        self.reported = None;
        self.vocoder.reset();
    }

    fn push(&mut self, bit: bool, out: &mut ChannelOutputs) {
        self.register = self.register << 1 | u32::from(bit);
        if !self.synced {
            let mask = (1u32 << SYNC_BITS) - 1;
            if hamming_distance(u64::from(self.register & mask), u64::from(SYNC & mask))
                <= SYNC_TOLERANCE
            {
                self.synced = true;
                self.bit = 0;
                self.frame = 1;
                self.packet.clear();
            }
            return;
        }
        self.bit += 1;
        if self.bit <= FRAME_BITS - DATA_BITS {
            self.voice_bits[self.bit - 1] = bit;
        }
        if self.bit > FRAME_BITS - DATA_BITS {
            self.data = self.data << 1 | u32::from(bit);
        }
        if self.bit < FRAME_BITS {
            return;
        }
        self.bit = 0;
        self.vocoder.decode(&self.voice_bits, false, out);
        let frame = self.frame;
        self.frame += 1;
        if self.frame >= FRAMES_PER_SUPERFRAME {
            self.synced = false;
        }
        if !frame.is_multiple_of(FRAMES_PER_SUPERFRAME) {
            self.slow_data(frame, out);
        }
    }

    fn slow_data(&mut self, frame: usize, out: &mut ChannelOutputs) {
        for (i, mask) in SCRAMBLER.into_iter().enumerate() {
            self.packet.push((self.data >> (16 - i * 8)) as u8 ^ mask);
        }
        if frame.is_multiple_of(2) && self.packet.len() >= 6 {
            let packet: Vec<u8> = self.packet.drain(..6).collect();
            self.packet.clear();
            self.packet_complete(&packet, out);
        } else if self.packet.len() > 6 {
            self.packet.clear();
        }
    }

    fn packet_complete(&mut self, packet: &[u8], out: &mut ChannelOutputs) {
        let kind = packet[0] >> 4;
        let length = usize::from(packet[0] & 0x0F).min(packet.len() - 1);
        match kind {
            TYPE_HEADER => {
                if self.header.len() + length > HEADER_BYTES {
                    self.header.clear();
                }
                self.header.extend_from_slice(&packet[1..=length]);
                if self.header.len() == HEADER_BYTES
                    && let Some(frame) = self.header_frame()
                {
                    out.events.push(DecoderEvent::Dv(frame));
                }
            }
            TYPE_TEXT => {
                let slot = usize::from(packet[0] & 0x03) * 5;
                for (i, &byte) in packet[1..5].iter().enumerate() {
                    if slot + i < self.text.len() {
                        self.text[slot + i] = byte;
                    }
                }
            }
            _ => {}
        }
    }

    fn header_frame(&mut self) -> Option<DvFrame> {
        let header = std::mem::take(&mut self.header);
        let (body, crc) = header.split_at(HEADER_BYTES - 2);
        if crc16_x25(body) != u16::from_le_bytes([crc[0], crc[1]]) {
            return None;
        }
        let call = |offset: usize, len: usize| {
            let text = String::from_utf8_lossy(&header[offset..offset + len])
                .trim()
                .to_owned();
            (!text.is_empty()).then_some(text)
        };
        let source = call(27, CALLSIGN_LEN);
        if self.reported == source && source.is_some() {
            return None;
        }
        self.reported.clone_from(&source);

        let mut frame = DvFrame::new(DvMode::Dstar, DvFrameKind::Header);
        frame.destination_call = call(19, CALLSIGN_LEN);
        frame.source_call = source;
        frame.via = call(3, CALLSIGN_LEN);
        frame.group_call = Some(frame.destination_call.as_deref() == Some("CQCQCQ"));
        let text = String::from_utf8_lossy(&self.text).trim().to_owned();
        frame.text = (!text.is_empty()).then_some(text);
        Some(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        dv::testutil::{decode, decode_with_audio},
        testgen::dv::dstar as tx,
        testutil::settings,
    };

    fn channel() -> DstarChannel {
        DstarChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(ChannelParams::Dstar(DstarParams::default())),
        )
        .expect("dstar channel")
    }

    #[test]
    fn decodes_the_callsigns_from_the_slow_data_channel() {
        let call = tx::Call::default();
        let iq = tx::transmission(&call, INPUT_RATE_HZ);
        let frames = decode(&mut channel(), &iq);

        let header = frames.first().expect("a decoded frame");
        assert_eq!(header.mode, DvMode::Dstar);
        assert_eq!(header.kind, DvFrameKind::Header);
        assert_eq!(header.source_call.as_deref(), Some(call.mycall.as_str()));
        assert_eq!(
            header.destination_call.as_deref(),
            Some(call.urcall.as_str())
        );
        assert_eq!(header.via.as_deref(), Some(call.repeater.as_str()));
        assert_eq!(header.group_call, Some(true));
    }

    #[test]
    fn a_repeated_header_is_reported_once() {
        let iq = tx::transmission(&tx::Call::default(), INPUT_RATE_HZ);
        assert_eq!(decode(&mut channel(), &iq).len(), 1);
    }

    #[test]
    fn decodes_ambe_voice_to_audio() {
        let iq = tx::transmission(&tx::Call::default(), INPUT_RATE_HZ);
        let (_, audio) = decode_with_audio(&mut channel(), &iq);
        assert!(
            audio.len() >= 40 * 960,
            "missing D-STAR audio: {}",
            audio.len()
        );
        assert!(audio.iter().all(|sample| sample.is_finite()));
        assert!(audio.iter().all(|sample| sample.abs() <= 1.0));
    }

    #[test]
    fn noise_decodes_to_nothing() {
        let noise = crate::testutil::complex_noise(81, 0.5, 400_000);
        assert!(decode(&mut channel(), &noise).is_empty());
    }

    fn weak(snr_db: f32, offset_hz: f64, seed: u64) -> Vec<Complex<f32>> {
        let mut iq = vec![Complex::new(0.0, 0.0); 9_600];
        iq.extend(tx::repeated_transmission(
            &tx::Call::default(),
            6,
            INPUT_RATE_HZ,
        ));
        let step = std::f64::consts::TAU * offset_hz / INPUT_RATE_HZ;
        for (k, s) in iq.iter_mut().enumerate() {
            let theta = step * k as f64;
            *s *= Complex::new(theta.cos() as f32, theta.sin() as f32);
        }
        let signal_power = 1.0;
        let sigma = (signal_power / 10f32.powf(snr_db / 10.0) / 2.0).sqrt();
        crate::testutil::add_awgn(&mut iq, sigma, seed);
        let mut filtered = Vec::new();
        channel_filter().process(&iq, &mut filtered);
        filtered
    }

    fn discriminator_headers(iq: &[Complex<f32>]) -> usize {
        let sps = INPUT_RATE_HZ / BAUD;
        let cpm = cpm_params(sps);
        let mut demod = CpmDemod::new(
            &cpm,
            &pulse::gaussian(sps, BT, MATCHED_SPAN, Norm::Area),
            TIMING_BW_BURST,
        );
        let mut decoder = Decoder::new();
        let mut out = ChannelOutputs::default();
        let mut soft = Vec::new();
        demod.process(iq, &mut soft);
        feed(&mut decoder, &soft, cpm.mapping(), &mut out);
        out.events.len()
    }

    #[test]
    fn coherent_detection_decodes_headers_the_discriminator_loses() {
        let (mut coherent, mut discriminator) = (0, 0);
        for seed in 0..4 {
            let iq = weak(-2.0, 700.0, 0x5eed + seed);
            coherent += usize::from(!decode(&mut channel(), &iq).is_empty());
            discriminator += usize::from(discriminator_headers(&iq) > 0);
        }
        assert_eq!(coherent, 4, "coherent decoded {coherent} of 4");
        assert_eq!(
            discriminator, 0,
            "discriminator decoded {discriminator} of 4"
        );
    }
}
