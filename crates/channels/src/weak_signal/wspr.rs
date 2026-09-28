mod fano;
mod message;

#[cfg(test)]
mod comparison;

use std::{f32::consts::TAU, sync::Arc};

use num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use rustfft::{Fft, FftPlanner};

use fano::{CODED_BITS, SYNC};
use message::CallBook;
pub(crate) use message::Message;

pub(crate) const SAMPLE_RATE: f32 = 12_000.0;
pub(crate) const SLOT_SAMPLES: usize = 1_440_000;
const SYMBOL_SAMPLES: usize = 8_192;
const TONE_HZ: f32 = SAMPLE_RATE / SYMBOL_SAMPLES as f32;
const DECIMATION: usize = 32;
const BASEBAND: usize = SLOT_SAMPLES / DECIMATION;
const BASEBAND_RATE: f32 = SAMPLE_RATE / DECIMATION as f32;
const BASEBAND_SYMBOL: usize = SYMBOL_SAMPLES / DECIMATION;
const CHUNK_REACH_HZ: f32 = 150.0;
const FRAME_STEP: usize = BASEBAND_SYMBOL / 4;
const FRAME_FFT: usize = 2 * BASEBAND_SYMBOL;
const HALF_TONE_HZ: f32 = BASEBAND_RATE / FRAME_FFT as f32;
const SIGNAL_BINS: usize = 7;
const NOISE_PERCENTILE: usize = 30;
const MIN_PRESENCE: f32 = 1.15;
const COARSE_DRIFTS_HZ: [f32; 5] = [-4.0, -2.0, 0.0, 2.0, 4.0];
const FINE_DECIMATION: usize = 16;
const FINE_RATE: f32 = BASEBAND_RATE / FINE_DECIMATION as f32;
const FINE_SYMBOL: usize = BASEBAND_SYMBOL / FINE_DECIMATION;
const FINE_SAMPLES: usize = BASEBAND / FINE_DECIMATION;
const MIN_SYNC: f32 = 0.13;
const MIN_ROUGH_SYNC: f32 = 0.13;
const RETRY_SYNC: f32 = 0.16;
const RESIDUAL_TONES: f32 = 2.0;
const NEIGHBOUR_TONES: f32 = 4.0;
const FANO_DELTA: f32 = 1.0;
const FANO_CYCLES: [usize; 3] = [400_000, 150_000, 100_000];
const PASSES: usize = 4;
const SUBTRACT_SYMBOLS: usize = 4;
const LLR_LIMIT: f32 = 5.0;
const BLOCKS: [usize; 3] = [4, 6, 3];
const MAX_BLOCK: usize = 6;
const COHERENCE_BLOCK: usize = 6;
struct Round {
    coherent: bool,
    frequency: (i32, f32),
    drift: (i32, f32),
    time: (isize, isize),
}

const ROUNDS: [Round; 3] = [
    Round {
        coherent: false,
        frequency: (8, 0.05),
        drift: (0, 0.0),
        time: (8, 1),
    },
    Round {
        coherent: true,
        frequency: (4, 0.05),
        drift: (2, 0.25),
        time: (8, 1),
    },
    Round {
        coherent: true,
        frequency: (2, 0.025),
        drift: (1, 0.125),
        time: (2, 1),
    },
];

const RETRY_ROUNDS: [Round; 2] = [
    Round {
        coherent: true,
        frequency: (6, 0.05),
        drift: (2, 0.25),
        time: (8, 1),
    },
    Round {
        coherent: true,
        frequency: (3, 0.02),
        drift: (2, 0.125),
        time: (2, 1),
    },
];

pub(crate) struct Spot {
    pub(crate) message: Message,
    pub(crate) snr_db: f32,
    pub(crate) frequency_hz: f32,
    pub(crate) start_s: f32,
    pub(crate) drift_hz: f32,
}

