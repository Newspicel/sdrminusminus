#![allow(clippy::expect_used)]

use num_complex::Complex;
use sdrmm_dsp::FracResampler;
use sdrmm_modem::pulse::{self, Norm};
use sdrmm_wire::{DatvCodeRate, DatvParams, DatvStandard};

use crate::datv::{
    dvbs::{DvbsEncoder, PACKET},
    dvbs2::{
        frame::{ModCod, Modulation},
        gse::{GsePdu, GseWriter},
        ldpc::Rate,
        receiver::Dvbs2Encoder,
        vlsnr::{VlMode, VlSnrEncoder},
    },
    ts::{TsWriter, pat, pmt, sdt},
};

pub const SYMBOL_RATE: f64 = 250_000.0;
pub const PROGRAM_NAME: &str = "Rust TV";
pub const PROVIDER: &str = "SDR--";
pub const CODE_RATE: DatvCodeRate = DatvCodeRate::ThreeQuarters;

#[must_use]
pub fn params() -> DatvParams {
    DatvParams {
        standard: DatvStandard::DvbS,
        symbol_rate: SYMBOL_RATE,
        code_rate: CODE_RATE,
        ..DatvParams::default()
    }
}

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
    tabled: usize,
    frame: u32,
}

impl Multiplex {
    #[must_use]
    pub fn new() -> Self {
        Self {
            writer: TsWriter::new(),
            queued: std::collections::VecDeque::new(),
            emitted: 0,
            tabled: 0,
            frame: 0,
        }
    }

