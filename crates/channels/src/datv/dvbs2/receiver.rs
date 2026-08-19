use num_complex::Complex;

#[cfg(any(test, feature = "test-signals"))]
use super::frame::{interleave, modulate};
use super::{
    bb::{BaseBandFrame, StreamKind},
    bch::Bch,
    frame::{Constellation, ModCod, Modulation, deinterleave, demodulate},
    gse::{Gse, GseMetrics, GsePdu},
    ldpc::{Frame, Ldpc, Rate},
    pl::{self, Scrambler, Signalling},
    vlsnr::{self, Piece, VlMode, VlSnrCodec},
};
use crate::datv::dvbs::PACKET;

const LOCK_COHERENCE: f32 = 0.75;
const NOISE: f32 = 0.25;
const ACQUIRE_GAIN: f32 = 1.0;
const TRACK_GAIN: f32 = 0.02;
const VLSNR_CONFIDENCE: f32 = 0.5;

#[derive(Clone, Copy, Debug, Default)]
pub struct Dvbs2Metrics {
    pub frames_ok: u32,
    pub frames_bad: u32,
    pub frames_skipped: u32,
    pub corrected_bits: u32,
    pub iterations: u32,
    pub gse: GseMetrics,
}

#[derive(Clone, Debug, Default)]
pub struct Dvbs2Output {
    pub packets: Vec<[u8; PACKET]>,
    pub pdus: Vec<GsePdu>,
}

impl Dvbs2Output {
    pub fn clear(&mut self) {
        self.packets.clear();
        self.pdus.clear();
    }
}

fn message_bits(modcod: ModCod, short: bool) -> usize {
    let information = modcod.rate.information(Frame::of(short));
    information.saturating_sub(modcod.correct(short) * if short { 14 } else { 16 })
}

pub struct Codec {
    pub modcod: ModCod,
    pub short: bool,
    pub ldpc: Ldpc,
    pub bch: Bch,
    pub baseband: BaseBandFrame,
    pub constellation: Constellation,
}

impl Codec {
    #[must_use]
    pub fn new(modcod: ModCod, short: bool) -> Option<Self> {
        let message = message_bits(modcod, short);
        if message == 0 {
            return None;
        }
        Some(Self {
            modcod,
            short,
            ldpc: Ldpc::new(modcod.rate, Frame::of(short))?,
            bch: Bch::new(Frame::of(short), modcod.correct(short), message),
            baseband: BaseBandFrame::new(message),
            constellation: Constellation::new(modcod.modulation, modcod.rate),
        })
    }

    #[cfg(any(test, feature = "test-signals"))]
    #[must_use]
    pub const fn signalling(&self, pilots: bool) -> Signalling {
        Signalling {
            modcod: self.modcod.index,
            short: self.short,
            pilots,
        }
    }
}

#[cfg(any(test, feature = "test-signals"))]
pub struct Dvbs2Encoder {
    codec: Codec,
    pilots: bool,
    carry: u8,
    scrambler: Scrambler,
    coded: Vec<bool>,
    payload: Vec<Complex<f32>>,
    frame: Vec<Complex<f32>>,
}

#[cfg(any(test, feature = "test-signals"))]
impl Dvbs2Encoder {
    #[must_use]
    pub fn new(modcod: ModCod, short: bool, pilots: bool) -> Option<Self> {
        Some(Self {
            codec: Codec::new(modcod, short)?,
            pilots,
            carry: 0x47,
            scrambler: Scrambler::new(),
            coded: Vec::new(),
            payload: Vec::new(),
            frame: Vec::new(),
        })
    }

    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.codec.baseband.capacity()
    }

    #[must_use]
    pub const fn field_bytes(&self) -> usize {
        self.codec.baseband.field_bytes()
    }

    pub fn frame(&mut self, packets: &[[u8; PACKET]], out: &mut Vec<Complex<f32>>) -> bool {
        let Some(baseband) = self.codec.baseband.build(packets, &mut self.carry) else {
            return false;
        };
        self.emit(&baseband, out);
        true
    }

    pub fn generic(&mut self, field: &[u8], isi: Option<u8>, out: &mut Vec<Complex<f32>>) -> bool {
        let Some(baseband) = self.codec.baseband.encapsulate(field, isi) else {
            return false;
        };
        self.emit(&baseband, out);
        true
    }

    fn emit(&mut self, baseband: &[bool], out: &mut Vec<Complex<f32>>) {
        let mut protected = Vec::new();
        self.codec.bch.encode(baseband, &mut protected);
        self.coded.clear();
        self.codec.ldpc.encode(&protected, &mut self.coded);
        let interleaved = interleave(
            &self.coded,
            self.codec.modcod.modulation,
            self.codec.modcod.rate,
        );
        self.payload.clear();
        modulate(&interleaved, &self.codec.constellation, &mut self.payload);
        self.frame.clear();
        let slots = self.codec.modcod.slots(self.codec.short);
        for slot in 0..slots {
            if self.pilots && slot > 0 && slot.is_multiple_of(pl::PILOT_PERIOD) {
                self.frame
                    .extend(std::iter::repeat_n(pl::pilot_symbol(), pl::PILOT_LENGTH));
            }
            let start = slot * pl::SLOT;
            self.frame
                .extend_from_slice(&self.payload[start..start + pl::SLOT]);
        }
        self.scrambler.reset();
        self.scrambler.scramble(&mut self.frame);
        pl::header(self.codec.signalling(self.pilots), out);
        out.extend_from_slice(&self.frame);
    }
}