#[derive(Clone, Copy, Debug)]
struct Track {
    start: isize,
    offset_hz: f32,
    drift_hz: f32,
}

type Powers = [[f32; 4]; CODED_BITS];
type Amplitudes = [[Complex<f32>; 4]; CODED_BITS];

pub(crate) struct WsprDecoder {
    forward: Arc<dyn RealToComplex<f32>>,
    forward_input: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    forward_scratch: Vec<Complex<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    inverse_scratch: Vec<Complex<f32>>,
    frame_fft: Arc<dyn Fft<f32>>,
    baseband: Vec<Complex<f32>>,
    fine: Vec<Complex<f32>>,
    power: Vec<f32>,
    twiddle_re: [[f32; FINE_SYMBOL]; 4],
    twiddle_im: [[f32; FINE_SYMBOL]; 4],
    book: CallBook,
}

impl WsprDecoder {
    pub(crate) fn new() -> Self {
        let forward = RealFftPlanner::<f32>::new().plan_fft_forward(SLOT_SAMPLES);
        let mut planner = FftPlanner::<f32>::new();
        let inverse = planner.plan_fft_inverse(BASEBAND);
        Self {
            forward_input: forward.make_input_vec(),
            spectrum: forward.make_output_vec(),
            forward_scratch: forward.make_scratch_vec(),
            forward,
            inverse_scratch: vec![Complex::default(); inverse.get_inplace_scratch_len()],
            inverse,
            frame_fft: planner.plan_fft_forward(FRAME_FFT),
            baseband: vec![Complex::default(); BASEBAND],
            fine: vec![Complex::default(); FINE_SAMPLES],
            power: Vec::new(),
            twiddle_re: std::array::from_fn(|tone| {
                std::array::from_fn(|n| {
                    (TAU * (tone as f32 - 1.5) * n as f32 / FINE_SYMBOL as f32).cos()
                })
            }),
            twiddle_im: std::array::from_fn(|tone| {
                std::array::from_fn(|n| {
                    -(TAU * (tone as f32 - 1.5) * n as f32 / FINE_SYMBOL as f32).sin()
                })
            }),
            book: CallBook::default(),
        }
    }

    pub(crate) fn decode(
        &mut self,
        samples: &[f32],
        low_hz: f32,
        high_hz: f32,
        max_candidates: usize,
    ) -> Vec<Spot> {
        self.transform(samples);
        let chunks = ((high_hz - low_hz) / (2.0 * CHUNK_REACH_HZ))
            .ceil()
            .max(1.0) as usize;
        let width = (high_hz - low_hz) / chunks as f32;
        let mut spots: Vec<Spot> = Vec::new();
        for chunk in 0..chunks {
            let centre = low_hz + width * (chunk as f32 + 0.5);
            self.downconvert(centre);
            let band = (low_hz - centre, high_hz - centre);
            for _ in 0..PASSES {
                let fresh = self.pass(centre, band, max_candidates, &spots);
                if fresh.is_empty() {
                    break;
                }
                spots.extend(fresh);
            }
        }
        spots
    }

    fn pass(
        &mut self,
        centre: f32,
        band: (f32, f32),
        max_candidates: usize,
        known: &[Spot],
    ) -> Vec<Spot> {
        let frames = self.spectrogram();
        let mut fresh: Vec<Spot> = Vec::new();
        for offset_hz in self.candidates(frames, band, max_candidates) {
            let near = |spots: &[Spot], reach: f32| {
                spots
                    .iter()
                    .any(|spot| (spot.frequency_hz - centre - offset_hz).abs() < reach * TONE_HZ)
            };
            if near(known, RESIDUAL_TONES) || near(&fresh, NEIGHBOUR_TONES) {
                continue;
            }
            let Some(track) = self.coarse(frames, offset_hz) else {
                continue;
            };
            let Some((spot, tones, track)) = self.attempt(centre, track) else {
                continue;
            };
            let duplicate = known.iter().chain(&fresh).any(|other| {
                other.message.text == spot.message.text
                    && (other.frequency_hz - spot.frequency_hz).abs() < 2.0 * TONE_HZ
            });
            if !duplicate {
                self.subtract(&tones, track);
                fresh.push(spot);
            }
        }
        fresh
    }

