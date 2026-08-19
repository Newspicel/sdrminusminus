use num_complex::Complex;

use super::{
    bb::BaseBandFrame,
    bch::Bch,
    frame::{Constellation, ModCod, Modulation, deinterleave, demodulate, interleave, modulate},
    ldpc::{Ldpc, Rate},
    pl::{self, Scrambler, Signalling},
};
use crate::datv::dvbs::PACKET;

const LOCK_COHERENCE: f32 = 0.75;
const NOISE: f32 = 0.25;

#[derive(Clone, Copy, Debug, Default)]
pub struct Dvbs2Metrics {
    pub frames_ok: u32,
    pub frames_bad: u32,
    pub corrected_bits: u32,
    pub iterations: u32,
}

fn message_bits(modcod: ModCod, short: bool) -> usize {
    let information = modcod.rate.information(short);
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
            ldpc: Ldpc::new(modcod.rate, short)?,
            bch: Bch::new(short, modcod.correct(short), message),
            baseband: BaseBandFrame::new(message),
            constellation: Constellation::new(modcod.modulation, modcod.rate),
        })
    }

    #[must_use]
    pub const fn signalling(&self, pilots: bool) -> Signalling {
        Signalling {
            modcod: self.modcod.index,
            short: self.short,
            pilots,
        }
    }
}

pub struct Dvbs2Encoder {
    codec: Codec,
    pilots: bool,
    carry: u8,
    scrambler: Scrambler,
    coded: Vec<bool>,
    payload: Vec<Complex<f32>>,
}

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
        })
    }

    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.codec.baseband.capacity()
    }

    pub fn frame(&mut self, packets: &[[u8; PACKET]], out: &mut Vec<Complex<f32>>) -> bool {
        let Some(baseband) = self.codec.baseband.build(packets, &mut self.carry) else {
            return false;
        };
        let mut protected = Vec::new();
        self.codec.bch.encode(&baseband, &mut protected);
        self.coded.clear();
        self.codec.ldpc.encode(&protected, &mut self.coded);
        let interleaved = interleave(
            &self.coded,
            self.codec.modcod.modulation,
            self.codec.modcod.rate,
        );
        self.payload.clear();
        modulate(&interleaved, &self.codec.constellation, &mut self.payload);
        self.scrambler.reset();
        self.scrambler.scramble(&mut self.payload);
        pl::header(self.codec.signalling(self.pilots), out);
        let slots = self.codec.modcod.slots(self.codec.short);
        for slot in 0..slots {
            if self.pilots && slot > 0 && slot.is_multiple_of(pl::PILOT_PERIOD) {
                out.extend(std::iter::repeat_n(pl::pilot_symbol(), pl::PILOT_LENGTH));
            }
            let start = slot * pl::SLOT;
            out.extend_from_slice(&self.payload[start..start + pl::SLOT]);
        }
        true
    }
}

pub struct Dvbs2Decoder {
    codec: Option<Codec>,
    scrambler: Scrambler,
    pending: Vec<Complex<f32>>,
    payload: Vec<Complex<f32>>,
    llrs: Vec<f32>,
    bits: Vec<bool>,
    searched: usize,
    pub metrics: Dvbs2Metrics,
    pub signalling: Option<Signalling>,
}

