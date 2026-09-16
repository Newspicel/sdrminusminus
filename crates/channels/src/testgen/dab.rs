#![allow(clippy::expect_used)]

use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use sdrmm_wire::DabTransmissionMode;

use crate::dab::{
    fic::{FIB_BYTES, FicEncoder, append_fib_crc},
    mode::Mode,
    msc::{CIF_BITS, SubChannelEncoder, subchannel_range},
    ofdm::{interleaving_for_mode, map_symbol_for_mode, reference_symbol_for_mode},
    protection::{Eep, Protection},
    superframe::{AudioFormat, SuperframeBuilder},
};

pub const ENSEMBLE_ID: u16 = 0x10CD;
pub const ENSEMBLE_LABEL: &str = "SDR-- test";
pub const MUSIC_SERVICE: u32 = 0xC1A1;
pub const TALK_SERVICE: u32 = 0xC1A2;
pub const MUSIC_BITRATE_KBPS: u16 = 96;
pub const TALK_BITRATE_KBPS: u16 = 64;

const MUSIC_SUBCHANNEL: u8 = 1;
const TALK_SUBCHANNEL: u8 = 2;
const MUSIC_START_CU: u16 = 0;
const MUSIC_SIZE_CU: u16 = 72;
const TALK_START_CU: u16 = 72;
const TALK_SIZE_CU: u16 = 48;

fn label_figure(extension: u8, id: &[u8], text: &str) -> Vec<u8> {
    let mut data = vec![extension];
    data.extend_from_slice(id);
    let mut padded = text.as_bytes().to_vec();
    padded.resize(16, b' ');
    data.extend_from_slice(&padded);
    data.extend_from_slice(&0xFF00u16.to_be_bytes());
    data
}

fn subchannel_figure() -> Vec<u8> {
    let mut data = vec![0x01u8];
    for (id, start, size) in [
        (MUSIC_SUBCHANNEL, MUSIC_START_CU, MUSIC_SIZE_CU),
        (TALK_SUBCHANNEL, TALK_START_CU, TALK_SIZE_CU),
    ] {
        data.push(id << 2 | (start >> 8) as u8);
        data.push(start as u8);
        data.push(0x80 | (3 - 1) << 2 | (size >> 8) as u8);
        data.push(size as u8);
    }
    data
}

fn service_figure(id: u32, subchannel: u8, aac: bool) -> Vec<u8> {
    vec![
        0x02,
        (id >> 8) as u8,
        id as u8,
        0x01,
        if aac { 63 } else { 0 },
        subchannel << 2 | 0x02,
    ]
}

fn ensemble_figure() -> Vec<u8> {
    let mut data = vec![0x00u8];
    data.extend_from_slice(&ENSEMBLE_ID.to_be_bytes());
    data.extend_from_slice(&[0x00, 0x00, 0x00]);
    data
}

fn fib(figures: &[(u8, Vec<u8>)]) -> [u8; FIB_BYTES] {
    let mut body = Vec::new();
    for (kind, data) in figures {
        body.push(kind << 5 | data.len() as u8);
        body.extend_from_slice(data);
    }
    body.resize(FIB_BYTES - 2, 0xFF);
    append_fib_crc(&mut body);
    let mut fib = [0u8; FIB_BYTES];
    fib.copy_from_slice(&body);
    fib
}

fn service_information() -> Vec<[u8; FIB_BYTES]> {
    vec![
        fib(&[(0, ensemble_figure()), (0, subchannel_figure())]),
        fib(&[
            (0, service_figure(MUSIC_SERVICE, MUSIC_SUBCHANNEL, true)),
            (0, service_figure(TALK_SERVICE, TALK_SUBCHANNEL, false)),
        ]),
        fib(&[(
            1,
            label_figure(0, &ENSEMBLE_ID.to_be_bytes(), ENSEMBLE_LABEL),
        )]),
        fib(&[(
            1,
            label_figure(1, &(MUSIC_SERVICE as u16).to_be_bytes(), "Rust FM"),
        )]),
        fib(&[(
            1,
            label_figure(1, &(TALK_SERVICE as u16).to_be_bytes(), "Rust Talk"),
        )]),
    ]
}

