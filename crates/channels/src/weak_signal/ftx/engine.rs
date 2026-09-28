use std::{f32::consts::TAU, sync::Arc};

use num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use rustfft::{Fft, FftPlanner};

use super::{
    ldpc,
    protocol::{BASEBAND_SYMBOL, MAX_SYMBOLS, MAX_TONES, Protocol, SAMPLE_RATE},
    synth,
};

const BASELINE_PERCENTILE: usize = 40;
const SPECTRAL_BASELINE_REACH: usize = 60;
const SPECTRAL_BASELINE_PERCENTILE: usize = 30;
const SPECTRAL_MIN_RATIO: f32 = 1.1;
const COARSE_TIME_SEARCH: isize = 10;
const JOINT_TIME_SEARCH: isize = 8;
const JOINT_FREQUENCY_STEPS: i32 = 3;
const FREQUENCY_STEPS: i32 = 8;
const LLR_SCALE: f32 = 2.83;
const BP_ITERATIONS: usize = 30;
const SYNC_SYMBOL_SHARE: usize = 3;
const OSD_DISTANCE_FLOOR: f32 = 0.045;
const OSD_DISTANCE_PER_SYNC_SHARE: f32 = 0.084;
const SUBTRACT_WINDOW_S: f32 = 1.0 / 3.0;
const ALIGN_BLOCKS: usize = 8;
const WHOLE_WINDOW_STEP: usize = 2;
const WHOLE_WINDOW_PEAKS: usize = 3;
const PEAK_MERGE_SAMPLES: isize = 4;
const WHOLE_WINDOW_OFFSETS: [f32; 9] = [-4.0, -3.0, -2.0, -1.0, 0.0, 1.0, 2.0, 3.0, 4.0];
const COHERENT_STEPS: i32 = 6;
const COHERENT_STEPS_PER_TONE: f32 = 60.0;

type Spectra = [[Complex<f32>; MAX_TONES]; MAX_SYMBOLS];

#[derive(Clone, Copy)]
pub(crate) struct Search {
    pub(crate) low_hz: f32,
    pub(crate) high_hz: f32,
    pub(crate) max_candidates: usize,
    pub(crate) passes: usize,
    pub(crate) osd_order: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct Found {
    pub(crate) payload: u128,
    pub(crate) text: String,
    pub(crate) frequency_hz: f32,
    pub(crate) start_s: f32,
    pub(crate) snr_db: f32,
    pub(crate) hard_errors: u32,
}

#[derive(Clone, Copy)]
struct Candidate {
    bin: usize,
    frame: isize,
    score: f32,
}

struct Probe {
    table: [[Complex<f32>; BASEBAND_SYMBOL]; MAX_TONES],
    per_symbol: Complex<f32>,
    coherent: bool,
}

struct Fine {
    frequency_hz: f64,
    start: f32,
}

pub(crate) struct Engine {
    protocol: &'static Protocol,
    forward: Arc<dyn RealToComplex<f32>>,
    forward_input: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    forward_scratch: Vec<Complex<f32>>,
    frame_fft: Arc<dyn RealToComplex<f32>>,
    frame_input: Vec<f32>,
    frame_output: Vec<Complex<f32>>,
    frame_scratch: Vec<Complex<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    baseband: Vec<Complex<f32>>,
    inverse_scratch: Vec<Complex<f32>>,
    power: Vec<f32>,
    tone_sums: Vec<f32>,
    sliding: Vec<Complex<f32>>,
    presence: Vec<f32>,
    presence_low_bin: usize,
    twiddles: [[Complex<f32>; BASEBAND_SYMBOL]; MAX_TONES],
    combos: Vec<Vec<u16>>,
    ldpc: ldpc::Decoder,
    wave: Vec<Complex<f32>>,
    envelope: Vec<Complex<f64>>,
    smoothed: Vec<Complex<f64>>,
}

impl Engine {
    pub(crate) fn new(protocol: &'static Protocol) -> Self {
        let mut real = RealFftPlanner::<f32>::new();
        let forward = real.plan_fft_forward(protocol.fft_samples);
        let frame_fft = real.plan_fft_forward(2 * protocol.symbol_samples);
        let inverse = FftPlanner::<f32>::new().plan_fft_inverse(protocol.baseband_samples());
        Self {
            forward_input: forward.make_input_vec(),
            spectrum: forward.make_output_vec(),
            forward_scratch: forward.make_scratch_vec(),
            forward,
            frame_input: frame_fft.make_input_vec(),
            frame_output: frame_fft.make_output_vec(),
            frame_scratch: frame_fft.make_scratch_vec(),
            frame_fft,
            baseband: vec![Complex::default(); protocol.baseband_samples()],
            inverse_scratch: vec![Complex::default(); inverse.get_inplace_scratch_len()],
            inverse,
            power: Vec::new(),
            tone_sums: Vec::new(),
            sliding: Vec::new(),
            presence: Vec::new(),
            presence_low_bin: 0,
            twiddles: std::array::from_fn(|tone| {
                std::array::from_fn(|n| {
                    Complex::from_polar(1.0, -TAU * (tone * n) as f32 / BASEBAND_SYMBOL as f32)
                })
            }),
            combos: protocol
                .coherent_spans
                .iter()
                .map(|&span| combo_bits(protocol, span))
                .collect(),
            ldpc: ldpc::Decoder::new(),
            wave: Vec::new(),
            envelope: Vec::new(),
            smoothed: Vec::new(),
            protocol,
        }
    }