pub struct Dvbs2Decoder {
    codec: Option<Codec>,
    scrambler: Scrambler,
    pending: Vec<Complex<f32>>,
    window: Vec<Complex<f32>>,
    frame: Vec<Complex<f32>>,
    payload: Vec<Complex<f32>>,
    anchors: Vec<(usize, f32)>,
    llrs: Vec<f32>,
    bits: Vec<bool>,
    searched: usize,
    frequency: f32,
    good: u32,
    gse: Gse,
    wanted: Option<u8>,
    seen: Vec<u32>,
    very_low: Option<VlSnrCodec>,
    pub metrics: Dvbs2Metrics,
    pub signalling: Option<Signalling>,
    pub stream: Option<StreamKind>,
    pub isi: Option<u8>,
}

impl Dvbs2Decoder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            codec: None,
            scrambler: Scrambler::new(),
            pending: Vec::new(),
            window: Vec::new(),
            frame: Vec::new(),
            payload: Vec::new(),
            anchors: Vec::new(),
            llrs: Vec::new(),
            bits: Vec::new(),
            searched: 0,
            frequency: 0.0,
            good: 0,
            gse: Gse::new(),
            wanted: None,
            seen: vec![0; 256],
            very_low: None,
            metrics: Dvbs2Metrics::default(),
            signalling: None,
            stream: None,
            isi: None,
        }
    }

    pub fn select(&mut self, isi: Option<u8>) {
        if self.wanted != isi {
            self.wanted = isi;
            self.gse.reset();
        }
    }

    pub fn reset(&mut self) {
        self.codec = None;
        self.very_low = None;
        self.pending.clear();
        self.searched = 0;
        self.frequency = 0.0;
        self.good = 0;
        self.gse.reset();
        self.seen.fill(0);
        self.metrics = Dvbs2Metrics::default();
        self.signalling = None;
        self.stream = None;
        self.isi = None;
    }

    #[must_use]
    pub fn streams(&self) -> Vec<(u8, u32)> {
        self.seen
            .iter()
            .enumerate()
            .filter(|&(_, &frames)| frames > 0)
            .map(|(isi, &frames)| (isi as u8, frames))
            .collect()
    }

    #[must_use]
    pub fn very_low_mode(&self) -> Option<VlMode> {
        self.very_low.as_ref().map(|codec| codec.mode)
    }

    #[must_use]
    pub fn mode(&self) -> Option<(Modulation, Rate)> {
        self.codec
            .as_ref()
            .map(|codec| (codec.modcod.modulation, codec.modcod.rate))
    }

    #[must_use]
    pub const fn frequency_error(&self) -> f32 {
        self.frequency
    }

    pub fn push(&mut self, symbols: &[Complex<f32>], out: &mut Dvbs2Output) {
        self.pending.extend_from_slice(symbols);
        while self.step(out) {}
        if self.pending.len() > 4 * (pl::HEADER + 360 * pl::SLOT) {
            let excess = self.pending.len() - 2 * (pl::HEADER + 360 * pl::SLOT);
            self.pending.drain(..excess);
            self.searched = self.searched.saturating_sub(excess);
        }
    }

    fn derotate(&mut self, at: usize, span: usize, rotation: f32, reference: Complex<f32>) {
        self.window.clear();
        self.window.reserve(span);
        let correction = reference.conj() / reference.norm().max(f32::EPSILON);
        for (index, &symbol) in self.pending[at..at + span].iter().enumerate() {
            self.window
                .push(symbol * Complex::from_polar(1.0, -rotation * index as f32) * correction);
        }
    }

    fn step(&mut self, out: &mut Dvbs2Output) -> bool {
        while self.searched + pl::HEADER <= self.pending.len() {
            let at = self.searched;
            let guess = self.frequency;
            self.derotate(at, pl::HEADER, guess, Complex::new(1.0, 0.0));
            let Some(fit) = pl::correlate_sof(&self.window) else {
                return false;
            };
            if fit.coherence < LOCK_COHERENCE {
                self.searched += 1;
                continue;
            }
            self.derotate(at, pl::HEADER, guess, fit.reference);
            let Some(signalling) = pl::read_signalling(&self.window) else {
                self.searched += 1;
                continue;
            };
            let set = vlsnr::set_of(signalling);
            let span = match (set, ModCod::from_index(signalling.modcod)) {
                (Some(set), _) => vlsnr::frame_symbols(set),
                (None, Some(modcod)) => {
                    pl::frame_symbols(modcod.slots(signalling.short), signalling.pilots)
                }
                (None, None) => {
                    self.searched += 1;
                    continue;
                }
            };
            if at + span > self.pending.len() {
                return false;
            }
            self.signalling = Some(signalling);
            let anchor = pl::header_phase(&self.window, signalling).unwrap_or(0.0);
            self.derotate(
                at,
                span,
                guess,
                fit.reference * Complex::from_polar(1.0, anchor),
            );
            match ModCod::from_index(signalling.modcod) {
                Some(modcod) if set.is_none() => {
                    self.consume(signalling, modcod, modcod.slots(signalling.short), out);
                }
                _ => self.consume_very_low(out),
            }
            let gain = if self.good > 0 {
                TRACK_GAIN
            } else {
                ACQUIRE_GAIN
            };
            self.frequency += gain * fit.rotation;
            self.pending.drain(..at + span);
            self.searched = 0;
            return true;
        }
        false
    }

    fn anchor(&mut self, at: usize, len: usize) {
        let Some(phase) = pl::pilot_phase(&self.frame[at..at + len]) else {
            return;
        };
        let previous = self.anchors[self.anchors.len() - 1].1;
        let turns = ((phase - previous) / std::f32::consts::TAU).round();
        self.anchors.push((
            pl::HEADER + at + len / 2,
            phase - turns * std::f32::consts::TAU,
        ));
    }

    fn track_phase(&mut self, signalling: Signalling, slots: usize) {
        self.anchors.clear();
        self.anchors.push((pl::HEADER / 2, 0.0));
        if signalling.pilots {
            for block in 1..=pl::pilot_blocks(slots) {
                self.anchor(pl::pilot_block_start(block), pl::PILOT_LENGTH);
            }
        }
        self.interpolate();
    }

    fn interpolate(&mut self) {
        if self.anchors.len() < 2 {
            return;
        }
        let mut segment = 0;
        for (index, symbol) in self.frame.iter_mut().enumerate() {
            let position = index + pl::HEADER;
            while segment + 2 < self.anchors.len() && position >= self.anchors[segment + 1].0 {
                segment += 1;
            }
            let (first, second) = (self.anchors[segment], self.anchors[segment + 1]);
            let span = (second.0 - first.0) as f32;
            let fraction = (position as f32 - first.0 as f32) / span;
            let phase = first.1 + fraction * (second.1 - first.1);
            *symbol *= Complex::from_polar(1.0, -phase);
        }
    }

    fn consume_very_low(&mut self, out: &mut Dvbs2Output) {
        self.frame.clear();
        self.frame.extend_from_slice(&self.window[pl::HEADER..]);
        let Some((index, confidence)) = vlsnr::read_header(&self.frame) else {
            self.fail();
            return;
        };
        if confidence < VLSNR_CONFIDENCE {
            self.fail();
            return;
        }
        let Some(mode) = VlMode::from_header(index) else {
            self.fail();
            return;
        };
        if self
            .very_low
            .as_ref()
            .is_none_or(|codec| codec.mode != mode)
        {
            self.very_low = VlSnrCodec::new(mode);
        }
        let Some(codec) = &mut self.very_low else {
            self.fail();
            return;
        };
        vlsnr::scramble(
            &mut self.scrambler,
            &codec.layout,
            mode.carrier,
            &mut self.frame,
            false,
        );
        let layout = codec.layout.clone();
        self.anchors.clear();
        self.anchors.push((pl::HEADER / 2, 0.0));
        let mut at = 0;
        for piece in &layout {
            if piece.is_pilot() {
                self.anchor(at, piece.len());
            }
            at += piece.len();
        }
        self.interpolate();
        self.payload.clear();
        let mut at = 0;
        for piece in &layout {
            if let Piece::Payload(len) = piece {
                self.payload.extend_from_slice(&self.frame[at..at + len]);
            }
            at += piece.len();
        }
        let Some(codec) = &mut self.very_low else {
            self.fail();
            return;
        };
        match codec.decode(&self.payload) {
            Some(data) => {
                self.metrics.frames_ok += 1;
                self.good = self.good.saturating_add(1);
                self.deliver(data, out);
            }
            None => self.fail(),
        }
    }

    fn fail(&mut self) {
        self.metrics.frames_bad += 1;
        self.good = 0;
    }

    fn deliver(&mut self, data: super::bb::BaseBandData, out: &mut Dvbs2Output) {
        self.stream = Some(data.header.kind);
        self.isi = (!data.header.single).then_some(data.header.isi);
        if !data.header.single {
            self.seen[usize::from(data.header.isi)] += 1;
        }
        if self
            .wanted
            .is_some_and(|isi| !data.header.single && isi != data.header.isi)
        {
            self.metrics.frames_skipped += 1;
            return;
        }
        if data.header.kind.is_encapsulated() {
            self.gse.push(&data.field, &mut out.pdus);
            self.metrics.gse = self.gse.metrics;
        } else {
            out.packets.extend(data.transport());
        }
    }

    fn consume(
        &mut self,
        signalling: Signalling,
        modcod: ModCod,
        slots: usize,
        out: &mut Dvbs2Output,
    ) {
        if self
            .codec
            .as_ref()
            .is_none_or(|codec| codec.modcod != modcod || codec.short != signalling.short)
        {
            self.codec = Codec::new(modcod, signalling.short);
        }
        if self.codec.is_none() {
            self.fail();
            return;
        }
        self.frame.clear();
        self.frame.extend_from_slice(&self.window[pl::HEADER..]);
        self.scrambler.reset();
        self.scrambler.descramble(&mut self.frame);
        self.track_phase(signalling, slots);
        self.payload.clear();
        let mut cursor = 0;
        for slot in 0..slots {
            if signalling.pilots && slot > 0 && slot.is_multiple_of(pl::PILOT_PERIOD) {
                cursor += pl::PILOT_LENGTH;
            }
            self.payload
                .extend_from_slice(&self.frame[cursor..cursor + pl::SLOT]);
            cursor += pl::SLOT;
        }
        let Some(codec) = &mut self.codec else {
            self.fail();
            return;
        };
        self.llrs.clear();
        demodulate(&self.payload, &codec.constellation, NOISE, &mut self.llrs);
        let ordered = deinterleave(&self.llrs, modcod.modulation, modcod.rate);
        self.bits.clear();
        let Some(iterations) = codec.ldpc.decode(&ordered, &mut self.bits) else {
            self.fail();
            return;
        };
        self.metrics.iterations += iterations as u32;
        let Some(corrected) = codec.bch.decode(&mut self.bits) else {
            self.fail();
            return;
        };
        self.metrics.corrected_bits += corrected as u32;
        self.bits.truncate(codec.bch.message());
        let Some(data) = codec.baseband.read(&self.bits) else {
            self.fail();
            return;
        };
        self.metrics.frames_ok += 1;
        self.good = self.good.saturating_add(1);
        self.deliver(data, out);
    }
}

