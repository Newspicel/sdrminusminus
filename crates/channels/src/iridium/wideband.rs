use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use super::CHANNEL_RATE;
use super::ddc::ChannelDdc;
use super::demod::{IridiumDemod, SYMBOL_RATE};

const THRESHOLD_DB: f32 = 16.0;
const HISTORY: usize = 512;
const ENBW: f32 = 1.72;
const BURST_WIDTH_HZ: f64 = 40_000.0;
const WARMUP_FRAMES: u64 = 64;
const MAX_BURST_S: f64 = 0.092;
const POST_S: f64 = 0.024;
const PRE_S: f64 = 0.004;
const PASSBAND_HZ: f64 = 28_000.0;
const MIN_BURST_SPAN: u64 = 2;
const MF_TAPS: usize = 51;
const MF_ALPHA: f64 = 0.4;
const RECENT_KEYS: usize = 256;
const DEDUP_BUCKET_HZ: f64 = 60_000.0;
const QUIET_TAIL_S: f64 = 0.15;

pub fn rrc_taps(sps: f64, num_taps: usize, beta: f64) -> Vec<f32> {
    use std::f64::consts::{PI, SQRT_2};
    let mid = (num_taps - 1) as f64 / 2.0;
    let mut taps: Vec<f64> = (0..num_taps)
        .map(|n| {
            let t = (n as f64 - mid) / sps;
            if t.abs() < 1e-9 {
                1.0 - beta + 4.0 * beta / PI
            } else if (t.abs() - 1.0 / (4.0 * beta)).abs() < 1e-9 {
                (beta / SQRT_2)
                    * ((1.0 + 2.0 / PI) * (PI / (4.0 * beta)).sin()
                        + (1.0 - 2.0 / PI) * (PI / (4.0 * beta)).cos())
            } else {
                let pt = PI * t;
                ((pt * (1.0 - beta)).sin() + 4.0 * beta * t * (pt * (1.0 + beta)).cos())
                    / (pt * (1.0 - (4.0 * beta * t).powi(2)))
            }
        })
        .collect();
    let energy: f64 = taps.iter().map(|h| h * h).sum::<f64>().sqrt();
    taps.iter_mut().for_each(|h| *h /= energy);
    taps.into_iter().map(|h| h as f32).collect()
}

fn matched_filter(x: &[Complex<f32>], taps: &[f32]) -> Vec<Complex<f32>> {
    let half = (taps.len() / 2) as isize;
    (0..x.len() as isize)
        .map(|i| {
            let mut acc = Complex::new(0.0f32, 0.0);
            for (k, &tap) in taps.iter().enumerate() {
                let idx = i + k as isize - half;
                if idx >= 0 && (idx as usize) < x.len() {
                    acc += x[idx as usize] * tap;
                }
            }
            acc
        })
        .collect()
}

#[derive(Clone, Copy)]
struct ActiveBurst {
    bin: usize,
    start_frame: u64,
    last_frame: u64,
}

impl ActiveBurst {
    fn long_enough(&self) -> bool {
        self.last_frame - self.start_frame >= MIN_BURST_SPAN
    }
}

pub struct WidebandBurst {
    pub offset_hz: f64,
    pub bits: Vec<u8>,
}

pub struct IridiumWideband {
    input_rate: f64,
    fft: Arc<dyn Fft<f32>>,
    nfft: usize,
    window: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    mag: Vec<f32>,
    hot: Vec<bool>,
    mask: Vec<bool>,
    base_sum: Vec<f32>,
    base_hist: Vec<f32>,
    base_slot: Vec<u32>,
    buf: Vec<Complex<f32>>,
    start_abs: u64,
    next_frame: u64,
    floor_frames: u64,
    active: Vec<ActiveBurst>,
    det_threshold: f32,
    half_width_bins: usize,
    recent_bits: VecDeque<u64>,
    mf_taps: Vec<f32>,
}