fn access_unit(len: usize, seed: u32) -> Vec<u8> {
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

struct Music {
    encoder: SubChannelEncoder,
    builder: SuperframeBuilder,
    format: AudioFormat,
    queued: Vec<Vec<u8>>,
    counter: u32,
}

impl Music {
    fn new() -> Self {
        let protection =
            Protection::eep(MUSIC_BITRATE_KBPS, Eep::A, 3).expect("EEP-A 3 at 96 kbit/s");
        let frame_bytes = protection.frame_bits() / 8;
        Self {
            encoder: SubChannelEncoder::new(protection),
            builder: SuperframeBuilder::new(frame_bytes).expect("96 kbit/s builds superframes"),
            format: AudioFormat {
                sample_rate_hz: 48_000,
                spectral_band_replication: true,
                parametric_stereo: false,
                stereo_core: true,
                surround: 0,
            },
            queued: Vec::new(),
            counter: 0,
        }
    }

    fn logical(&mut self) -> Vec<u8> {
        if self.queued.is_empty() {
            self.counter += 1;
            let units: Vec<Vec<u8>> = (0..self.format.access_units())
                .map(|index| access_unit(180, self.counter * 8 + index as u32))
                .collect();
            self.queued = self
                .builder
                .build(self.format, &units)
                .expect("the access units fit the superframe");
            self.queued.reverse();
        }
        self.queued.pop().unwrap_or_default()
    }
}

struct Talk {
    encoder: SubChannelEncoder,
    frame_bytes: usize,
    counter: u32,
}

impl Talk {
    fn new() -> Self {
        let protection =
            Protection::eep(TALK_BITRATE_KBPS, Eep::A, 3).expect("EEP-A 3 at 64 kbit/s");
        let frame_bytes = protection.frame_bits() / 8;
        Self {
            encoder: SubChannelEncoder::new(protection),
            frame_bytes,
            counter: 0,
        }
    }

    fn logical(&mut self) -> Vec<u8> {
        self.counter += 1;
        let fixture = include_bytes!("../../../../fixtures/broadcast_audio/tone_48k_mono.mp2");
        let start = ((self.counter as usize - 1) * self.frame_bytes) % fixture.len();
        fixture[start..start + self.frame_bytes].to_vec()
    }
}

struct Modulator {
    mode: Mode,
    transmission_mode: DabTransmissionMode,
    inverse: Arc<dyn Fft<f32>>,
    bins: Vec<usize>,
    reference: Vec<Complex<f32>>,
}

impl Modulator {
    fn new(transmission_mode: DabTransmissionMode) -> Self {
        let mode = Mode::new(transmission_mode);
        let mut planner = FftPlanner::<f32>::new();
        let bins = interleaving_for_mode(transmission_mode);
        let spectrum = reference_symbol_for_mode(transmission_mode);
        let reference = bins.iter().map(|&bin| spectrum[bin]).collect();
        Self {
            mode,
            transmission_mode,
            inverse: planner.plan_fft_inverse(mode.useful),
            bins,
            reference,
        }
    }

    fn emit(&self, points: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        let mut spectrum = vec![Complex::new(0.0, 0.0); self.mode.useful];
        for (index, &bin) in self.bins.iter().enumerate() {
            spectrum[bin] = points[index];
        }
        let mut time = spectrum;
        self.inverse.process(&mut time);
        let scale = 1.0 / (self.mode.useful as f32).sqrt();
        out.extend(
            time[self.mode.useful - self.mode.guard..]
                .iter()
                .map(|&value| value * scale),
        );
        out.extend(time.iter().map(|&value| value * scale));
    }

    fn frame(&self, symbols: &[Vec<bool>], out: &mut Vec<Complex<f32>>) {
        out.extend(std::iter::repeat_n(Complex::new(0.0, 0.0), self.mode.null));
        let mut previous = self.reference.clone();
        self.emit(&previous, out);
        for bits in symbols {
            let mapped = map_symbol_for_mode(self.transmission_mode, bits);
            let points: Vec<Complex<f32>> = mapped
                .iter()
                .zip(&previous)
                .map(|(&point, &reference)| point * reference)
                .collect();
            self.emit(&points, out);
            previous = points;
        }
    }
}

fn place(cif: &mut [bool], start_cu: u16, size_cu: u16, fragment: &[bool]) {
    if let Some((low, high)) = subchannel_range(start_cu, size_cu)
        && high - low == fragment.len()
    {
        cif[low..high].copy_from_slice(fragment);
    }
}

#[must_use]
pub fn ensemble(frames: usize) -> Vec<Complex<f32>> {
    ensemble_for_mode(DabTransmissionMode::I, frames)
}

#[must_use]
pub fn ensemble_for_mode(
    transmission_mode: DabTransmissionMode,
    frames: usize,
) -> Vec<Complex<f32>> {
    let mode = Mode::new(transmission_mode);
    let modulator = Modulator::new(transmission_mode);
    let mut fic = FicEncoder::for_mode(transmission_mode);
    let mut music = Music::new();
    let mut talk = Talk::new();
    let information = service_information();
    let mut fib_at = 0usize;
    let mut iq = Vec::with_capacity(frames * mode.frame());
    let mut fragment = Vec::new();
    for _ in 0..frames {
        let mut bits = Vec::with_capacity(mode.symbols * mode.symbol_bits());
        for _ in 0..mode.cifs {
            let mut group = vec![[0u8; FIB_BYTES]; mode.fibs_per_block];
            for slot in &mut group {
                *slot = information[fib_at % information.len()];
                fib_at += 1;
            }
            fic.block(&group, &mut bits);
        }
        for _ in 0..mode.cifs {
            let mut cif = vec![false; CIF_BITS];
            let payload = music.logical();
            music.encoder.frame(&payload, &mut fragment);
            place(&mut cif, MUSIC_START_CU, MUSIC_SIZE_CU, &fragment);
            let payload = talk.logical();
            talk.encoder.frame(&payload, &mut fragment);
            place(&mut cif, TALK_START_CU, TALK_SIZE_CU, &fragment);
            bits.extend_from_slice(&cif);
        }
        let symbols: Vec<Vec<bool>> = bits
            .chunks(mode.symbol_bits())
            .map(<[bool]>::to_vec)
            .collect();
        modulator.frame(&symbols, &mut iq);
    }
    iq
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dab::ofdm::{FRAME, NULL, SYMBOL, SYMBOL_BITS, interleaving};

    #[test]
    fn a_generated_frame_has_the_documented_length_and_a_null_symbol() {
        let iq = ensemble(2);
        assert_eq!(iq.len(), 2 * FRAME);
        assert!(iq[..NULL].iter().all(|value| value.norm() < 1e-9));
        let power: f32 = iq[NULL..NULL + SYMBOL]
            .iter()
            .map(num_complex::Complex::norm_sqr)
            .sum();
        assert!(power > 0.1, "the phase reference symbol carries no power");
    }

    #[test]
    fn the_carrier_count_matches_the_transmission_mode() {
        assert_eq!(interleaving().len(), crate::dab::ofdm::CARRIERS);
        assert_eq!(SYMBOL_BITS, 2 * crate::dab::ofdm::CARRIERS);
    }
}