    fn transform(&mut self, samples: &[f32]) {
        self.forward_input.fill(0.0);
        let length = samples.len().min(SLOT_SAMPLES);
        self.forward_input[..length].copy_from_slice(&samples[..length]);
        if self
            .forward
            .process_with_scratch(
                &mut self.forward_input,
                &mut self.spectrum,
                &mut self.forward_scratch,
            )
            .is_err()
        {
            self.spectrum.fill(Complex::default());
        }
    }

    fn downconvert(&mut self, centre_hz: f32) {
        let resolution = SAMPLE_RATE / SLOT_SAMPLES as f32;
        let middle = (centre_hz / resolution).round() as isize;
        let half = (BASEBAND / 2) as isize;
        self.baseband.fill(Complex::default());
        for offset in -half..half {
            if let Some(&bin) = usize::try_from(middle + offset)
                .ok()
                .and_then(|index| self.spectrum.get(index))
            {
                self.baseband[offset.rem_euclid(BASEBAND as isize) as usize] = bin;
            }
        }
        self.inverse
            .process_with_scratch(&mut self.baseband, &mut self.inverse_scratch);
    }

    fn spectrogram(&mut self) -> usize {
        let frames = (BASEBAND - BASEBAND_SYMBOL) / FRAME_STEP + 1;
        self.power.clear();
        self.power.resize(frames * FRAME_FFT, 0.0);
        let mut buffer = vec![Complex::default(); FRAME_FFT];
        for frame in 0..frames {
            buffer.fill(Complex::default());
            buffer[..BASEBAND_SYMBOL]
                .copy_from_slice(&self.baseband[frame * FRAME_STEP..][..BASEBAND_SYMBOL]);
            self.frame_fft.process(&mut buffer);
            let row = &mut self.power[frame * FRAME_FFT..][..FRAME_FFT];
            for (index, value) in buffer.iter().enumerate() {
                row[(index + FRAME_FFT / 2) % FRAME_FFT] = value.norm_sqr();
            }
        }
        frames
    }

    fn bin_hz(bin: usize) -> f32 {
        (bin as f32 - (FRAME_FFT / 2) as f32) * HALF_TONE_HZ
    }

    fn candidates(&self, frames: usize, band: (f32, f32), limit: usize) -> Vec<f32> {
        let mut average = vec![0.0f32; FRAME_FFT];
        for frame in 0..frames {
            for (sum, power) in average.iter_mut().zip(&self.power[frame * FRAME_FFT..]) {
                *sum += power;
            }
        }
        let reach = (CHUNK_REACH_HZ / HALF_TONE_HZ) as usize;
        let first = FRAME_FFT / 2 - reach;
        let last = FRAME_FFT / 2 + reach;
        let energy: Vec<f32> = (first..last)
            .map(|centre| {
                average[centre - SIGNAL_BINS / 2..=centre + SIGNAL_BINS / 2]
                    .iter()
                    .sum()
            })
            .collect();
        let mut sorted = energy.clone();
        sorted.sort_unstable_by(f32::total_cmp);
        let Some(&floor) = sorted.get(sorted.len() * NOISE_PERCENTILE / 100) else {
            return Vec::new();
        };
        if floor <= 0.0 {
            return Vec::new();
        }
        let mut peaks: Vec<(f32, f32)> = (1..energy.len() - 1)
            .filter(|&index| {
                energy[index] >= energy[index - 1]
                    && energy[index] > energy[index + 1]
                    && energy[index] / floor >= MIN_PRESENCE
            })
            .map(|index| (Self::bin_hz(first + index), energy[index] / floor))
            .filter(|&(hz, _)| (band.0..=band.1).contains(&hz))
            .collect();
        peaks.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
        peaks.truncate(limit);
        peaks.into_iter().map(|(hz, _)| hz).collect()
    }