impl IridiumWideband {
    pub fn new(input_rate: f64) -> Result<Self, String> {
        let decim = (input_rate / CHANNEL_RATE).round() as usize;
        if decim == 0 || (input_rate - decim as f64 * CHANNEL_RATE).abs() > 1e-6 {
            return Err(format!(
                "input rate {input_rate} is not an integer multiple of {CHANNEL_RATE}"
            ));
        }
        let bins = ((input_rate / 1000.0).round() as usize).max(2);
        let nfft = 1usize << (usize::BITS - 1 - bins.leading_zeros());
        let window = (0..nfft)
            .map(|n| {
                let x = std::f32::consts::TAU * n as f32 / (nfft - 1) as f32;
                0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos()
            })
            .collect();
        let fft = FftPlanner::new().plan_fft_forward(nfft);
        let scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
        let bin_width = input_rate / nfft as f64;
        Ok(Self {
            input_rate,
            fft,
            nfft,
            window,
            spectrum: vec![Complex::default(); nfft],
            scratch,
            mag: vec![0.0; nfft],
            hot: vec![false; nfft],
            mask: vec![true; nfft],
            base_sum: vec![0.0; nfft],
            base_hist: vec![0.0; nfft * HISTORY],
            base_slot: vec![0; nfft],
            buf: Vec::new(),
            start_abs: 0,
            next_frame: 0,
            floor_frames: 0,
            active: Vec::new(),
            det_threshold: 10f32.powf(THRESHOLD_DB / 10.0) / ENBW,
            half_width_bins: ((BURST_WIDTH_HZ / bin_width / 2.0).round() as usize).max(1),
            recent_bits: VecDeque::with_capacity(RECENT_KEYS + 1),
            mf_taps: rrc_taps(CHANNEL_RATE / SYMBOL_RATE, MF_TAPS, MF_ALPHA),
        })
    }

    pub fn process(&mut self, input: &[Complex<f32>]) -> Vec<WidebandBurst> {
        self.buf.extend_from_slice(input);
        let frame_len = self.nfft as u64;
        let mut finished = Vec::new();
        while (self.next_frame + 1) * frame_len <= self.start_abs + self.buf.len() as u64 {
            let rel = (self.next_frame * frame_len - self.start_abs) as usize;
            self.frame_magnitudes(rel);
            self.detect_frame(&mut finished);
            self.next_frame += 1;
        }
        let mut out: Vec<WidebandBurst> = finished.iter().flat_map(|b| self.extract(b)).collect();
        out.retain(|burst| self.first_sighting(burst));
        self.drop_consumed();
        out
    }

    fn frame_magnitudes(&mut self, rel: usize) {
        let samples = &self.buf[rel..rel + self.nfft];
        for ((slot, s), &w) in self.spectrum.iter_mut().zip(samples).zip(&self.window) {
            *slot = s * w;
        }
        self.fft
            .process_with_scratch(&mut self.spectrum, &mut self.scratch);
        for (m, c) in self.mag.iter_mut().zip(&self.spectrum) {
            *m = c.norm_sqr();
        }
    }

    fn mean(&self, k: usize) -> f32 {
        self.base_sum[k] / self.base_slot[k].clamp(1, HISTORY as u32) as f32
    }

    fn masked_range(&self, center: usize) -> std::ops::RangeInclusive<usize> {
        let hw = self.half_width_bins as i64;
        let lo = (center as i64 - hw).max(0) as usize;
        let hi = (center as i64 + hw).min(self.nfft as i64 - 1) as usize;
        lo..=hi
    }

    fn detect_frame(&mut self, finished: &mut Vec<ActiveBurst>) {
        self.floor_frames += 1;
        let detecting = self.floor_frames > WARMUP_FRAMES;
        self.mark_hot(detecting);
        self.extend_active();
        self.mask.fill(true);
        for index in 0..self.active.len() {
            let range = self.masked_range(self.active[index].bin);
            self.mask[range].fill(false);
        }
        if detecting {
            self.claim_peaks();
        }
        self.squelch(finished);
        self.update_baseline();
        self.finish_quiet(finished);
    }

    fn mark_hot(&mut self, detecting: bool) {
        for k in 0..self.nfft {
            let slot = self.base_slot[k];
            self.hot[k] = detecting && slot > 0 && {
                let mean = self.base_sum[k] / slot.min(HISTORY as u32) as f32;
                mean > 0.0 && self.mag[k] > mean * self.det_threshold
            };
        }
    }

    fn extend_active(&mut self) {
        for burst in &mut self.active {
            let c = burst.bin as i64;
            let lit = (-1..=1).any(|d| {
                let b = c + d;
                b >= 0 && (b as usize) < self.nfft && self.hot[b as usize]
            });
            if lit {
                burst.last_frame = self.next_frame;
            }
        }
    }

    fn claim_peaks(&mut self) {
        let hw = self.half_width_bins;
        let mut peaks: Vec<(usize, f32)> = (hw..self.nfft.saturating_sub(hw))
            .filter(|&k| self.mask[k] && self.hot[k])
            .map(|k| {
                let mean = self.mean(k);
                (k, if mean > 0.0 { self.mag[k] / mean } else { 0.0 })
            })
            .collect();
        peaks.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
        for (bin, _) in peaks {
            if !self.mask[bin] {
                continue;
            }
            self.active.push(ActiveBurst {
                bin,
                start_frame: self.next_frame,
                last_frame: self.next_frame,
            });
            let range = self.masked_range(bin);
            self.mask[range].fill(false);
        }
    }

