#![allow(clippy::expect_used)]

use num_complex::Complex;
use sdrmm_dsp::FracResampler;
use sdrmm_modem::pulse::{self, Norm};
use sdrmm_wire::DatvCodeRate;

use crate::datv::{
    dvbs::{DvbsEncoder, PACKET},
    dvbs2::{
        frame::{ModCod, Modulation},
        ldpc::Rate,
        receiver::Dvbs2Encoder,
    },
    ts::{TsWriter, pat, pmt, sdt},
};

pub const SYMBOL_RATE: f64 = 250_000.0;
pub const PROGRAM_NAME: &str = "Rust TV";
pub const PROVIDER: &str = "sdr--";
pub const CODE_RATE: DatvCodeRate = DatvCodeRate::ThreeQuarters;

const INPUT_RATE_HZ: f64 = 2_000_000.0;
const ROLL_OFF: f64 = 0.35;
const SPS: usize = 4;
const PULSE_SPAN: usize = 8;
const TABLE_PERIOD: usize = 40;

fn elementary(len: usize, seed: u32) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect()
}

pub struct Multiplex {
    writer: TsWriter,
    queued: std::collections::VecDeque<[u8; PACKET]>,
    emitted: usize,
    frame: u32,
}

impl Multiplex {
    #[must_use]
    pub fn new() -> Self {
        Self {
            writer: TsWriter::new(),
            queued: std::collections::VecDeque::new(),
            emitted: 0,
            frame: 0,
        }
    }

    pub fn packet(&mut self) -> [u8; PACKET] {
        if self.queued.is_empty() {
            let mut batch = Vec::new();
            if self.emitted.is_multiple_of(TABLE_PERIOD) {
                self.writer.section(0x0000, &pat(), &mut batch);
                self.writer.section(0x0100, &pmt(), &mut batch);
                self.writer
                    .section(0x0011, &sdt(PROVIDER, PROGRAM_NAME), &mut batch);
            }
            let pts = 90_000 + u64::from(self.frame) * 3_600;
            self.writer.pes(
                0x0101,
                0xE0,
                pts,
                &elementary(1_400, self.frame * 2 + 1),
                &mut batch,
            );
            self.writer.pes(
                0x0102,
                0xC0,
                pts,
                &elementary(400, self.frame * 2 + 2),
                &mut batch,
            );
            self.frame += 1;
            self.queued.extend(batch);
        }
        self.emitted += 1;
        self.queued
            .pop_front()
            .unwrap_or_else(crate::datv::ts::null_packet)
    }
}

impl Default for Multiplex {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use]
pub fn transport(packets: usize) -> Vec<[u8; PACKET]> {
    let mut multiplex = Multiplex::new();
    (0..packets).map(|_| multiplex.packet()).collect()
}

#[must_use]
pub fn dvbs(seconds: usize) -> Vec<Complex<f32>> {
    let wanted = seconds * SYMBOL_RATE as usize;
    let mut encoder = DvbsEncoder::new(CODE_RATE);
    let mut multiplex = Multiplex::new();
    let mut symbols = Vec::with_capacity(wanted);
    while symbols.len() < wanted {
        encoder.packet(&multiplex.packet(), &mut symbols);
    }
    shape(&symbols)
}

pub const S2_MODULATION: Modulation = Modulation::Qpsk;
pub const S2_RATE: Rate = Rate::R3_4;

#[must_use]
pub fn dvbs2(seconds: usize) -> Vec<Complex<f32>> {
    dvbs2_mode(seconds, S2_MODULATION, S2_RATE, true, false)
}

#[must_use]
pub fn dvbs2_mode(
    seconds: usize,
    modulation: Modulation,
    rate: Rate,
    short: bool,
    pilots: bool,
) -> Vec<Complex<f32>> {
    let wanted = seconds * SYMBOL_RATE as usize;
    let modcod = ModCod::find(modulation, rate).expect("a catalogued mode");
    let mut encoder = Dvbs2Encoder::new(modcod, short, pilots).expect("a supported mode");
    let mut multiplex = Multiplex::new();
    let mut symbols = Vec::with_capacity(wanted);
    while symbols.len() < wanted {
        let packets: Vec<[u8; PACKET]> = (0..encoder.capacity())
            .map(|_| multiplex.packet())
            .collect();
        encoder.frame(&packets, &mut symbols);
    }
    shape(&symbols)
}

fn shape(symbols: &[Complex<f32>]) -> Vec<Complex<f32>> {
    let taps = pulse::root_raised_cosine(SPS as f64, ROLL_OFF, PULSE_SPAN, Norm::Energy);
    let mut upsampled = Vec::with_capacity(symbols.len() * SPS);
    for &symbol in symbols {
        upsampled.push(symbol);
        upsampled.extend(std::iter::repeat_n(Complex::new(0.0, 0.0), SPS - 1));
    }
    let mut shaped = Vec::with_capacity(upsampled.len());
    for index in 0..upsampled.len() {
        let mut sum = Complex::new(0.0f32, 0.0);
        for (offset, &tap) in taps.iter().enumerate() {
            if let Some(&value) = upsampled.get(index.wrapping_sub(offset)) {
                sum += value * tap;
            }
        }
        shaped.push(sum);
    }
    let mut resampler = FracResampler::new(INPUT_RATE_HZ / (SPS as f64 * SYMBOL_RATE));
    let mut out = Vec::new();
    resampler.process(&shaped, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datv::ts::TsDemux;

    #[test]
    fn the_generated_transport_stream_carries_a_named_program() {
        let mut demux = TsDemux::new();
        let mut units = Vec::new();
        for packet in transport(120) {
            demux.push(&packet, &mut units);
        }
        let program = demux.program().expect("a program");
        assert_eq!(program.number, crate::datv::ts::PROGRAM);
        assert_eq!(program.name.as_deref(), Some(PROGRAM_NAME));
        assert_eq!(program.provider.as_deref(), Some(PROVIDER));
        assert_eq!(program.streams.len(), 2);
        assert!(units.len() >= 2);
    }

    #[test]
    fn the_shaped_waveform_runs_at_the_channel_rate() {
        let iq = dvbs(1);
        let expected = INPUT_RATE_HZ as usize;
        assert!(
            iq.len().abs_diff(expected) < expected / 8,
            "{} samples for one second",
            iq.len()
        );
        let power: f32 =
            iq.iter().map(num_complex::Complex::norm_sqr).sum::<f32>() / iq.len() as f32;
        assert!(power > 0.05, "the waveform carries no power");
    }
}