impl Dvbs2Decoder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            codec: None,
            scrambler: Scrambler::new(),
            pending: Vec::new(),
            payload: Vec::new(),
            llrs: Vec::new(),
            bits: Vec::new(),
            searched: 0,
            metrics: Dvbs2Metrics::default(),
            signalling: None,
        }
    }

    pub fn reset(&mut self) {
        self.codec = None;
        self.pending.clear();
        self.searched = 0;
        self.metrics = Dvbs2Metrics::default();
        self.signalling = None;
    }

    #[must_use]
    pub fn mode(&self) -> Option<(Modulation, Rate)> {
        self.codec
            .as_ref()
            .map(|codec| (codec.modcod.modulation, codec.modcod.rate))
    }

    pub fn push(&mut self, symbols: &[Complex<f32>], packets: &mut Vec<[u8; PACKET]>) {
        self.pending.extend_from_slice(symbols);
        while self.step(packets) {}
        if self.pending.len() > 4 * (pl::HEADER + 360 * pl::SLOT) {
            let excess = self.pending.len() - 2 * (pl::HEADER + 360 * pl::SLOT);
            self.pending.drain(..excess);
            self.searched = self.searched.saturating_sub(excess);
        }
    }

    fn step(&mut self, packets: &mut Vec<[u8; PACKET]>) -> bool {
        while self.searched + pl::HEADER <= self.pending.len() {
            let at = self.searched;
            let Some((coherence, signalling, reference)) =
                pl::correlate_header(&self.pending[at..])
            else {
                return false;
            };
            if coherence < LOCK_COHERENCE {
                self.searched += 1;
                continue;
            }
            let Some(modcod) = ModCod::from_index(signalling.modcod) else {
                self.searched += 1;
                continue;
            };
            let slots = modcod.slots(signalling.short);
            let span = pl::frame_symbols(slots, signalling.pilots);
            if at + span > self.pending.len() {
                return false;
            }
            self.signalling = Some(signalling);
            self.consume(at, signalling, modcod, slots, reference, packets);
            self.pending.drain(..at + span);
            self.searched = 0;
            return true;
        }
        false
    }

    fn consume(
        &mut self,
        at: usize,
        signalling: Signalling,
        modcod: ModCod,
        slots: usize,
        reference: Complex<f32>,
        packets: &mut Vec<[u8; PACKET]>,
    ) {
        if self
            .codec
            .as_ref()
            .is_none_or(|codec| codec.modcod != modcod || codec.short != signalling.short)
        {
            self.codec = Codec::new(modcod, signalling.short);
        }
        let Some(codec) = &mut self.codec else {
            self.metrics.frames_bad += 1;
            return;
        };
        self.payload.clear();
        let mut cursor = at + pl::HEADER;
        for slot in 0..slots {
            if signalling.pilots && slot > 0 && slot.is_multiple_of(pl::PILOT_PERIOD) {
                cursor += pl::PILOT_LENGTH;
            }
            self.payload
                .extend_from_slice(&self.pending[cursor..cursor + pl::SLOT]);
            cursor += pl::SLOT;
        }
        let correction = reference.conj() / reference.norm().max(f32::EPSILON);
        for symbol in &mut self.payload {
            *symbol *= correction;
        }
        self.scrambler.reset();
        self.scrambler.descramble(&mut self.payload);
        self.llrs.clear();
        demodulate(&self.payload, &codec.constellation, NOISE, &mut self.llrs);
        let ordered = deinterleave(&self.llrs, modcod.modulation, modcod.rate);
        self.bits.clear();
        let Some(iterations) = codec.ldpc.decode(&ordered, &mut self.bits) else {
            self.metrics.frames_bad += 1;
            return;
        };
        self.metrics.iterations += iterations as u32;
        let Some(corrected) = codec.bch.decode(&mut self.bits) else {
            self.metrics.frames_bad += 1;
            return;
        };
        self.metrics.corrected_bits += corrected as u32;
        self.bits.truncate(codec.bch.message());
        match codec.baseband.read(&self.bits) {
            Some(found) => {
                self.metrics.frames_ok += 1;
                packets.extend(found);
            }
            None => self.metrics.frames_bad += 1,
        }
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
        let mut received = Vec::new();
        for block in symbols.chunks(4_096) {
            decoder.push(block, &mut received);
        }
        (sent, received)
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
        let mut received = Vec::new();
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
        let mut received = Vec::new();
        decoder.push(&symbols, &mut received);
        assert_eq!(received, sent);
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
            let mut received = Vec::new();
            decoder.push(&turned, &mut received);
            assert_eq!(received, sent, "turned by {turn} rad");
        }
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
        let mut received = Vec::new();
        decoder.push(&noise, &mut received);
        assert!(received.is_empty());
        assert_eq!(decoder.metrics.frames_ok, 0);
    }
}