    pub(crate) fn decode(
        &mut self,
        samples: &[f32],
        search: &Search,
        accept: &mut dyn FnMut(u128, bool) -> Option<String>,
    ) -> Vec<Found> {
        let mut audio = samples.to_vec();
        audio.resize(self.protocol.slot_samples, 0.0);
        let mut found: Vec<Found> = Vec::new();
        for pass in 0..search.passes {
            let candidates = self.candidates(&audio, search);
            self.transform(&audio);
            let fresh_from = found.len();
            for candidate in candidates {
                if let Some(result) = self.attempt(candidate, search, accept, &found) {
                    found.push(result);
                }
            }
            if found.len() == fresh_from || pass + 1 == search.passes {
                break;
            }
            for result in &found[fresh_from..] {
                self.subtract(&mut audio, result);
            }
        }
        found
    }

    fn candidates(&mut self, audio: &[f32], search: &Search) -> Vec<Candidate> {
        let protocol = self.protocol;
        let half = protocol.tone_spacing_hz() / 2.0;
        let low_bin = (search.low_hz / half).floor().max(1.0) as usize;
        let high_bin = (search.high_hz / half).ceil() as usize;
        let reach = 2 * (protocol.tones - 1);
        let width = (high_bin + reach + 1).min(self.frame_output.len()) - low_bin;
        let bases = width.saturating_sub(reach);
        let frames = self.spectrogram(audio, low_bin, width);
        self.sum_tones(frames, width, bases);
        let best: Vec<(f32, isize)> = (0..bases)
            .map(|base| self.best_start(base, frames, width, bases))
            .collect();
        let mut sorted: Vec<f32> = best.iter().map(|&(score, _)| score).collect();
        sorted.sort_unstable_by(f32::total_cmp);
        let Some(&baseline) = sorted.get(sorted.len() * BASELINE_PERCENTILE / 100) else {
            return Vec::new();
        };
        if baseline <= 0.0 {
            return Vec::new();
        }
        let score = |base: usize| best[base].0 / baseline;
        let mut candidates: Vec<Candidate> = (0..bases)
            .filter(|&base| {
                let here = score(base);
                here >= protocol.min_sync
                    && (base == 0 || score(base - 1) <= here)
                    && (base + 1 == bases || score(base + 1) < here)
            })
            .map(|base| Candidate {
                bin: low_bin + base,
                frame: best[base].1,
                score: score(base),
            })
            .collect();
        candidates.sort_unstable_by(|a, b| b.score.total_cmp(&a.score));
        self.presence_low_bin = low_bin;
        self.presence = self.spectral_presence(frames, width, bases);
        if protocol.spectral_candidates {
            let extra = self.spectral_candidates(bases, low_bin, &candidates);
            candidates.extend(extra.into_iter().map(|base| Candidate {
                bin: low_bin + base,
                frame: best[base].1,
                score: score(base),
            }));
        }
        candidates.truncate(search.max_candidates);
        candidates
    }

    fn spectral_presence(&self, frames: usize, width: usize, bases: usize) -> Vec<f32> {
        let span = 2 * self.protocol.tones;
        let mut average = vec![0.0f32; width];
        for frame in 0..frames {
            for (sum, power) in average
                .iter_mut()
                .zip(&self.power[frame * width..][..width])
            {
                *sum += power;
            }
        }
        let energy: Vec<f32> = (0..bases)
            .map(|base| average[base..(base + span).min(width)].iter().sum())
            .collect();
        let mut window = Vec::with_capacity(2 * SPECTRAL_BASELINE_REACH + 1);
        (0..bases)
            .map(|base| {
                window.clear();
                let low = base.saturating_sub(SPECTRAL_BASELINE_REACH);
                let high = (base + SPECTRAL_BASELINE_REACH + 1).min(bases);
                window.extend_from_slice(&energy[low..high]);
                let rank = window.len() * SPECTRAL_BASELINE_PERCENTILE / 100;
                let (_, &mut floor, _) = window.select_nth_unstable_by(rank, f32::total_cmp);
                if floor > 0.0 {
                    energy[base] / floor
                } else {
                    0.0
                }
            })
            .collect()
    }

    fn presence_at(&self, frequency_hz: f64) -> f32 {
        let half = f64::from(self.protocol.tone_spacing_hz()) / 2.0;
        let base = (frequency_hz / half).round() as isize - self.presence_low_bin as isize;
        (base - 1..=base + 1)
            .filter_map(|index| usize::try_from(index).ok())
            .filter_map(|index| self.presence.get(index))
            .fold(0.0, |best: f32, &ratio| best.max(ratio))
    }

    fn spectral_candidates(&self, bases: usize, low_bin: usize, known: &[Candidate]) -> Vec<usize> {
        let ratio = &self.presence;
        let mut extra: Vec<(usize, f32)> = (0..bases)
            .filter(|&base| {
                let here = ratio[base];
                here >= SPECTRAL_MIN_RATIO
                    && (base == 0 || ratio[base - 1] <= here)
                    && (base + 1 == bases || ratio[base + 1] < here)
            })
            .map(|base| (base, ratio[base]))
            .collect();
        extra.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
        extra
            .into_iter()
            .map(|(base, _)| base)
            .filter(|&base| {
                !known
                    .iter()
                    .any(|candidate| candidate.bin == low_bin + base)
            })
            .collect()
    }