    fn coarse(&self, frames: usize, offset_hz: f32) -> Option<Track> {
        let centre_bin = (offset_hz / HALF_TONE_HZ).round() as isize + (FRAME_FFT / 2) as isize;
        let starts = frames.saturating_sub(4 * (CODED_BITS - 1) + 1);
        let mut best: Option<(f32, Track)> = None;
        for drift_hz in COARSE_DRIFTS_HZ {
            for shift in -2..=2 {
                let bin = centre_bin + shift;
                for start in 0..starts {
                    let score = self.coarse_score(start, bin, drift_hz);
                    if best.is_none_or(|(known, _)| score > known) {
                        best = Some((
                            score,
                            Track {
                                start: (start * FRAME_STEP) as isize,
                                offset_hz: (bin - (FRAME_FFT / 2) as isize) as f32 * HALF_TONE_HZ,
                                drift_hz,
                            },
                        ));
                    }
                }
            }
        }
        best.filter(|&(score, _)| score > 0.0)
            .map(|(_, track)| track)
    }

    fn coarse_score(&self, start: usize, centre_bin: isize, drift_hz: f32) -> f32 {
        let (mut aligned, mut total) = (0.0f32, 0.0f32);
        for (symbol, &sync) in SYNC.iter().enumerate() {
            let drift = drift_hz * (symbol as f32 - 81.0) / 162.0 / HALF_TONE_HZ;
            let base = centre_bin + drift.round() as isize - 3;
            if base < 0 || base as usize + 6 >= FRAME_FFT {
                return 0.0;
            }
            let row = &self.power[(start + 4 * symbol) * FRAME_FFT..];
            let tone = |index: usize| row[base as usize + 2 * index];
            let (even, odd) = (tone(0) + tone(2), tone(1) + tone(3));
            let sign = if sync == b'1' { 1.0 } else { -1.0 };
            aligned += sign * (odd - even);
            total += even + odd;
        }
        if total > 0.0 { aligned / total } else { 0.0 }
    }

    fn attempt(&mut self, centre: f32, coarse: Track) -> Option<(Spot, [u8; CODED_BITS], Track)> {
        self.decimate(coarse.offset_hz);
        let mut track = self.refine(
            Track {
                start: coarse.start / FINE_DECIMATION as isize,
                offset_hz: 0.0,
                drift_hz: coarse.drift_hz,
            },
            &ROUNDS,
        )?;
        let sync = sync_score(&self.powers(track));
        if sync < MIN_SYNC {
            return None;
        }
        let mut decoded = self.decode_track(track);
        if decoded.is_none() && sync >= RETRY_SYNC {
            track = self.refine(track, &RETRY_ROUNDS)?;
            decoded = self.decode_track(track);
        }
        let (decoded, snr) = decoded?;
        let message = message::unpack(decoded, &mut self.book)?;
        let tones = fano::tones(decoded);
        let absolute = Track {
            start: track.start * FINE_DECIMATION as isize,
            offset_hz: coarse.offset_hz + track.offset_hz,
            drift_hz: track.drift_hz,
        };
        Some((
            Spot {
                message,
                snr_db: 10.0 * snr.max(1e-3).log10() + 10.0 * (TONE_HZ / 2_500.0).log10(),
                frequency_hz: centre + absolute.offset_hz,
                start_s: absolute.start as f32 * DECIMATION as f32 / SAMPLE_RATE,
                drift_hz: absolute.drift_hz,
            },
            tones,
            absolute,
        ))
    }