impl Default for Dvbs2Decoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datv::dvbs::SYNC;

    fn transport(count: usize, seed: u32) -> Vec<[u8; PACKET]> {
        let mut state = seed | 1;
        (0..count)
            .map(|index| {
                let mut packet = [0u8; PACKET];
                packet[0] = SYNC;
                packet[1] = 0x41;
                packet[2] = index as u8;
                for byte in &mut packet[3..] {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    *byte = state as u8;
                }
                packet
            })
            .collect()
    }

    fn assert_tail(sent: &[[u8; PACKET]], received: &[[u8; PACKET]], least: usize, what: &str) {
        assert!(
            received.len() >= least,
            "{what}: {} packets of {least} wanted",
            received.len()
        );
        assert_eq!(received, &sent[sent.len() - received.len()..], "{what}");
    }

    fn round_trip(
        modcod: ModCod,
        short: bool,
        pilots: bool,
    ) -> (Vec<[u8; PACKET]>, Vec<[u8; PACKET]>) {
        let mut encoder = Dvbs2Encoder::new(modcod, short, pilots).expect("a supported mode");
        let sent = transport(3 * encoder.capacity(), 19);
        let mut symbols = Vec::new();
        for chunk in sent.chunks(encoder.capacity()) {
            encoder.frame(chunk, &mut symbols);
        }
        let mut decoder = Dvbs2Decoder::new();
        let mut received = Dvbs2Output::default();
        for block in symbols.chunks(4_096) {
            decoder.push(block, &mut received);
        }
        (sent, received.packets)
    }

    #[test]
    fn a_short_frame_round_trips_through_the_whole_chain() {
        let modcod = ModCod::find(Modulation::Qpsk, Rate::R1_2).expect("QPSK 1/2");
        let (sent, received) = round_trip(modcod, true, false);
        assert_eq!(received, sent);
    }

    #[test]
    fn pilot_blocks_are_stepped_over() {
        let modcod = ModCod::find(Modulation::Qpsk, Rate::R3_4).expect("QPSK 3/4");
        let (sent, received) = round_trip(modcod, false, true);
        assert_eq!(received, sent);
    }

    #[test]
    fn every_catalogued_mode_round_trips() {
        for index in 1..=28u8 {
            let modcod = ModCod::from_index(index).expect("a catalogued index");
            for short in [false, true] {
                if Codec::new(modcod, short).is_none() {
                    assert!(short && modcod.rate == Rate::R9_10);
                    continue;
                }
                let (sent, received) = round_trip(modcod, short, false);
                assert_eq!(
                    received,
                    sent,
                    "modcod {index}: {:?} {} short={short}",
                    modcod.modulation,
                    modcod.rate.label()
                );
            }
        }
    }

    #[test]
    fn the_twisted_interleaver_survives_the_whole_chain() {
        let modcod = ModCod::find(Modulation::Psk8, Rate::R3_5).expect("8PSK 3/5");
        assert_eq!(modcod.index, 12);
        let (sent, received) = round_trip(modcod, false, true);
        assert_eq!(received, sent);
    }

    #[test]
    fn the_higher_order_constellations_carry_their_frames() {
        for (modulation, rate) in [
            (Modulation::Apsk16, Rate::R2_3),
            (Modulation::Apsk16, Rate::R9_10),
            (Modulation::Apsk32, Rate::R3_4),
            (Modulation::Apsk32, Rate::R9_10),
        ] {
            let modcod = ModCod::find(modulation, rate).expect("a catalogued mode");
            let (sent, received) = round_trip(modcod, false, true);
            assert_eq!(received, sent, "{modulation:?} {}", rate.label());
        }
    }

    #[test]
    fn the_signalling_read_from_the_header_names_the_mode() {
        let modcod = ModCod::find(Modulation::Psk8, Rate::R3_4).expect("8PSK 3/4");
        let mut encoder = Dvbs2Encoder::new(modcod, false, true).expect("a supported mode");
        let mut symbols = Vec::new();
        encoder.frame(&transport(encoder.capacity(), 3), &mut symbols);
        let mut decoder = Dvbs2Decoder::new();
        let mut received = Dvbs2Output::default();
        decoder.push(&symbols, &mut received);
        assert_eq!(
            decoder.signalling,
            Some(Signalling {
                modcod: modcod.index,
                short: false,
                pilots: true
            })
        );
        assert_eq!(decoder.mode(), Some((Modulation::Psk8, Rate::R3_4)));
        assert_eq!(decoder.metrics.frames_ok, 1);
    }

    #[test]
    fn a_frame_that_starts_late_in_the_stream_is_still_found() {
        let modcod = ModCod::find(Modulation::Qpsk, Rate::R1_2).expect("QPSK 1/2");
        let mut encoder = Dvbs2Encoder::new(modcod, true, false).expect("a supported mode");
        let sent = transport(encoder.capacity(), 23);
        let mut symbols = vec![Complex::new(0.01, -0.02); 137];
        encoder.frame(&sent, &mut symbols);
        let mut decoder = Dvbs2Decoder::new();
        let mut received = Dvbs2Output::default();
        decoder.push(&symbols, &mut received);
        assert_eq!(received.packets, sent);
    }

    #[test]
    fn a_turned_constellation_is_recovered_from_the_header_phase() {
        let modcod = ModCod::find(Modulation::Psk8, Rate::R3_4).expect("8PSK 3/4");
        let mut encoder = Dvbs2Encoder::new(modcod, false, false).expect("a supported mode");
        let sent = transport(encoder.capacity(), 29);
        let mut symbols = Vec::new();
        encoder.frame(&sent, &mut symbols);
        for turn in [0.0f32, 0.7, 1.9, 3.5, 5.1] {
            let turned: Vec<Complex<f32>> = symbols
                .iter()
                .map(|&value| value * Complex::from_polar(1.0, turn))
                .collect();
            let mut decoder = Dvbs2Decoder::new();
            let mut received = Dvbs2Output::default();
            decoder.push(&turned, &mut received);
            assert_eq!(received.packets, sent, "turned by {turn} rad");
        }
    }

    #[test]
    fn a_carrier_offset_is_tracked_across_a_frame() {
        for (modulation, rate) in [
            (Modulation::Qpsk, Rate::R1_2),
            (Modulation::Psk8, Rate::R3_5),
            (Modulation::Apsk16, Rate::R3_4),
            (Modulation::Apsk32, Rate::R5_6),
        ] {
            let modcod = ModCod::find(modulation, rate).expect("a catalogued mode");
            let mut encoder = Dvbs2Encoder::new(modcod, false, true).expect("a supported mode");
            let sent = transport(3 * encoder.capacity(), 31);
            let mut symbols = Vec::new();
            for chunk in sent.chunks(encoder.capacity()) {
                encoder.frame(chunk, &mut symbols);
            }
            let offset = 0.004f32;
            let turned: Vec<Complex<f32>> = symbols
                .iter()
                .enumerate()
                .map(|(index, &value)| {
                    value * Complex::from_polar(1.0, 0.9 + offset * index as f32)
                })
                .collect();
            let mut decoder = Dvbs2Decoder::new();
            let mut received = Dvbs2Output::default();
            for block in turned.chunks(4_096) {
                decoder.push(block, &mut received);
            }
            let label = format!("{modulation:?} {}", rate.label());
            assert_tail(&sent, &received.packets, 2 * encoder.capacity(), &label);
            assert!(
                (decoder.frequency_error() - offset).abs() < 1e-3,
                "{label}: {}",
                decoder.frequency_error()
            );
        }
    }

    #[test]
    fn a_stream_without_pilots_locks_after_the_first_frame() {
        let modcod = ModCod::find(Modulation::Apsk16, Rate::R2_3).expect("16APSK 2/3");
        let mut encoder = Dvbs2Encoder::new(modcod, true, false).expect("a supported mode");
        let sent = transport(4 * encoder.capacity(), 37);
        let mut symbols = Vec::new();
        for chunk in sent.chunks(encoder.capacity()) {
            encoder.frame(chunk, &mut symbols);
        }
        let turned: Vec<Complex<f32>> = symbols
            .iter()
            .enumerate()
            .map(|(index, &value)| value * Complex::from_polar(1.0, 0.002 * index as f32))
            .collect();
        let mut decoder = Dvbs2Decoder::new();
        let mut received = Dvbs2Output::default();
        for block in turned.chunks(4_096) {
            decoder.push(block, &mut received);
        }
        assert_tail(
            &sent,
            &received.packets,
            3 * encoder.capacity(),
            "16APSK 2/3 short",
        );
    }

    fn drive(symbols: &[Complex<f32>], decoder: &mut Dvbs2Decoder, out: &mut Dvbs2Output) {
        for block in symbols.chunks(4_096) {
            decoder.push(block, out);
        }
    }

    fn datagram(protocol: u16, label: &[u8], len: usize, seed: u32) -> GsePdu {
        let mut state = seed | 1;
        GsePdu {
            protocol,
            label: label.to_vec(),
            data: (0..len)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    state as u8
                })
                .collect(),
        }
    }

    #[test]
    fn an_encapsulated_stream_hands_over_whole_datagrams() {
        use super::super::gse::GseWriter;

        let modcod = ModCod::find(Modulation::Apsk16, Rate::R3_4).expect("16APSK 3/4");
        let mut encoder = Dvbs2Encoder::new(modcod, false, true).expect("a supported mode");
        let sent: Vec<GsePdu> = (0..6)
            .map(|index| datagram(0x0800, &[1, 2, 3, 4, 5, index], 900 + index as usize, 41))
            .collect();
        let mut writer = GseWriter::new();
        let mut symbols = Vec::new();
        let mut field = Vec::new();
        for pdu in &sent {
            writer.fragmented(pdu, 3, &mut field);
            GseWriter::pad(&mut field, encoder.field_bytes());
            assert!(encoder.generic(&field, None, &mut symbols));
            field.clear();
        }
        let mut decoder = Dvbs2Decoder::new();
        let mut received = Dvbs2Output::default();
        drive(&symbols, &mut decoder, &mut received);
        assert_eq!(received.pdus, sent);
        assert!(received.packets.is_empty());
        assert_eq!(decoder.stream, Some(StreamKind::GenericContinuous));
        assert_eq!(decoder.metrics.gse.crc_errors, 0);
        assert_eq!(decoder.metrics.gse.pdus, sent.len() as u32);
    }

    #[test]
    fn only_the_chosen_input_stream_is_handed_on() {
        use super::super::gse::GseWriter;

        let modcod = ModCod::find(Modulation::Qpsk, Rate::R3_4).expect("QPSK 3/4");
        let mut encoder = Dvbs2Encoder::new(modcod, true, false).expect("a supported mode");
        let mut symbols = Vec::new();
        let mut wanted = Vec::new();
        for round in 0..4u8 {
            for isi in [7u8, 9] {
                let pdu = datagram(0x86DD, &[isi, round, 0], 200, u32::from(round) + 1);
                if isi == 7 {
                    wanted.push(pdu.clone());
                }
                let mut field = Vec::new();
                GseWriter::whole(&pdu, &mut field);
                GseWriter::pad(&mut field, encoder.field_bytes());
                assert!(encoder.generic(&field, Some(isi), &mut symbols));
            }
        }
        let mut decoder = Dvbs2Decoder::new();
        decoder.select(Some(7));
        let mut received = Dvbs2Output::default();
        drive(&symbols, &mut decoder, &mut received);
        assert_eq!(received.pdus, wanted);
        assert_eq!(decoder.metrics.frames_skipped, 4);
        assert_eq!(decoder.isi, Some(9));

        let mut all = Dvbs2Decoder::new();
        let mut every = Dvbs2Output::default();
        drive(&symbols, &mut all, &mut every);
        assert_eq!(every.pdus.len(), 8);
        assert_eq!(all.metrics.frames_skipped, 0);
    }

    #[test]
    fn the_mode_may_change_from_one_frame_to_the_next() {
        let modes = [
            (Modulation::Qpsk, Rate::R1_2),
            (Modulation::Apsk32, Rate::R3_4),
            (Modulation::Psk8, Rate::R3_5),
            (Modulation::Apsk16, Rate::R9_10),
        ];
        let mut symbols = Vec::new();
        let mut sent = Vec::new();
        let mut seed = 43u32;
        for (modulation, rate) in modes {
            let modcod = ModCod::find(modulation, rate).expect("a catalogued mode");
            let mut encoder = Dvbs2Encoder::new(modcod, false, true).expect("a supported mode");
            let packets = transport(encoder.capacity(), seed);
            seed += 2;
            assert!(encoder.frame(&packets, &mut symbols));
            sent.extend(packets);
        }
        let mut decoder = Dvbs2Decoder::new();
        let mut received = Dvbs2Output::default();
        drive(&symbols, &mut decoder, &mut received);
        assert_eq!(received.packets, sent);
        assert_eq!(decoder.metrics.frames_ok, modes.len() as u32);
        assert_eq!(decoder.metrics.frames_bad, 0);
        assert_eq!(decoder.mode(), Some((Modulation::Apsk16, Rate::R9_10)));
    }

    #[test]
    fn every_very_low_signal_mode_round_trips_through_the_whole_chain() {
        use super::super::vlsnr::{CATALOGUE, VlSnrEncoder};

        for mode in CATALOGUE {
            let mut encoder = VlSnrEncoder::new(mode).unwrap_or_else(|| panic!("{}", mode.label));
            let sent = transport(2 * encoder.capacity(), 47);
            let mut symbols = Vec::new();
            for chunk in sent.chunks(encoder.capacity()) {
                assert!(encoder.frame(chunk, &mut symbols), "{}", mode.label);
            }
            let mut decoder = Dvbs2Decoder::new();
            let mut received = Dvbs2Output::default();
            drive(&symbols, &mut decoder, &mut received);
            assert_eq!(received.packets, sent, "{}", mode.label);
            assert_eq!(decoder.metrics.frames_bad, 0, "{}", mode.label);
            assert_eq!(decoder.very_low_mode(), Some(mode), "{}", mode.label);
            assert_eq!(decoder.mode(), None, "{}", mode.label);
        }
    }

    #[test]
    fn a_very_low_signal_frame_is_the_length_of_the_legacy_frame_it_hides_in() {
        use super::super::vlsnr::{CATALOGUE, VlSet, VlSnrEncoder};

        for mode in CATALOGUE {
            let mut encoder = VlSnrEncoder::new(mode).expect("a supported mode");
            let mut symbols = Vec::new();
            encoder.frame(&transport(encoder.capacity(), 51), &mut symbols);
            let legacy = match mode.set {
                VlSet::One => {
                    let modcod = ModCod::find(Modulation::Qpsk, Rate::R9_10).expect("QPSK 9/10");
                    pl::frame_symbols(modcod.slots(false), true)
                }
                VlSet::Two => {
                    let modcod =
                        ModCod::find(Modulation::Apsk16, Rate::R9_10).expect("16APSK 9/10");
                    pl::frame_symbols(modcod.slots(false), true)
                }
            };
            assert_eq!(symbols.len(), legacy, "{}", mode.label);
        }
    }

    #[test]
    fn a_very_low_signal_stream_carries_a_generic_stream_too() {
        use super::super::{
            gse::GseWriter,
            vlsnr::{VlMode, VlSnrEncoder},
        };

        let mode = VlMode::from_header(9).expect("BPSK 1/5 short");
        let mut encoder = VlSnrEncoder::new(mode).expect("a supported mode");
        let sent: Vec<GsePdu> = (0..3)
            .map(|index| datagram(0x86DD, &[7, 7, index], 200 + index as usize, 53))
            .collect();
        let mut symbols = Vec::new();
        for pdu in &sent {
            let mut field = Vec::new();
            GseWriter::whole(pdu, &mut field);
            GseWriter::pad(&mut field, encoder.field_bytes());
            assert!(encoder.generic(&field, Some(7), &mut symbols));
        }
        let mut decoder = Dvbs2Decoder::new();
        let mut received = Dvbs2Output::default();
        drive(&symbols, &mut decoder, &mut received);
        assert_eq!(received.pdus, sent);
        assert_eq!(decoder.isi, Some(7));
        assert_eq!(decoder.metrics.frames_bad, 0);
    }

    #[test]
    fn a_very_low_signal_frame_rides_out_a_carrier_offset() {
        use super::super::vlsnr::{VlMode, VlSnrEncoder};

        let mode = VlMode::from_header(11).expect("BPSK 1/3 short");
        let mut encoder = VlSnrEncoder::new(mode).expect("a supported mode");
        let sent = transport(3 * encoder.capacity(), 59);
        let mut symbols = Vec::new();
        for chunk in sent.chunks(encoder.capacity()) {
            encoder.frame(chunk, &mut symbols);
        }
        let turned: Vec<Complex<f32>> = symbols
            .iter()
            .enumerate()
            .map(|(index, &value)| value * Complex::from_polar(1.0, 1.7 + 0.003 * index as f32))
            .collect();
        let mut decoder = Dvbs2Decoder::new();
        let mut received = Dvbs2Output::default();
        drive(&turned, &mut decoder, &mut received);
        assert_tail(&sent, &received.packets, 2 * encoder.capacity(), "BPSK 1/3");
    }

    #[test]
    fn noise_yields_neither_a_frame_nor_a_packet() {
        let mut state = 0x0c0f_fee1u32;
        let noise: Vec<Complex<f32>> = (0..200_000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                Complex::new(
                    (state >> 16) as f32 / 32_768.0 - 1.0,
                    (state & 0xFFFF) as f32 / 32_768.0 - 1.0,
                )
            })
            .collect();
        let mut decoder = Dvbs2Decoder::new();
        let mut received = Dvbs2Output::default();
        decoder.push(&noise, &mut received);
        assert!(received.packets.is_empty());
        assert_eq!(decoder.metrics.frames_ok, 0);
    }
}