    fn spectrogram(&mut self, audio: &[f32], low_bin: usize, width: usize) -> usize {
        let samples = self.protocol.symbol_samples;
        let step = samples / 4;
        let frames = (audio.len() - samples) / step + 1;
        self.power.clear();
        self.power.resize(frames * width, 0.0);
        for frame in 0..frames {
            self.frame_input.fill(0.0);
            self.frame_input[..samples].copy_from_slice(&audio[frame * step..][..samples]);
            if self
                .frame_fft
                .process_with_scratch(
                    &mut self.frame_input,
                    &mut self.frame_output,
                    &mut self.frame_scratch,
                )
                .is_err()
            {
                continue;
            }
            for (slot, bin) in self.power[frame * width..][..width]
                .iter_mut()
                .zip(&self.frame_output[low_bin..])
            {
                *slot = bin.norm_sqr();
            }
        }
        frames
    }

    fn sum_tones(&mut self, frames: usize, width: usize, bases: usize) {
        let tones = self.protocol.tones;
        self.tone_sums.clear();
        self.tone_sums.resize(frames * bases, 0.0);
        for frame in 0..frames {
            let row = &self.power[frame * width..][..width];
            for (base, sum) in self.tone_sums[frame * bases..][..bases]
                .iter_mut()
                .enumerate()
            {
                *sum = (0..tones).map(|tone| row[base + 2 * tone]).sum();
            }
        }
    }

    fn best_start(&self, base: usize, frames: usize, width: usize, bases: usize) -> (f32, isize) {
        let protocol = self.protocol;
        let step = (protocol.symbol_samples / 4) as f32;
        let lead = synth::lead_samples(protocol) as f32;
        let first = ((protocol.earliest_start_s * SAMPLE_RATE + lead) / step).floor() as isize;
        let last = ((protocol.latest_start_s * SAMPLE_RATE + lead) / step).ceil() as isize;
        let others = (protocol.tones - 1) as f32;
        let mut best = (0.0, 0);
        for start in first..=last {
            let (mut signal, mut noise, mut late_signal, mut late_noise) = (0.0, 0.0, 0.0, 0.0);
            for (block, &(offset, pattern)) in protocol.sync.iter().enumerate() {
                for (index, &tone) in pattern.iter().enumerate() {
                    let frame = start + 4 * (offset + index) as isize;
                    if frame < 0 || frame as usize >= frames {
                        continue;
                    }
                    let frame = frame as usize;
                    let power = self.power[frame * width + base + 2 * usize::from(tone)];
                    let rest = (self.tone_sums[frame * bases + base] - power) / others;
                    signal += power;
                    noise += rest;
                    if block > 0 {
                        late_signal += power;
                        late_noise += rest;
                    }
                }
            }
            let ratio = |signal: f32, noise: f32| if noise > 0.0 { signal / noise } else { 0.0 };
            let score = ratio(signal, noise).max(ratio(late_signal, late_noise));
            if score > best.0 {
                best = (score, start);
            }
        }
        best
    }