    fn decimate(&mut self, offset_hz: f32) {
        let step = Complex::from_polar(1.0, -TAU * offset_hz / BASEBAND_RATE);
        let mut mixed = Vec::with_capacity(BASEBAND);
        let mut phase = Complex::new(1.0f32, 0.0);
        for (index, sample) in self.baseband.iter().enumerate() {
            if index % 1_024 == 0 {
                phase = Complex::from_polar(1.0, -TAU * offset_hz * index as f32 / BASEBAND_RATE);
            }
            mixed.push(sample * phase);
            phase *= step;
        }
        let mut prefix = Vec::with_capacity(BASEBAND + 1);
        let mut running = Complex::<f32>::default();
        prefix.push(running);
        for value in &mixed {
            running += value;
            prefix.push(running);
        }
        let boxed = |at: isize| {
            let low = (at - FINE_DECIMATION as isize / 2).clamp(0, BASEBAND as isize) as usize;
            let high = (at + FINE_DECIMATION as isize / 2).clamp(0, BASEBAND as isize) as usize;
            prefix[high] - prefix[low]
        };
        for (index, out) in self.fine.iter_mut().enumerate() {
            let centre = (index * FINE_DECIMATION) as isize;
            *out = (-(FINE_DECIMATION as isize) / 2..FINE_DECIMATION as isize / 2)
                .map(|shift| boxed(centre + shift))
                .sum();
        }
    }

    fn powers(&self, track: Track) -> Powers {
        self.amplitudes(track)
            .map(|row| row.map(|amplitude| amplitude.norm_sqr()))
    }

    fn amplitudes(&self, track: Track) -> Amplitudes {
        let mut amplitudes = [[Complex::default(); 4]; CODED_BITS];
        let mut alignment = Complex::new(1.0f32, 0.0);
        let (mut real, mut imaginary) = ([0.0f32; FINE_SYMBOL], [0.0f32; FINE_SYMBOL]);
        for (symbol, row) in amplitudes.iter_mut().enumerate() {
            let offset = track.offset_hz + track.drift_hz * (symbol as f32 - 81.0) / 162.0;
            let begin = track.start + (symbol * FINE_SYMBOL) as isize;
            let step = Complex::from_polar(1.0, -TAU * offset / FINE_RATE);
            let mut rotation = Complex::new(1.0f32, 0.0);
            for n in 0..FINE_SYMBOL {
                let sample = self.fine_at(begin + n as isize) * rotation;
                real[n] = sample.re;
                imaginary[n] = sample.im;
                rotation *= step;
            }
            for (amplitude, (twiddle_re, twiddle_im)) in row
                .iter_mut()
                .zip(self.twiddle_re.iter().zip(&self.twiddle_im))
            {
                let (mut sum_re, mut sum_im) = (0.0f32, 0.0f32);
                for n in 0..FINE_SYMBOL {
                    sum_re += real[n] * twiddle_re[n] - imaginary[n] * twiddle_im[n];
                    sum_im += real[n] * twiddle_im[n] + imaginary[n] * twiddle_re[n];
                }
                *amplitude = Complex::new(sum_re, sum_im) * alignment;
            }
            alignment *= -Complex::from_polar(1.0, -TAU * offset / TONE_HZ);
        }
        amplitudes
    }

    fn fine_at(&self, index: isize) -> Complex<f32> {
        match usize::try_from(index) {
            Ok(index) if index < FINE_SAMPLES => self.fine[index],
            _ => self.fine[index.rem_euclid(FINE_SAMPLES as isize) as usize],
        }
    }

    fn decode_track(&self, track: Track) -> Option<(u64, f32)> {
        let (energy, noise) = signal_to_noise(&self.powers(track));
        let amplitudes = self.amplitudes(track);
        let scale = 2.0 * energy.sqrt() / noise;
        BLOCKS
            .iter()
            .zip(FANO_CYCLES)
            .find_map(|(&block, cycles)| {
                fano::decode(&soft_metrics(&amplitudes, block, scale), FANO_DELTA, cycles)
            })
            .map(|decoded| (decoded, energy / noise))
    }