    fn squelch(&mut self, finished: &mut Vec<ActiveBurst>) {
        let max_bursts = ((self.input_rate / BURST_WIDTH_HZ) * 0.8) as usize;
        if self.active.len() <= max_bursts {
            return;
        }
        finished.extend(
            self.active
                .iter()
                .filter(|b| b.start_frame < self.next_frame && b.long_enough()),
        );
        self.active.clear();
    }

    fn update_baseline(&mut self) {
        for k in (0..self.nfft).filter(|&k| self.mask[k]) {
            let idx = k * HISTORY + (self.base_slot[k] as usize) % HISTORY;
            self.base_sum[k] += self.mag[k] - self.base_hist[idx];
            self.base_hist[idx] = self.mag[k];
            self.base_slot[k] += 1;
        }
    }

    fn finish_quiet(&mut self, finished: &mut Vec<ActiveBurst>) {
        let frame_len = self.nfft as f64;
        let post_frames = (POST_S * self.input_rate / frame_len).ceil() as u64;
        let max_frames = (MAX_BURST_S * self.input_rate / frame_len).ceil() as u64;
        let current = self.next_frame;
        self.active.retain(|burst| {
            let done = current >= burst.last_frame + post_frames
                || burst.last_frame - burst.start_frame > max_frames;
            if done && burst.long_enough() {
                finished.push(*burst);
            }
            !done
        });
    }

    fn first_sighting(&mut self, burst: &WidebandBurst) -> bool {
        let mut hasher = DefaultHasher::new();
        burst.bits.hash(&mut hasher);
        let bucket = (burst.offset_hz / DEDUP_BUCKET_HZ).round() as i64;
        let key = hasher.finish() ^ (bucket as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        if self.recent_bits.contains(&key) {
            return false;
        }
        self.recent_bits.push_back(key);
        if self.recent_bits.len() > RECENT_KEYS {
            self.recent_bits.pop_front();
        }
        true
    }

    fn drop_consumed(&mut self) {
        let earliest = self
            .active
            .iter()
            .map(|b| b.start_frame)
            .min()
            .unwrap_or(self.next_frame)
            * self.nfft as u64;
        let pre = (PRE_S * self.input_rate) as u64;
        let keep_from = earliest.saturating_sub(pre).max(self.start_abs);
        let drop = (keep_from - self.start_abs) as usize;
        if drop > 0 && drop <= self.buf.len() {
            self.buf.drain(..drop);
            self.start_abs = keep_from;
        }
    }

    fn offset_hz(&self, bin: usize) -> f64 {
        let bin = bin as f64;
        let nfft = self.nfft as f64;
        let signed = if bin <= nfft / 2.0 { bin } else { bin - nfft };
        signed * self.input_rate / nfft
    }

    fn extract(&self, burst: &ActiveBurst) -> Vec<WidebandBurst> {
        let frame_len = self.nfft as u64;
        let pre = (PRE_S * self.input_rate) as u64;
        let post = (POST_S * self.input_rate) as u64;
        let s0 = (burst.start_frame * frame_len)
            .saturating_sub(pre)
            .max(self.start_abs);
        let s1 = ((burst.last_frame + 1) * frame_len + post)
            .min(self.start_abs + self.buf.len() as u64);
        if s1 <= s0 {
            return Vec::new();
        }
        let offset = self.offset_hz(burst.bin);
        let Ok(mut ddc) = ChannelDdc::new(self.input_rate, CHANNEL_RATE, offset, PASSBAND_HZ)
        else {
            return Vec::new();
        };
        let mut chan = Vec::new();
        ddc.process(
            &self.buf[(s0 - self.start_abs) as usize..(s1 - self.start_abs) as usize],
            &mut chan,
        );
        let filtered = matched_filter(&chan, &self.mf_taps);
        let mut out = demod_channel(chan, offset);
        out.extend(demod_channel(filtered, offset));
        out
    }
}

fn noise_floor(chan: &[Complex<f32>]) -> f32 {
    if chan.is_empty() {
        return 1.0;
    }
    let mut powers: Vec<f32> = chan.iter().map(|s| s.norm_sqr()).collect();
    let k = powers.len() / 5;
    powers.select_nth_unstable_by(k, |a, b| a.total_cmp(b));
    powers[k]
}

fn demod_channel(mut chan: Vec<Complex<f32>>, offset: f64) -> Vec<WidebandBurst> {
    let noise = noise_floor(&chan);
    chan.resize(
        chan.len() + (CHANNEL_RATE * QUIET_TAIL_S) as usize,
        Complex::default(),
    );
    let mut demod = IridiumDemod::new(CHANNEL_RATE);
    demod.seed_noise(noise);
    demod
        .acquire_multi(&chan, PRE_S * CHANNEL_RATE)
        .into_iter()
        .map(|b| WidebandBurst {
            offset_hz: offset + b.cfo_hz,
            bits: b.bits,
        })
        .collect()
}