    fn transform(&mut self, audio: &[f32]) {
        self.forward_input.fill(0.0);
        self.forward_input[..audio.len()].copy_from_slice(audio);
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

    fn downconvert(&mut self, frequency_hz: f64) -> f64 {
        let protocol = self.protocol;
        let resolution = f64::from(SAMPLE_RATE) / protocol.fft_samples as f64;
        let centre = (frequency_hz / resolution).round() as isize;
        let spacing = f64::from(protocol.tone_spacing_hz()) / resolution;
        let low = -(1.5 * spacing).round() as isize;
        let high = ((protocol.tones as f64 + 0.5) * spacing).round() as isize;
        let taper = spacing.round().max(1.0) as isize;
        let length = self.baseband.len() as isize;
        self.baseband.fill(Complex::default());
        for offset in low..=high {
            let Some(&bin) = usize::try_from(centre + offset)
                .ok()
                .and_then(|index| self.spectrum.get(index))
            else {
                continue;
            };
            let edge = (offset - low).min(high - offset);
            let gain = if edge < taper {
                0.5 - 0.5 * (std::f32::consts::PI * edge as f32 / taper as f32).cos()
            } else {
                1.0
            };
            self.baseband[offset.rem_euclid(length) as usize] = bin * gain;
        }
        self.inverse
            .process_with_scratch(&mut self.baseband, &mut self.inverse_scratch);
        centre as f64 * resolution
    }

    fn at(&self, index: isize) -> Complex<f32> {
        self.baseband[index.rem_euclid(self.baseband.len() as isize) as usize]
    }

    fn probe(&self, offset_hz: f32, coherent: bool) -> Probe {
        let cycles = offset_hz / self.protocol.tone_spacing_hz();
        let drift =
            |n: usize| Complex::from_polar(1.0, -TAU * cycles * n as f32 / BASEBAND_SYMBOL as f32);
        let drifts: [Complex<f32>; BASEBAND_SYMBOL] = std::array::from_fn(drift);
        Probe {
            table: std::array::from_fn(|tone| {
                std::array::from_fn(|n| self.twiddles[tone][n] * drifts[n])
            }),
            per_symbol: drift(BASEBAND_SYMBOL),
            coherent,
        }
    }

    fn sync_power(&self, start: isize, offset_hz: f32) -> f32 {
        self.sync_score(start, &self.probe(offset_hz, self.protocol.coherent_sync))
    }

    fn sync_score(&self, start: isize, probe: &Probe) -> f32 {
        let mut total = 0.0;
        for &(first, pattern) in self.protocol.sync {
            let mut block = Complex::<f32>::default();
            let mut phase = Complex::new(1.0f32, 0.0);
            for (index, &tone) in pattern.iter().enumerate() {
                let samples = self.symbol(start + ((first + index) * BASEBAND_SYMBOL) as isize);
                let sum: Complex<f32> = samples
                    .iter()
                    .zip(&probe.table[usize::from(tone)])
                    .map(|(sample, twiddle)| sample * twiddle)
                    .sum();
                if probe.coherent {
                    block += sum * phase;
                    phase *= probe.per_symbol;
                } else {
                    total += sum.norm_sqr();
                }
            }
            total += block.norm_sqr();
        }
        total
    }

    fn symbol(&self, begin: isize) -> [Complex<f32>; BASEBAND_SYMBOL] {
        let length = self.baseband.len() as isize;
        if begin >= 0 && begin + BASEBAND_SYMBOL as isize <= length {
            let begin = begin as usize;
            let mut samples = [Complex::default(); BASEBAND_SYMBOL];
            samples.copy_from_slice(&self.baseband[begin..begin + BASEBAND_SYMBOL]);
            return samples;
        }
        std::array::from_fn(|n| self.at(begin + n as isize))
    }

    fn best_time(&self, around: isize, reach: isize, offset_hz: f32) -> (isize, f32) {
        let probe = self.probe(offset_hz, self.protocol.coherent_sync);
        (around - reach..=around + reach)
            .map(|start| (start, self.sync_score(start, &probe)))
            .fold((around, f32::MIN), |best, next| {
                if next.1 > best.1 { next } else { best }
            })
    }

    fn refine(&mut self, candidate: Candidate) -> Fine {
        let protocol = self.protocol;
        let half = f64::from(protocol.tone_spacing_hz()) / 2.0;
        let centre = self.downconvert(candidate.bin as f64 * half);
        let per_frame = (protocol.symbol_samples / 4 / protocol.decimation()) as isize;
        let (start, offset) = if protocol.full_time_search {
            let (start, offset, _) = self
                .whole_window_peaks()
                .into_iter()
                .map(|(start, offset)| self.joint(start, offset))
                .map(|(start, offset)| (start, offset, self.sync_power(start, offset)))
                .fold((0, 0.0, f32::MIN), |best, next| {
                    if next.2 > best.2 { next } else { best }
                });
            (start, offset)
        } else {
            let (start, _) = self.best_time(candidate.frame * per_frame, COARSE_TIME_SEARCH, 0.0);
            let step = protocol.tone_spacing_hz() / 12.5;
            let powers: Vec<f32> = (-FREQUENCY_STEPS..=FREQUENCY_STEPS)
                .map(|index| self.sync_power(start, index as f32 * step))
                .collect();
            let peak = argmax(&powers);
            let offset = (peak as f32 - FREQUENCY_STEPS as f32 + interpolate(&powers, peak)) * step;
            (
                self.best_time(start, JOINT_TIME_SEARCH / 2, offset).0,
                offset,
            )
        };
        let offset = self.coherent_frequency(start, offset);
        let probe = self.probe(offset, protocol.coherent_sync);
        let around = [
            self.sync_score(start - 1, &probe),
            self.sync_score(start, &probe),
            self.sync_score(start + 1, &probe),
        ];
        self.rotate(offset);
        Fine {
            frequency_hz: centre + f64::from(offset),
            start: start as f32 + interpolate(&around, 1),
        }
    }

    fn joint(&self, start: isize, offset_hz: f32) -> (isize, f32) {
        let step = self.protocol.tone_spacing_hz() / 12.5;
        let mut best = (start, offset_hz, f32::MIN);
        for index in -JOINT_FREQUENCY_STEPS..=JOINT_FREQUENCY_STEPS {
            let offset = offset_hz + index as f32 * step;
            let probe = self.probe(offset, self.protocol.coherent_sync);
            for time in start - JOINT_TIME_SEARCH..=start + JOINT_TIME_SEARCH {
                let power = self.sync_score(time, &probe);
                if power > best.2 {
                    best = (time, offset, power);
                }
            }
        }
        (best.0, best.1)
    }

    fn whole_window_peaks(&mut self) -> Vec<(isize, f32)> {
        let protocol = self.protocol;
        let rate = protocol.baseband_rate();
        let lead = (protocol.ramp_symbols * BASEBAND_SYMBOL) as isize;
        let first = (protocol.earliest_start_s * rate) as isize + lead;
        let last = (protocol.latest_start_s * rate) as isize + lead;
        let eighth = protocol.tone_spacing_hz() / 8.0;
        let length = self.baseband.len();
        let mut peaks: Vec<(isize, f32, f32)> = Vec::with_capacity(WHOLE_WINDOW_PEAKS + 1);
        for steps in WHOLE_WINDOW_OFFSETS {
            let cycles = steps * eighth / protocol.tone_spacing_hz();
            self.slide(cycles);
            let per_symbol = Complex::from_polar(1.0, -TAU * cycles);
            for start in (first..=last).step_by(WHOLE_WINDOW_STEP) {
                let mut total = 0.0;
                for &(offset, pattern) in protocol.sync {
                    let mut block = Complex::<f32>::default();
                    let mut phase = Complex::new(1.0f32, 0.0);
                    for (index, &tone) in pattern.iter().enumerate() {
                        let at = (start + ((offset + index) * BASEBAND_SYMBOL) as isize)
                            .rem_euclid(length as isize) as usize;
                        block += self.sliding[usize::from(tone) * length + at] * phase;
                        phase *= per_symbol;
                    }
                    total += block.norm_sqr();
                }
                remember_peak(&mut peaks, (start, steps * eighth, total), eighth);
            }
        }
        peaks
            .into_iter()
            .map(|(start, offset, _)| (start, offset))
            .collect()
    }

    fn slide(&mut self, cycles: f32) {
        let length = self.baseband.len();
        let tones = self.protocol.tones;
        self.sliding.resize(tones * length, Complex::default());
        for tone in 0..tones {
            let frequency = (tone as f32 + cycles) / BASEBAND_SYMBOL as f32;
            let turn = Complex::from_polar(1.0, TAU * frequency);
            let mut sum: Complex<f32> = (0..BASEBAND_SYMBOL)
                .map(|n| self.baseband[n] * Complex::from_polar(1.0, -TAU * frequency * n as f32))
                .sum();
            let row = &mut self.sliding[tone * length..(tone + 1) * length];
            let window = Complex::from_polar(1.0, -TAU * frequency * BASEBAND_SYMBOL as f32);
            for (n, slot) in row.iter_mut().enumerate() {
                *slot = sum;
                let leaving = self.baseband[n];
                let entering = self.baseband[(n + BASEBAND_SYMBOL) % length];
                sum = (sum - leaving + entering * window) * turn;
            }
        }
    }

    fn coherent_frequency(&self, start: isize, around_hz: f32) -> f32 {
        let step = self.protocol.tone_spacing_hz() / COHERENT_STEPS_PER_TONE;
        let powers: Vec<f32> = (-COHERENT_STEPS..=COHERENT_STEPS)
            .map(|index| self.sync_score(start, &self.probe(around_hz + index as f32 * step, true)))
            .collect();
        let peak = argmax(&powers);
        around_hz + (peak as f32 - COHERENT_STEPS as f32 + interpolate(&powers, peak)) * step
    }

    fn rotate(&mut self, offset_hz: f32) {
        let rate = self.protocol.baseband_rate();
        let length = self.baseband.len();
        let step = Complex::from_polar(1.0, -TAU * offset_hz / rate);
        let mut rotation = Complex::new(1.0f32, 0.0);
        for (index, sample) in self.baseband.iter_mut().enumerate() {
            if index % 256 == 0 {
                let time = if index < length / 2 {
                    index as f32
                } else {
                    index as f32 - length as f32
                };
                rotation = Complex::from_polar(1.0, -TAU * offset_hz * time / rate);
            }
            *sample *= rotation;
            rotation *= step;
        }
    }

    fn spectra(&self, start: isize) -> Spectra {
        let protocol = self.protocol;
        let mut spectra = [[Complex::default(); MAX_TONES]; MAX_SYMBOLS];
        for (symbol, bins) in spectra.iter_mut().take(protocol.symbols).enumerate() {
            let begin = start + (symbol * BASEBAND_SYMBOL) as isize;
            let samples = self.symbol(begin);
            for (tone, bin) in bins.iter_mut().take(protocol.tones).enumerate() {
                *bin = samples
                    .iter()
                    .zip(&self.twiddles[tone])
                    .map(|(sample, twiddle)| sample * twiddle)
                    .sum();
            }
        }
        spectra
    }

    fn osd_distance(&self, sync: usize) -> f32 {
        let total: usize = self
            .protocol
            .sync
            .iter()
            .map(|(_, pattern)| pattern.len())
            .sum();
        let share = sync as f32 / total as f32 - 1.0 / SYNC_SYMBOL_SHARE as f32;
        OSD_DISTANCE_FLOOR + OSD_DISTANCE_PER_SYNC_SHARE * share
    }

    fn min_sync_symbols(&self) -> usize {
        let total: usize = self
            .protocol
            .sync
            .iter()
            .map(|(_, pattern)| pattern.len())
            .sum();
        total / SYNC_SYMBOL_SHARE
    }

    fn sync_symbols(&self, spectra: &Spectra) -> usize {
        let protocol = self.protocol;
        (0..protocol.symbols)
            .filter(|&symbol| {
                protocol.sync_tone(symbol).is_some_and(|tone| {
                    let bins = &spectra[symbol][..protocol.tones];
                    argmax_by(bins, |bin| bin.norm_sqr()) == usize::from(tone)
                })
            })
            .count()
    }

    fn attempt(
        &mut self,
        candidate: Candidate,
        search: &Search,
        accept: &mut dyn FnMut(u128, bool) -> Option<String>,
        found: &[Found],
    ) -> Option<Found> {
        let protocol = self.protocol;
        let fine = self.refine(candidate);
        let spectra = self.spectra(fine.start.round() as isize);
        let sync = self.sync_symbols(&spectra);
        if sync < self.min_sync_symbols() {
            return None;
        }
        let decoded = self.decode_spectra(&spectra, 0, 0.0, accept, found);
        let osd_distance = if self.presence_at(fine.frequency_hz) >= protocol.osd_min_presence {
            self.osd_distance(sync)
        } else {
            0.0
        };
        let (payload, text, hard_errors) = decoded.or_else(|| {
            self.decode_spectra(&spectra, search.osd_order, osd_distance, accept, found)
        })?;
        let tones = protocol.tones_for(payload);
        let lead = synth::lead_samples(protocol) as f32;
        let coarse = fine.start * protocol.decimation() as f32 - lead;
        let (start, drift_hz) = self.pinpoint(&tones, coarse.round() as isize);
        Some(Found {
            payload,
            text,
            frequency_hz: (fine.frequency_hz + drift_hz) as f32,
            start_s: start as f32 / SAMPLE_RATE,
            snr_db: self.snr(&spectra, &tones),
            hard_errors,
        })
    }

    fn decode_spectra(
        &mut self,
        spectra: &Spectra,
        osd_order: usize,
        osd_distance: f32,
        accept: &mut dyn FnMut(u128, bool) -> Option<String>,
        found: &[Found],
    ) -> Option<(u128, String, u32)> {
        let protocol = self.protocol;
        let mut sets = Vec::with_capacity(protocol.coherent_spans.len() + 1);
        let mut single = None;
        for (index, &span) in protocol.coherent_spans.iter().enumerate() {
            let (plain, normalised) = self.llrs(spectra, span, &self.combos[index]);
            sets.push(plain);
            single.get_or_insert(normalised);
        }
        sets.extend(single);
        if osd_order == 0 {
            return sets
                .iter()
                .find_map(|llr| self.try_decode(llr, 0, 0.0, accept, found));
        }
        let spans = protocol.coherent_spans.len();
        (0..spans)
            .rev()
            .chain([spans])
            .find_map(|set| self.try_decode(&sets[set], osd_order, osd_distance, accept, found))
    }

    fn try_decode(
        &mut self,
        llr: &[f32; ldpc::N],
        osd_order: usize,
        osd_distance: f32,
        accept: &mut dyn FnMut(u128, bool) -> Option<String>,
        found: &[Found],
    ) -> Option<(u128, String, u32)> {
        let present = llr
            .iter()
            .filter(|value| value.abs() > f32::EPSILON)
            .count();
        let osd_order = if present * 10 >= ldpc::N * 9 {
            osd_order
        } else {
            0
        };
        let decoded = self.ldpc.decode(llr, BP_ITERATIONS, osd_order)?;
        if decoded.osd && decoded.distance > osd_distance {
            return None;
        }
        let payload = decoded.payload ^ self.protocol.scramble;
        if found.iter().any(|known| known.payload == payload) {
            return None;
        }
        let text = accept(payload, decoded.osd)?;
        Some((payload, text, decoded.hard_errors))
    }

    fn llrs(
        &self,
        spectra: &Spectra,
        span: usize,
        combos: &[u16],
    ) -> ([f32; ldpc::N], [f32; ldpc::N]) {
        let protocol = self.protocol;
        let bits_per_symbol = protocol.bits_per_symbol;
        let mut plain = [0.0f32; ldpc::N];
        let mut normalised = [0.0f32; ldpc::N];
        let mut bit = 0;
        let mut metrics = vec![0.0f32; combos.len()];
        for (block_start, block_length) in protocol.data_blocks() {
            let mut offset = 0;
            while offset < block_length {
                let group = span.min(block_length - offset);
                let count = protocol.tones.pow(group as u32);
                let first = block_start + offset;
                for (combo, metric) in metrics.iter_mut().take(count).enumerate() {
                    let mut sum = Complex::default();
                    let mut rest = combo;
                    for symbol in (0..group).rev() {
                        sum += spectra[first + symbol][rest % protocol.tones];
                        rest /= protocol.tones;
                    }
                    *metric = sum.norm_sqr();
                }
                let group_bits = group * bits_per_symbol;
                for index in 0..group_bits {
                    let mask = 1u16 << (group_bits - 1 - index);
                    let (mut one, mut zero) = (0.0f32, 0.0f32);
                    for (combo, &metric) in metrics.iter().take(count).enumerate() {
                        if combos[combo] & mask != 0 {
                            one = one.max(metric);
                        } else {
                            zero = zero.max(metric);
                        }
                    }
                    let (one, zero) = (one.sqrt(), zero.sqrt());
                    plain[bit + index] = one - zero;
                    let scale = one.max(zero);
                    normalised[bit + index] = if scale > 0.0 {
                        (one - zero) / scale
                    } else {
                        0.0
                    };
                }
                bit += group_bits;
                offset += group;
            }
        }
        normalise(&mut plain);
        normalise(&mut normalised);
        (plain, normalised)
    }

    fn pinpoint(&mut self, tones: &[u8; MAX_SYMBOLS], coarse: isize) -> (isize, f64) {
        let protocol = self.protocol;
        synth::waveform(protocol, &tones[..protocol.symbols], 0.0, &mut self.wave);
        let step = protocol.decimation() as isize;
        let quarter = (step / 4).max(1);
        let mut best = (coarse, f64::MIN);
        for shift in (-2 * step..=2 * step).step_by(quarter as usize) {
            best = self.better_alignment(best, coarse + shift);
        }
        let centre = best.0;
        for shift in -quarter..=quarter {
            best = self.better_alignment(best, centre + shift);
        }
        (best.0, self.phase_slope(best.0))
    }

    fn better_alignment(&self, best: (isize, f64), start: isize) -> (isize, f64) {
        let power: f64 = self
            .block_correlations(start)
            .iter()
            .map(|(_, sum)| sum.norm_sqr())
            .sum();
        if power > best.1 { (start, power) } else { best }
    }

    fn block_correlations(&self, start: isize) -> [(f64, Complex<f64>); ALIGN_BLOCKS] {
        let step = self.protocol.decimation() as isize;
        let first = start.div_euclid(step) + 1;
        let offset = (first * step - start) as usize;
        let count = (self.wave.len() - offset) / step as usize;
        let per_block = count.div_ceil(ALIGN_BLOCKS);
        let mut blocks = [(0.0, Complex::default()); ALIGN_BLOCKS];
        for index in 0..count {
            let reference = self.wave[offset + index * step as usize];
            let sample = self.at(first + index as isize);
            let product = sample * reference.conj();
            let block = &mut blocks[index / per_block];
            block.0 += index as f64;
            block.1 += Complex::new(f64::from(product.re), f64::from(product.im));
        }
        for (index, block) in blocks.iter_mut().enumerate() {
            let members = per_block
                .min(count.saturating_sub(index * per_block))
                .max(1);
            block.0 = block.0 / members as f64 * step as f64 / f64::from(SAMPLE_RATE);
        }
        blocks
    }

    fn phase_slope(&self, start: isize) -> f64 {
        let blocks = self.block_correlations(start);
        let mut unwrapped = Vec::with_capacity(ALIGN_BLOCKS);
        let mut previous = 0.0;
        for (index, (time, sum)) in blocks.iter().enumerate() {
            let mut phase = sum.arg();
            if index > 0 {
                phase = previous
                    + (phase - previous + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
                    - std::f64::consts::PI;
            }
            previous = phase;
            unwrapped.push((*time, phase, sum.norm()));
        }
        let weight: f64 = unwrapped.iter().map(|&(_, _, w)| w).sum();
        if weight <= 0.0 {
            return 0.0;
        }
        let mean_time = unwrapped.iter().map(|&(t, _, w)| t * w).sum::<f64>() / weight;
        let mean_phase = unwrapped.iter().map(|&(_, p, w)| p * w).sum::<f64>() / weight;
        let (mut covariance, mut variance) = (0.0, 0.0);
        for &(time, phase, w) in &unwrapped {
            covariance += w * (time - mean_time) * (phase - mean_phase);
            variance += w * (time - mean_time) * (time - mean_time);
        }
        if variance <= 0.0 {
            return 0.0;
        }
        covariance / variance / std::f64::consts::TAU
    }

    fn snr(&self, spectra: &Spectra, tones: &[u8; MAX_SYMBOLS]) -> f32 {
        let protocol = self.protocol;
        let (mut signal, mut noise) = (0.0f32, 0.0f32);
        for (bins, &tone) in spectra.iter().zip(tones).take(protocol.symbols) {
            let tone = usize::from(tone);
            signal += bins[tone].norm_sqr();
            noise += bins[(tone + protocol.tones / 2) % protocol.tones].norm_sqr();
        }
        let ratio = if noise > 0.0 {
            signal / noise - 1.0
        } else {
            1e3
        };
        10.0 * ratio.max(1e-3).log10() + protocol.snr_offset_db
    }

    fn subtract(&mut self, audio: &mut [f32], found: &Found) {
        let protocol = self.protocol;
        let tones = protocol.tones_for(found.payload);
        synth::waveform(
            protocol,
            &tones[..protocol.symbols],
            f64::from(found.frequency_hz),
            &mut self.wave,
        );
        let start = (found.start_s * SAMPLE_RATE).round() as isize;
        let begin = start.max(0) as usize;
        let skip = (begin as isize - start) as usize;
        let end = ((start + self.wave.len() as isize).max(0) as usize).min(audio.len());
        if end <= begin || skip >= self.wave.len() {
            return;
        }
        let reference = &self.wave[skip..skip + (end - begin)];
        self.envelope.clear();
        self.envelope.extend(
            audio[begin..end]
                .iter()
                .zip(reference)
                .map(|(&sample, wave)| {
                    Complex::new(f64::from(sample * wave.re), -f64::from(sample * wave.im))
                }),
        );
        let half = (SUBTRACT_WINDOW_S * SAMPLE_RATE / 4.0) as usize;
        box_average(&self.envelope, half, &mut self.smoothed);
        box_average(&self.smoothed, half, &mut self.envelope);
        for ((sample, wave), amplitude) in audio[begin..end]
            .iter_mut()
            .zip(reference)
            .zip(&self.envelope)
        {
            let wave = Complex::new(f64::from(wave.re), f64::from(wave.im));
            *sample -= (2.0 * (amplitude * wave).re) as f32;
        }
    }
}

fn remember_peak(peaks: &mut Vec<(isize, f32, f32)>, peak: (isize, f32, f32), merge_hz: f32) {
    if let Some(near) = peaks.iter_mut().find(|known| {
        (known.0 - peak.0).abs() <= PEAK_MERGE_SAMPLES && (known.1 - peak.1).abs() <= merge_hz
    }) {
        if peak.2 > near.2 {
            *near = peak;
        }
    } else {
        peaks.push(peak);
    }
    peaks.sort_unstable_by(|a, b| b.2.total_cmp(&a.2));
    peaks.truncate(WHOLE_WINDOW_PEAKS);
}

fn box_average(input: &[Complex<f64>], half: usize, out: &mut Vec<Complex<f64>>) {
    let mut prefix = Vec::with_capacity(input.len() + 1);
    prefix.push(Complex::<f64>::default());
    let mut running = Complex::<f64>::default();
    for &value in input {
        running += value;
        prefix.push(running);
    }
    out.clear();
    out.extend((0..input.len()).map(|index| {
        let low = index.saturating_sub(half);
        let high = (index + half + 1).min(input.len());
        (prefix[high] - prefix[low]) / (high - low) as f64
    }));
}

fn combo_bits(protocol: &Protocol, span: usize) -> Vec<u16> {
    let count = protocol.tones.pow(span as u32);
    (0..count)
        .map(|combo| {
            let mut bits = 0u16;
            let mut rest = combo;
            let mut shift = 0;
            for _ in 0..span {
                let value = protocol.inverse_gray(rest % protocol.tones) as u16;
                bits |= value << shift;
                shift += protocol.bits_per_symbol;
                rest /= protocol.tones;
            }
            bits
        })
        .collect()
}

fn normalise(values: &mut [f32; ldpc::N]) {
    let count = values.len() as f32;
    let mean = values.iter().sum::<f32>() / count;
    let square = values.iter().map(|value| value * value).sum::<f32>() / count;
    let variance = square - mean * mean;
    let sigma = if variance > 0.0 {
        variance.sqrt()
    } else {
        square.sqrt()
    };
    if sigma > 0.0 {
        for value in values.iter_mut() {
            *value *= LLR_SCALE / sigma;
        }
    }
}

fn argmax(values: &[f32]) -> usize {
    argmax_by(values, |&value| value)
}

fn argmax_by<T>(values: &[T], key: impl Fn(&T) -> f32) -> usize {
    values
        .iter()
        .enumerate()
        .fold((0, f32::MIN), |best, (index, value)| {
            let score = key(value);
            if score > best.1 { (index, score) } else { best }
        })
        .0
}

fn interpolate(values: &[f32], peak: usize) -> f32 {
    if peak == 0 || peak + 1 >= values.len() {
        return 0.0;
    }
    let (left, centre, right) = (values[peak - 1], values[peak], values[peak + 1]);
    let curvature = left - 2.0 * centre + right;
    if curvature >= 0.0 {
        return 0.0;
    }
    (0.5 * (left - right) / curvature).clamp(-0.5, 0.5)
}

#[cfg(test)]
mod tests {
    use super::{super::protocol::FT8, *};

    fn search() -> Search {
        Search {
            low_hz: 200.0,
            high_hz: 3_000.0,
            max_candidates: 50,
            passes: 1,
            osd_order: 0,
        }
    }

    fn band_power(audio: &[f32], centre_hz: f32) -> f32 {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(audio.len());
        let mut input = audio.to_vec();
        let mut output = fft.make_output_vec();
        fft.process(&mut input, &mut output).unwrap();
        let resolution = SAMPLE_RATE / audio.len() as f32;
        let low = ((centre_hz - 10.0) / resolution) as usize;
        let high = ((centre_hz + 60.0) / resolution) as usize;
        output[low..high].iter().map(|bin| bin.norm_sqr()).sum()
    }

    #[test]
    fn subtraction_leaves_little_of_a_decoded_signal() {
        let payload = super::super::message::pack("CQ K1ABC FN42").unwrap();
        let tones = FT8.tones_for(payload);
        let mut wave = Vec::new();
        synth::waveform(&FT8, &tones[..FT8.symbols], 1_234.56, &mut wave);
        let offset = 8_765;
        let mut audio = vec![0.0f32; FT8.slot_samples];
        for (sample, value) in audio[offset..].iter_mut().zip(&wave) {
            *sample = 0.1 * value.re;
        }
        let before = band_power(&audio, 1_234.56);
        let mut engine = Engine::new(&FT8);
        let found = engine.decode(&audio, &search(), &mut |_, _| Some(String::new()));
        assert_eq!(found.len(), 1);
        engine.subtract(&mut audio, &found[0]);
        let after = band_power(&audio, 1_234.56);
        let residual_db = 10.0 * (after / before).log10();
        assert!(residual_db < -35.0, "residual {residual_db:.1} dB");
    }
}