    fn refine(&self, mut track: Track, rounds: &[Round]) -> Option<Track> {
        for round in rounds {
            let score = if round.coherent {
                coherence
            } else {
                incoherence
            };
            let base = track;
            track = best_of(
                self,
                score,
                track,
                (-round.frequency.0..=round.frequency.0).flat_map(|step| {
                    (-round.drift.0..=round.drift.0).map(move |drift| Track {
                        offset_hz: base.offset_hz + step as f32 * round.frequency.1,
                        drift_hz: base.drift_hz + drift as f32 * round.drift.1,
                        ..base
                    })
                }),
            );
            track = best_of(
                self,
                score,
                track,
                (-round.time.0..=round.time.0).map(|step| Track {
                    start: track.start + step * round.time.1,
                    ..track
                }),
            );
            if !round.coherent {
                track = best_of(
                    self,
                    score,
                    track,
                    (-4..=4).map(|step| Track {
                        drift_hz: track.drift_hz + step as f32 * 0.25,
                        ..track
                    }),
                );
                if incoherence(&self.amplitudes(track)) < MIN_ROUGH_SYNC {
                    return None;
                }
            }
        }
        Some(track)
    }

    fn subtract(&mut self, tones: &[u8; CODED_BITS], track: Track) {
        let reference = reference(tones, track);
        let mut envelope: Vec<Complex<f32>> = reference
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let at = (track.start + index as isize).rem_euclid(BASEBAND as isize) as usize;
                self.baseband[at] * value.conj()
            })
            .collect();
        let half = SUBTRACT_SYMBOLS * BASEBAND_SYMBOL / 2;
        envelope = box_average(&box_average(&envelope, half), half);
        for (index, (value, amplitude)) in reference.iter().zip(&envelope).enumerate() {
            let at = (track.start + index as isize).rem_euclid(BASEBAND as isize) as usize;
            self.baseband[at] -= amplitude * value;
        }
    }
}

fn best_of(
    decoder: &WsprDecoder,
    score: fn(&Amplitudes) -> f32,
    current: Track,
    options: impl Iterator<Item = Track>,
) -> Track {
    let mut best = (current, score(&decoder.amplitudes(current)));
    for track in options {
        let score = score(&decoder.amplitudes(track));
        if score > best.1 {
            best = (track, score);
        }
    }
    best.0
}

fn coherence(amplitudes: &Amplitudes) -> f32 {
    let mut total = 0.0;
    for first in (0..CODED_BITS).step_by(COHERENCE_BLOCK) {
        let mut best = 0.0f32;
        for combo in 0..1usize << COHERENCE_BLOCK {
            let sum: Complex<f32> = (0..COHERENCE_BLOCK)
                .map(|index| {
                    let symbol = first + index;
                    let bit = (combo >> index) & 1;
                    amplitudes[symbol][fano::sync_bit(symbol) + 2 * bit]
                })
                .sum();
            best = best.max(sum.norm_sqr());
        }
        total += best;
    }
    total
}

fn incoherence(amplitudes: &Amplitudes) -> f32 {
    sync_score(&amplitudes.map(|row| row.map(|amplitude| amplitude.norm_sqr())))
}

fn sync_score(powers: &Powers) -> f32 {
    let (mut aligned, mut total) = (0.0f32, 0.0f32);
    for (symbol, row) in powers.iter().enumerate() {
        let (even, odd) = (row[0] + row[2], row[1] + row[3]);
        let sign = if SYNC[symbol] == b'1' { 1.0 } else { -1.0 };
        aligned += sign * (odd - even);
        total += even + odd;
    }
    if total > 0.0 { aligned / total } else { 0.0 }
}