    pub fn packet(&mut self) -> [u8; PACKET] {
        if self.queued.is_empty() {
            let mut batch = Vec::new();
            let pts = 90_000 + u64::from(self.frame) * 43_200;
            let video = include_bytes!("../../../../fixtures/broadcast_audio/pattern.m2v");
            let mut starts = Vec::new();
            let mut presentation = Vec::new();
            let mut gop = 0u64;
            for (at, header) in video.windows(8).enumerate() {
                if header[..4] == [0, 0, 1, 0xb8] {
                    let code = u32::from_be_bytes(header[4..8].try_into().expect("GOP time code"));
                    gop = u64::from((code >> 7) & 63);
                } else if header[..4] == [0, 0, 1, 0] {
                    starts.push(if starts.is_empty() { 0 } else { at });
                    let temporal = u64::from(u16::from_be_bytes([header[4], header[5]]) >> 6);
                    presentation.push((gop + temporal) * 3600);
                }
            }
            starts.push(video.len());
            let audio = include_bytes!("../../../../fixtures/broadcast_audio/tone_48k_mono.mp2");
            let mut audio_frame = 0;
            for (index, span) in starts.windows(2).enumerate() {
                self.writer.pes(
                    0x0101,
                    0xe0,
                    pts + presentation[index],
                    &video[span[0]..span[1]],
                    &mut batch,
                );
                while audio_frame < 20 && audio_frame * 2160 < (index + 1) * 3600 {
                    self.writer.pes(
                        0x0102,
                        0xc0,
                        pts + audio_frame as u64 * 2160,
                        &audio[audio_frame * 192..(audio_frame + 1) * 192],
                        &mut batch,
                    );
                    audio_frame += 1;
                }
            }
            self.frame += 1;
            self.queued.extend(batch);
        }
        if self.emitted == 0 || self.emitted >= self.tabled + TABLE_PERIOD {
            self.tabled = self.emitted;
            let mut tables = Vec::new();
            self.writer.section(0x0000, &pat(), &mut tables);
            self.writer.section(0x0100, &pmt(), &mut tables);
            self.writer
                .section(0x0011, &sdt(PROVIDER, PROGRAM_NAME), &mut tables);
            for packet in tables.into_iter().rev() {
                self.queued.push_front(packet);
            }
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
    dvbs_with(seconds, &params())
}

#[must_use]
pub fn dvbs_with(seconds: usize, p: &DatvParams) -> Vec<Complex<f32>> {
    let wanted = seconds * p.symbol_rate as usize;
    let mut encoder = DvbsEncoder::new(p.code_rate);
    let mut multiplex = Multiplex::new();
    let mut symbols = Vec::with_capacity(wanted);
    while symbols.len() < wanted {
        encoder.packet(&multiplex.packet(), &mut symbols);
    }
    shape_with(&symbols, p)
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
    let modcod = ModCod::find_for_frame(modulation, rate, short).expect("a catalogued mode");
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

#[must_use]
pub fn dvbs2_superframes(seconds: usize) -> Vec<Complex<f32>> {
    use crate::datv::dvbs2::{pl, superframe};
    let count = (seconds * SYMBOL_RATE as usize).div_ceil(superframe::LENGTH);
    let mode = ModCod::from_index(214).expect("256APSK mode");
    let mut encoder = Dvbs2Encoder::new(mode, false, false).expect("256APSK encoder");
    let mut header = Vec::new();
    pl::header(
        pl::Signalling {
            modcod: 214,
            short: false,
            pilots: true,
        },
        &mut header,
    );
    let mut multiplex = Multiplex::new();
    let mut payload = Vec::new();
    while payload.len() < count * superframe::LENGTH {
        let packets: Vec<_> = (0..encoder.capacity())
            .map(|_| multiplex.packet())
            .collect();
        let start = payload.len();
        encoder.frame(&packets, &mut payload);
        payload[start..start + pl::HEADER].copy_from_slice(&header);
    }
    shape(&superframe::wrap(&payload, 0, true, count))
}

#[must_use]
pub fn datagram(protocol: u16, label: &[u8], len: usize, seed: u32) -> GsePdu {
    GsePdu {
        protocol,
        label: label.to_vec(),
        data: elementary(len, seed),
    }
}

#[must_use]
pub fn dvbs2_generic(seconds: usize, streams: &[u8]) -> Vec<Complex<f32>> {
    let wanted = seconds * SYMBOL_RATE as usize;
    let modcod = ModCod::find(Modulation::Apsk16, Rate::R3_4).expect("a catalogued mode");
    let mut encoder = Dvbs2Encoder::new(modcod, false, true).expect("a supported mode");
    let mut writer = GseWriter::new();
    let mut symbols = Vec::with_capacity(wanted);
    let mut round = 0u32;
    while symbols.len() < wanted {
        for &isi in streams {
            let pdu = datagram(0x0800, &[0x02, isi, 0, 0, 0, round as u8], 1_200, round + 1);
            let mut field = Vec::new();
            if round.is_multiple_of(2) {
                writer.fragmented(&pdu, 2, &mut field);
            } else {
                GseWriter::whole(&pdu, &mut field);
            }
            GseWriter::pad(&mut field, encoder.field_bytes());
            encoder.generic(&field, Some(isi), &mut symbols);
        }
        round += 1;
    }
    shape(&symbols)
}

#[must_use]
pub fn dvbs2_very_low(seconds: usize, header: u8) -> Vec<Complex<f32>> {
    let wanted = seconds * SYMBOL_RATE as usize;
    let mode = VlMode::from_header(header).expect("a catalogued VL-SNR mode");
    let mut encoder = VlSnrEncoder::new(mode).expect("a supported mode");
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

#[must_use]
pub fn dvbs2_very_low_generic(seconds: usize, header: u8, streams: &[u8]) -> Vec<Complex<f32>> {
    let wanted = seconds * SYMBOL_RATE as usize;
    let mode = VlMode::from_header(header).expect("a catalogued VL-SNR mode");
    let mut encoder = VlSnrEncoder::new(mode).expect("a supported mode");
    let mut symbols = Vec::with_capacity(wanted);
    let mut round = 0u32;
    while symbols.len() < wanted {
        for &isi in streams {
            let pdu = datagram(0x86DD, &[0x02, isi, 0, 0, 0, round as u8], 120, round + 1);
            let mut field = Vec::new();
            GseWriter::whole(&pdu, &mut field);
            GseWriter::pad(&mut field, encoder.field_bytes());
            encoder.generic(&field, Some(isi), &mut symbols);
        }
        round += 1;
    }
    shape(&symbols)
}

fn shape(symbols: &[Complex<f32>]) -> Vec<Complex<f32>> {
    shape_with(symbols, &params())
}

fn shape_with(symbols: &[Complex<f32>], p: &DatvParams) -> Vec<Complex<f32>> {
    let taps = pulse::root_raised_cosine(SPS as f64, p.roll_off.factor(), PULSE_SPAN, Norm::Energy);
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
    let rate = crate::datv::input_rate_hz(p);
    let mut resampler = FracResampler::new(rate / (SPS as f64 * p.symbol_rate));
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
        let expected = crate::datv::input_rate_hz(&params()) as usize;
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