fn signal_to_noise(powers: &Powers) -> (f32, f32) {
    let (mut noise, mut peak) = (0.0f32, 0.0f32);
    for (symbol, row) in powers.iter().enumerate() {
        let sync = fano::sync_bit(symbol);
        noise += row[1 - sync] + row[3 - sync];
        peak += row[sync].max(row[sync + 2]);
    }
    let noise = (noise / (2.0 * CODED_BITS as f32)).max(f32::MIN_POSITIVE);
    let energy = (peak / CODED_BITS as f32 - noise).max(0.0);
    (energy, noise)
}

fn soft_metrics(amplitudes: &Amplitudes, block: usize, scale: f32) -> [[f32; 2]; CODED_BITS] {
    let mut llrs = [0.0f32; CODED_BITS];
    for first in (0..CODED_BITS).step_by(block) {
        let size = block.min(CODED_BITS - first);
        let mut best = [[0.0f32; 2]; MAX_BLOCK];
        for combo in 0..1usize << size {
            let sum: Complex<f32> = (0..size)
                .map(|index| {
                    let symbol = first + index;
                    let bit = (combo >> (size - 1 - index)) & 1;
                    amplitudes[symbol][fano::sync_bit(symbol) + 2 * bit]
                })
                .sum();
            let metric = sum.norm_sqr();
            for (index, slot) in best.iter_mut().take(size).enumerate() {
                let bit = (combo >> (size - 1 - index)) & 1;
                slot[bit] = slot[bit].max(metric);
            }
        }
        for (index, slot) in best.iter().take(size).enumerate() {
            llrs[first + index] = scale * (slot[1].sqrt() - slot[0].sqrt());
        }
    }
    let positions = fano::interleaved_positions();
    std::array::from_fn(|bit| fano::bit_metrics(llrs[positions[bit]].clamp(-LLR_LIMIT, LLR_LIMIT)))
}

fn reference(tones: &[u8; CODED_BITS], track: Track) -> Vec<Complex<f32>> {
    let mut wave = Vec::with_capacity(CODED_BITS * BASEBAND_SYMBOL);
    let mut phase = 0.0f64;
    for (symbol, &tone) in tones.iter().enumerate() {
        let frequency = f64::from(
            track.offset_hz
                + track.drift_hz * (symbol as f32 - 81.0) / 162.0
                + (f32::from(tone) - 1.5) * TONE_HZ,
        );
        let step = std::f64::consts::TAU * frequency / f64::from(BASEBAND_RATE);
        for _ in 0..BASEBAND_SYMBOL {
            wave.push(Complex::from_polar(1.0, phase as f32));
            phase = (phase + step).rem_euclid(std::f64::consts::TAU);
        }
    }
    wave
}

fn box_average(input: &[Complex<f32>], half: usize) -> Vec<Complex<f32>> {
    let mut prefix = Vec::with_capacity(input.len() + 1);
    let mut running = Complex::<f64>::default();
    prefix.push(running);
    for value in input {
        running += Complex::new(f64::from(value.re), f64::from(value.im));
        prefix.push(running);
    }
    (0..input.len())
        .map(|index| {
            let low = index.saturating_sub(half);
            let high = (index + half + 1).min(input.len());
            let mean = (prefix[high] - prefix[low]) / (high - low) as f64;
            Complex::new(mean.re as f32, mean.im as f32)
        })
        .collect()
}

#[cfg(any(test, feature = "test-signals"))]
pub(crate) fn waveform(text: &str, audio_hz: f32) -> Option<Vec<f32>> {
    let tones = fano::tones(message::pack(text)?);
    let mut wave = Vec::with_capacity(CODED_BITS * SYMBOL_SAMPLES);
    let mut phase = 0.0f64;
    for &tone in &tones {
        let step = std::f64::consts::TAU * f64::from(audio_hz + (f32::from(tone) - 1.5) * TONE_HZ)
            / f64::from(SAMPLE_RATE);
        for _ in 0..SYMBOL_SAMPLES {
            wave.push(phase.sin() as f32);
            phase = (phase + step).rem_euclid(std::f64::consts::TAU);
        }
    }
    Some(wave)
}
