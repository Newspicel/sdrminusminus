use std::f64::consts::TAU;

use num_complex::Complex;

use crate::{fastmath::fast_power_db, fft::FftPair};

pub const STITCH_FFT: usize = 2048;
pub const STITCH_KEEP: f64 = 0.85;

const MIN_LANES: usize = 2;
const MAX_LANES: usize = 16;
const RAMP: f64 = 0.05;
const DC_CORNER: f64 = 1e-5;
const MATCH_SMOOTHING: f32 = 0.2;
const MATCH_COHERENCE: f32 = 0.3;
const HOP: usize = STITCH_FFT / 2;
const MIN_OVERLAP: f64 = 0.02;
const MAX_SHIFT_BINS: f64 = 1e12;
const WARMUP_FRAMES: u64 = 16;
const UPDATE_FRAMES: u64 = 64;
const LEVEL_SMOOTHING: f32 = 0.125;
const FLOOR_STEP: f32 = 0.02;
const FLOOR_FALL_DB: f32 = 3.0;
const FLOOR_RISE_DB: f32 = 1.0;
const MEDIAN_BINS: usize = 33;
const FLATTEN_DB: f32 = 6.0;
const MAX_EQUALISE_DB: f32 = 30.0;
const MAX_QUALITY_DB: f32 = 60.0;
const SPUR_ABOVE_DB: f32 = 12.0;
const PARTNER_ABOVE_DB: f32 = 3.0;
const SPUR_FRAMES: u32 = 8;
const SPUR_WINDOW: u16 = (1 << 10) - 1;
const POWER_FLOOR: f32 = 1e-30;
const POWER_FLOOR_DB: f32 = -300.0;
const DEAD_DB: f32 = -250.0;

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum StitchError {
    #[error("a stitch takes {MIN_LANES} to {MAX_LANES} lanes, not {0}")]
    Lanes(usize),
    #[error("the input rate must be positive and finite, got {0} Hz")]
    Rate(f64),
    #[error("expected {expected} lanes, got {got}")]
    LaneCount { expected: usize, got: usize },
    #[error("lane {lane} holds {got} samples, lane 0 holds {expected}")]
    LaneLength {
        lane: usize,
        expected: usize,
        got: usize,
    },
    #[error("lane offset {0} Hz is out of range")]
    Offset(f64),
    #[error("lanes {0} and {1} do not overlap")]
    NoOverlap(usize, usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StitchOptions {
    pub equalise: bool,
    pub flatten: bool,
    pub snr_blend: bool,
    pub spur_reject: bool,
    pub match_phase: bool,
}

impl StitchOptions {
    const fn tracks_floor(self) -> bool {
        self.equalise || self.snr_blend || self.spur_reject
    }
}

impl Default for StitchOptions {
    fn default() -> Self {
        Self {
            equalise: true,
            flatten: true,
            snr_blend: true,
            spur_reject: true,
            match_phase: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LaneState {
    pub gain_db: f32,
    pub noise_db: f32,
    pub phase_deg: f32,
    pub coherence: Option<f32>,
    pub spur_bins: u32,
}

fn step_bins(lanes: usize) -> i64 {
    let bins = (STITCH_KEEP * STITCH_FFT as f64).round() as i64;
    if lanes.is_multiple_of(2) && bins % 2 == 1 {
        bins - 1
    } else {
        bins
    }
}

#[must_use]
pub fn auto_offsets(lanes: usize, input_rate: f64) -> Vec<f64> {
    let bin_hz = input_rate / STITCH_FFT as f64;
    let step = step_bins(lanes);
    (0..lanes)
        .map(|lane| {
            let twice = 2 * lane as i64 - (lanes as i64 - 1);
            (twice * step) as f64 / 2.0 * bin_hz
        })
        .collect()
}

#[must_use]
pub fn output_rate(lanes: usize, input_rate: f64) -> f64 {
    lanes as f64 * input_rate
}

fn lane_mask(bin: i64) -> f32 {
    let f = (bin as f64 / STITCH_FFT as f64).abs();
    let flat = STITCH_KEEP / 2.0 - RAMP / 2.0;
    if f <= flat {
        1.0
    } else if f >= flat + RAMP {
        0.0
    } else {
        (0.5 * (1.0 + (std::f64::consts::PI * (f - flat) / RAMP).cos())) as f32
    }
}

fn signed_bin(index: usize, len: usize) -> i64 {
    if index < len / 2 {
        index as i64
    } else {
        index as i64 - len as i64
    }
}

fn power_db(power: f32) -> f32 {
    fast_power_db(power.max(POWER_FLOOR))
}

fn amplitude_of_db(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn power_of_db(db: f32) -> f32 {
    10f32.powf(db / 10.0)
}

fn median(values: &mut [f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    let middle = values.len() / 2;
    let (_, value, _) = values.select_nth_unstable_by(middle, f32::total_cmp);
    Some(*value)
}

fn remember(seen: u16, now: bool) -> u16 {
    ((seen << 1) | u16::from(now)) & SPUR_WINDOW
}

#[derive(Clone, Copy)]
struct Tap {
    lane_bin: usize,
    out_bin: usize,
    mask: f32,
    gain: f32,
    quality: f32,
    share: f32,
    weight: f32,
}

impl Tap {
    fn new(lane_bin: usize, out_bin: usize, mask: f32) -> Self {
        Self {
            lane_bin,
            out_bin,
            mask,
            gain: 1.0,
            quality: 1.0,
            share: mask,
            weight: 0.0,
        }
    }
}

#[derive(Clone, Copy)]
struct Overlap {
    low_bin: usize,
    high_bin: usize,
    low_seen: u16,
    high_seen: u16,
}

#[derive(Clone, Copy, Default)]
struct Match {
    low: usize,
    high: usize,
    start: usize,
    end: usize,
    cross: Complex<f32>,
    low_power: f32,
    high_power: f32,
    gain: Complex<f32>,
    coherence: Option<f32>,
}

impl Match {
    fn fresh(low: usize, high: usize) -> Self {
        Self {
            low,
            high,
            gain: Complex::new(1.0, 0.0),
            ..Self::default()
        }
    }

    fn update(&mut self, cross: Complex<f32>, low_power: f32, high_power: f32) {
        let keep = 1.0 - MATCH_SMOOTHING;
        self.cross = self.cross * keep + cross * MATCH_SMOOTHING;
        self.low_power = self.low_power * keep + low_power * MATCH_SMOOTHING;
        self.high_power = self.high_power * keep + high_power * MATCH_SMOOTHING;
        let product = self.low_power * self.high_power;
        if product <= f32::MIN_POSITIVE {
            return;
        }
        let coherence = self.cross.norm_sqr() / product;
        self.coherence = Some(coherence);
        if coherence > MATCH_COHERENCE {
            self.gain = self.cross / self.high_power;
        }
    }

    fn step(&self, phase_only: bool) -> Complex<f32> {
        if !phase_only {
            return self.gain;
        }
        let norm = self.gain.norm();
        if norm > f32::MIN_POSITIVE {
            self.gain / norm
        } else {
            Complex::new(1.0, 0.0)
        }
    }
}

struct SortedWindow {
    values: [f32; MEDIAN_BINS],
    len: usize,
}

impl SortedWindow {
    const fn new() -> Self {
        Self {
            values: [0.0; MEDIAN_BINS],
            len: 0,
        }
    }

    fn position(&self, value: f32) -> usize {
        self.values[..self.len].partition_point(|held| held.total_cmp(&value).is_lt())
    }

    fn insert(&mut self, value: f32) {
        let at = self.position(value);
        self.values.copy_within(at..self.len, at + 1);
        self.values[at] = value;
        self.len += 1;
    }

    fn remove(&mut self, value: f32) {
        let at = self.position(value);
        self.values.copy_within(at + 1..self.len, at);
        self.len -= 1;
    }

    fn median(&self) -> f32 {
        self.values[self.len / 2]
    }
}

struct Floor {
    level: Vec<f32>,
    level_db: Vec<f32>,
    floor_db: Vec<f32>,
    smooth_db: Vec<f32>,
    spur: Vec<bool>,
}

impl Floor {
    fn new() -> Self {
        Self {
            level: vec![0.0; STITCH_FFT],
            level_db: vec![POWER_FLOOR_DB; STITCH_FFT],
            floor_db: vec![POWER_FLOOR_DB; STITCH_FFT],
            smooth_db: vec![POWER_FLOOR_DB; STITCH_FFT],
            spur: vec![false; STITCH_FFT],
        }
    }

    fn track(&mut self, spectrum: &[Complex<f32>], frame: u64) {
        if frame < WARMUP_FRAMES {
            self.warm(spectrum, frame);
            return;
        }
        for (level, bin) in self.level.iter_mut().zip(spectrum) {
            *level += (bin.norm_sqr() - *level) * LEVEL_SMOOTHING;
        }
        for (level_db, level) in self.level_db.iter_mut().zip(&self.level) {
            *level_db = power_db(*level);
        }
        for (floor_db, level_db) in self.floor_db.iter_mut().zip(&self.level_db) {
            *floor_db += FLOOR_STEP * (*level_db - *floor_db).clamp(-FLOOR_FALL_DB, FLOOR_RISE_DB);
        }
    }

    fn warm(&mut self, spectrum: &[Complex<f32>], frame: u64) {
        let weight = 1.0 / (frame + 1) as f32;
        for (level, bin) in self.level.iter_mut().zip(spectrum) {
            *level += (bin.norm_sqr() - *level) * weight;
        }
        if frame + 1 < WARMUP_FRAMES {
            return;
        }
        let bins = self
            .level
            .iter()
            .zip(&mut self.level_db)
            .zip(&mut self.floor_db);
        for ((level, level_db), floor_db) in bins {
            *level_db = power_db(*level);
            *floor_db = *level_db;
        }
    }

    fn smooth(&mut self, ordered: &mut [f32]) {
        let half = STITCH_FFT / 2;
        for (index, slot) in ordered.iter_mut().enumerate() {
            *slot = self.floor_db[(index + half) % STITCH_FFT];
        }
        let reach = MEDIAN_BINS / 2;
        let mut window = SortedWindow::new();
        for value in ordered.iter().take(reach) {
            window.insert(*value);
        }
        for index in 0..ordered.len() {
            if let Some(outgoing) = index.checked_sub(reach + 1) {
                window.remove(ordered[outgoing]);
            }
            if let Some(incoming) = ordered.get(index + reach) {
                window.insert(*incoming);
            }
            self.smooth_db[(index + half) % STITCH_FFT] = window.median();
        }
    }

    fn above_db(&self, bin: usize) -> f32 {
        self.level_db[bin] - self.smooth_db[bin]
    }
}

struct Lane {
    offset_hz: f64,
    shift_bins: i64,
    out_shift: i64,
    residual: Complex<f64>,
    phasor: Complex<f64>,
    dc: Complex<f32>,
    frame: Vec<Complex<f32>>,
    spectrum: Vec<Complex<f32>>,
    taps: Vec<Tap>,
    correction: Complex<f32>,
    floor: Floor,
    noise_db: f32,
    gain_db: f32,
    spur_bins: u32,
}

impl Lane {
    fn new() -> Self {
        Self {
            offset_hz: f64::NAN,
            shift_bins: 0,
            out_shift: 0,
            residual: Complex::new(1.0, 0.0),
            phasor: Complex::new(1.0, 0.0),
            dc: Complex::default(),
            frame: vec![Complex::default(); STITCH_FFT],
            spectrum: vec![Complex::default(); STITCH_FFT],
            taps: Vec::with_capacity(STITCH_FFT),
            correction: Complex::new(1.0, 0.0),
            floor: Floor::new(),
            noise_db: POWER_FLOOR_DB,
            gain_db: 0.0,
            spur_bins: 0,
        }
    }

    fn condition(&mut self, input: &[Complex<f32>], at: usize, dc_alpha: f32) {
        for (slot, sample) in self.frame[at..].iter_mut().zip(input) {
            self.dc += (*sample - self.dc) * dc_alpha;
            let clean = *sample - self.dc;
            let turn = Complex::new(self.phasor.re as f32, self.phasor.im as f32);
            *slot = clean * turn;
            self.phasor *= self.residual;
        }
        self.phasor /= self.phasor.norm();
    }

    fn rotation(&self, frame_start: u64) -> Complex<f32> {
        let len = STITCH_FFT as i64;
        let start = (frame_start % STITCH_FFT as u64) as i64;
        let turns = (self.out_shift.rem_euclid(len) * start).rem_euclid(len);
        let phase = TAU * turns as f64 / STITCH_FFT as f64;
        Complex::from_polar(1.0, phase as f32)
    }

    fn flag_spur(&mut self, bin: usize) {
        if !self.floor.spur[bin] {
            self.floor.spur[bin] = true;
            self.spur_bins += 1;
        }
    }

    fn forget_estimates(&mut self) {
        self.gain_db = 0.0;
        self.noise_db = POWER_FLOOR_DB;
        self.spur_bins = 0;
        self.floor.spur.fill(false);
    }
}

#[derive(Clone, Copy)]
struct Shaping {
    options: StitchOptions,
    ready: bool,
    reference_db: f32,
}

impl Shaping {
    fn gain_db(self, lane_gain_db: f32, floor_db: f32, live: bool) -> f32 {
        if self.ready && live && self.options.equalise && self.options.flatten {
            (self.reference_db - floor_db)
                .clamp(lane_gain_db - FLATTEN_DB, lane_gain_db + FLATTEN_DB)
        } else {
            lane_gain_db
        }
    }

    fn quality(self, floor_db: f32, equalised_db: f32) -> f32 {
        if !self.ready || !self.options.snr_blend {
            return 1.0;
        }
        if floor_db <= DEAD_DB {
            return 0.0;
        }
        power_of_db((self.reference_db - equalised_db).clamp(-MAX_QUALITY_DB, MAX_QUALITY_DB))
    }
}

pub struct Stitcher {
    input_rate: f64,
    out_len: usize,
    dc_alpha: f32,
    options: StitchOptions,
    lanes: Vec<Lane>,
    order: Vec<usize>,
    matches: Vec<Match>,
    previous: Vec<Match>,
    overlaps: Vec<Overlap>,
    gap: Option<(usize, usize)>,
    center_bins: i64,
    sums: Vec<f32>,
    blend_sums: Vec<f32>,
    ordered: Vec<f32>,
    scratch: Vec<f32>,
    output: Vec<Complex<f32>>,
    lane_fft: FftPair,
    out_fft: FftPair,
    fill: usize,
    frame_start: u64,
    tracked: u64,
    ready: bool,
    reference_db: f32,
}

impl Stitcher {
    pub fn new(lanes: usize, input_rate: f64) -> Result<Self, StitchError> {
        if !(MIN_LANES..=MAX_LANES).contains(&lanes) {
            return Err(StitchError::Lanes(lanes));
        }
        if !input_rate.is_finite() || input_rate <= 0.0 {
            return Err(StitchError::Rate(input_rate));
        }
        let out_len = STITCH_FFT * lanes;
        let mut stitcher = Self {
            input_rate,
            out_len,
            dc_alpha: (1.0 - (-TAU * DC_CORNER).exp()) as f32,
            options: StitchOptions::default(),
            lanes: (0..lanes).map(|_| Lane::new()).collect(),
            order: (0..lanes).collect(),
            matches: Vec::with_capacity(lanes),
            previous: Vec::with_capacity(lanes),
            overlaps: Vec::with_capacity((lanes - 1) * STITCH_FFT),
            gap: None,
            center_bins: 0,
            sums: vec![0.0; out_len],
            blend_sums: vec![0.0; out_len],
            ordered: vec![0.0; STITCH_FFT],
            scratch: Vec::with_capacity(STITCH_FFT),
            output: vec![Complex::default(); out_len],
            lane_fft: FftPair::new(STITCH_FFT),
            out_fft: FftPair::new(out_len),
            fill: 0,
            frame_start: 0,
            tracked: 0,
            ready: false,
            reference_db: 0.0,
        };
        stitcher.retune(&auto_offsets(lanes, input_rate))?;
        Ok(stitcher)
    }

    #[must_use]
    pub fn output_rate(&self) -> f64 {
        output_rate(self.lanes.len(), self.input_rate)
    }

    #[must_use]
    pub fn output_center_offset_hz(&self) -> f64 {
        self.center_bins as f64 * self.bin_hz()
    }

    #[must_use]
    pub fn lane_state(&self, lane: usize) -> LaneState {
        let Some(state) = self.lanes.get(lane) else {
            return LaneState::default();
        };
        let magnitude = state.correction.norm();
        let matched_db = if magnitude > f32::MIN_POSITIVE {
            20.0 * magnitude.log10()
        } else {
            0.0
        };
        LaneState {
            gain_db: state.gain_db + matched_db,
            noise_db: state.noise_db,
            phase_deg: state.correction.arg().to_degrees(),
            coherence: self
                .matches
                .iter()
                .find(|pair| pair.high == lane)
                .and_then(|pair| pair.coherence),
            spur_bins: state.spur_bins,
        }
    }

    pub fn set_options(&mut self, options: StitchOptions) {
        if options.tracks_floor() && !self.options.tracks_floor() {
            self.restart_floors();
        }
        if !options.tracks_floor() {
            self.ready = false;
        }
        self.options = options;
        self.update_weights();
    }

    pub fn retune(&mut self, offsets_hz: &[f64]) -> Result<(), StitchError> {
        if offsets_hz.len() != self.lanes.len() {
            return Err(StitchError::LaneCount {
                expected: self.lanes.len(),
                got: offsets_hz.len(),
            });
        }
        let bin_hz = self.bin_hz();
        if let Some(offset) = offsets_hz
            .iter()
            .find(|offset| !offset.is_finite() || (*offset / bin_hz).abs() > MAX_SHIFT_BINS)
        {
            return Err(StitchError::Offset(*offset));
        }
        let mut changed = [false; MAX_LANES];
        for (index, (lane, offset)) in self.lanes.iter_mut().zip(offsets_hz).enumerate() {
            if lane.offset_hz.to_bits() == offset.to_bits() {
                continue;
            }
            changed[index] = true;
            lane.offset_hz = *offset;
            lane.shift_bins = (offset / bin_hz).round() as i64;
            let residual = offset - lane.shift_bins as f64 * bin_hz;
            lane.residual = Complex::from_polar(1.0, TAU * residual / self.input_rate);
        }
        if changed.iter().any(|flag| *flag) {
            self.centre_lanes();
            self.place_taps();
            self.pair_lanes(&changed);
            self.restart_floors();
            self.update_weights();
        }
        self.gap
            .map_or(Ok(()), |(low, high)| Err(StitchError::NoOverlap(low, high)))
    }

    pub fn reset(&mut self) {
        self.fill = 0;
        self.frame_start = 0;
        for lane in &mut self.lanes {
            lane.phasor = Complex::new(1.0, 0.0);
            lane.dc = Complex::default();
        }
        self.restart_floors();
        self.update_weights();
    }

    pub fn process(
        &mut self,
        lanes: &[&[Complex<f32>]],
        out: &mut Vec<Complex<f32>>,
    ) -> Result<usize, StitchError> {
        if lanes.len() != self.lanes.len() {
            return Err(StitchError::LaneCount {
                expected: self.lanes.len(),
                got: lanes.len(),
            });
        }
        let len = lanes.first().map_or(0, |lane| lane.len());
        if let Some((lane, input)) = lanes
            .iter()
            .enumerate()
            .find(|(_, input)| input.len() != len)
        {
            return Err(StitchError::LaneLength {
                lane,
                expected: len,
                got: input.len(),
            });
        }
        let before = out.len();
        let mut at = 0;
        while at < len {
            let take = (STITCH_FFT - self.fill).min(len - at);
            for (lane, input) in self.lanes.iter_mut().zip(lanes) {
                lane.condition(&input[at..at + take], self.fill, self.dc_alpha);
            }
            self.fill += take;
            at += take;
            if self.fill == STITCH_FFT {
                self.run_frame(out);
            }
        }
        Ok(out.len() - before)
    }

    fn bin_hz(&self) -> f64 {
        self.input_rate / STITCH_FFT as f64
    }

    fn centre_lanes(&mut self) {
        let shifts = self.lanes.iter().map(|lane| lane.shift_bins);
        let low = shifts.clone().min().unwrap_or(0);
        let high = shifts.max().unwrap_or(0);
        self.center_bins = (low + high).div_euclid(2);
        for lane in &mut self.lanes {
            lane.out_shift = lane.shift_bins - self.center_bins;
        }
    }

    fn place_taps(&mut self) {
        let half = self.out_len as i64 / 2;
        let out_len = self.out_len as i64;
        for lane in &mut self.lanes {
            lane.taps.clear();
            for lane_bin in 0..STITCH_FFT {
                let bin = signed_bin(lane_bin, STITCH_FFT);
                let mask = lane_mask(bin);
                let target = bin + lane.out_shift;
                if mask <= 0.0 || target < -half || target >= half {
                    continue;
                }
                let out_bin = target.rem_euclid(out_len) as usize;
                lane.taps.push(Tap::new(lane_bin, out_bin, mask));
            }
        }
    }

    fn pair_lanes(&mut self, changed: &[bool; MAX_LANES]) {
        let lanes = &self.lanes;
        self.order
            .sort_unstable_by(|a, b| lanes[*a].offset_hz.total_cmp(&lanes[*b].offset_hz));
        self.previous.clear();
        self.previous.extend_from_slice(&self.matches);
        self.matches.clear();
        self.overlaps.clear();
        self.gap = None;
        for window in 0..self.order.len().saturating_sub(1) {
            let (low, high) = (self.order[window], self.order[window + 1]);
            let start = self.overlaps.len();
            let shared = self.find_overlap(low, high);
            if (shared as f64) < MIN_OVERLAP * STITCH_FFT as f64 && self.gap.is_none() {
                self.gap = Some((low, high));
            }
            if shared == 0 {
                continue;
            }
            let kept = self
                .previous
                .iter()
                .find(|old| old.low == low && old.high == high && !changed[low] && !changed[high])
                .copied()
                .unwrap_or_else(|| Match::fresh(low, high));
            self.matches.push(Match {
                start,
                end: start + shared,
                ..kept
            });
        }
    }

    fn find_overlap(&mut self, low: usize, high: usize) -> usize {
        self.sums.fill(-1.0);
        for tap in &self.lanes[low].taps {
            self.sums[tap.out_bin] = tap.lane_bin as f32;
        }
        let start = self.overlaps.len();
        for tap in &self.lanes[high].taps {
            let found = self.sums[tap.out_bin];
            if found >= 0.0 {
                self.overlaps.push(Overlap {
                    low_bin: found as usize,
                    high_bin: tap.lane_bin,
                    low_seen: 0,
                    high_seen: 0,
                });
            }
        }
        self.overlaps.len() - start
    }

    fn restart_floors(&mut self) {
        self.tracked = 0;
        self.ready = false;
        for entry in &mut self.overlaps {
            entry.low_seen = 0;
            entry.high_seen = 0;
        }
    }

    fn update_weights(&mut self) {
        if self.ready {
            self.estimate_noise();
            self.flag_spurs();
        } else {
            for lane in &mut self.lanes {
                lane.forget_estimates();
            }
        }
        self.shape_taps();
        self.normalise_taps();
    }

    fn estimate_noise(&mut self) {
        for lane in &mut self.lanes {
            lane.floor.smooth(&mut self.ordered);
            self.scratch.clear();
            self.scratch.extend(
                lane.taps
                    .iter()
                    .map(|tap| lane.floor.smooth_db[tap.lane_bin]),
            );
            lane.noise_db = median(&mut self.scratch).unwrap_or(POWER_FLOOR_DB);
        }
        let (sum, count) = self
            .lanes
            .iter()
            .filter(|lane| lane.noise_db > DEAD_DB)
            .fold((0.0f32, 0u32), |(sum, count), lane| {
                (sum + lane.noise_db, count + 1)
            });
        self.reference_db = if count == 0 { 0.0 } else { sum / count as f32 };
        let equalise = self.options.equalise;
        for lane in &mut self.lanes {
            lane.gain_db = if equalise && lane.noise_db > DEAD_DB {
                (self.reference_db - lane.noise_db).clamp(-MAX_EQUALISE_DB, MAX_EQUALISE_DB)
            } else {
                0.0
            };
        }
    }

    fn flag_spurs(&mut self) {
        let (lanes, matches, overlaps) = (&mut self.lanes, &self.matches, &self.overlaps);
        for lane in lanes.iter_mut() {
            lane.spur_bins = 0;
            lane.floor.spur.fill(false);
        }
        if !self.options.spur_reject {
            return;
        }
        for pair in matches {
            for entry in &overlaps[pair.start..pair.end] {
                if entry.low_seen.count_ones() >= SPUR_FRAMES {
                    lanes[pair.low].flag_spur(entry.low_bin);
                }
                if entry.high_seen.count_ones() >= SPUR_FRAMES {
                    lanes[pair.high].flag_spur(entry.high_bin);
                }
            }
        }
    }

    fn shape_taps(&mut self) {
        let shaping = Shaping {
            options: self.options,
            ready: self.ready,
            reference_db: self.reference_db,
        };
        for lane in &mut self.lanes {
            let live = lane.noise_db > DEAD_DB;
            for tap in &mut lane.taps {
                let floor_db = lane.floor.smooth_db[tap.lane_bin];
                let gain_db = shaping.gain_db(lane.gain_db, floor_db, live);
                tap.gain = amplitude_of_db(gain_db);
                tap.quality = if lane.floor.spur[tap.lane_bin] {
                    0.0
                } else {
                    shaping.quality(floor_db, floor_db + gain_db)
                };
            }
        }
    }

    fn normalise_taps(&mut self) {
        self.sums.fill(0.0);
        self.blend_sums.fill(0.0);
        for tap in self.lanes.iter().flat_map(|lane| &lane.taps) {
            self.sums[tap.out_bin] += tap.mask;
            self.blend_sums[tap.out_bin] += tap.mask * tap.quality;
        }
        let scale = 1.0 / STITCH_FFT as f32;
        for tap in self.lanes.iter_mut().flat_map(|lane| &mut lane.taps) {
            let (mask_sum, blend_sum) = (self.sums[tap.out_bin], self.blend_sums[tap.out_bin]);
            tap.share = if blend_sum > f32::MIN_POSITIVE {
                mask_sum.min(1.0) * tap.mask * tap.quality / blend_sum
            } else {
                tap.mask / mask_sum.max(1.0)
            };
            tap.weight = tap.share * tap.gain * scale;
        }
    }

    fn run_frame(&mut self, out: &mut Vec<Complex<f32>>) {
        for lane in &mut self.lanes {
            lane.spectrum.copy_from_slice(&lane.frame);
            self.lane_fft.forward(&mut lane.spectrum);
            lane.frame.copy_within(HOP.., 0);
        }
        self.fill = STITCH_FFT - HOP;
        self.match_lanes();
        if self.options.tracks_floor() {
            self.track_floors();
        }
        self.synthesise();
        let quarter = self.out_len / 4;
        out.extend_from_slice(&self.output[quarter..self.out_len - quarter]);
        self.frame_start += HOP as u64;
    }

    fn match_lanes(&mut self) {
        let start = self.frame_start;
        let (lanes, overlaps) = (&self.lanes, &self.overlaps);
        for pair in &mut self.matches {
            let (low, high) = (&lanes[pair.low], &lanes[pair.high]);
            let (low_turn, high_turn) = (low.rotation(start), high.rotation(start));
            let mut cross = Complex::default();
            let (mut low_power, mut high_power) = (0.0f32, 0.0f32);
            for entry in &overlaps[pair.start..pair.end] {
                let a = low.spectrum[entry.low_bin] * low_turn;
                let b = high.spectrum[entry.high_bin] * high_turn;
                cross += a * b.conj();
                low_power += a.norm_sqr();
                high_power += b.norm_sqr();
            }
            pair.update(cross, low_power, high_power);
        }
        self.chain_corrections();
    }

    fn chain_corrections(&mut self) {
        for lane in &mut self.lanes {
            lane.correction = Complex::new(1.0, 0.0);
        }
        if !self.options.match_phase {
            return;
        }
        let phase_only = self.options.equalise;
        for pair in &self.matches {
            let base = self.lanes[pair.low].correction;
            self.lanes[pair.high].correction = base * pair.step(phase_only);
        }
    }

    fn track_floors(&mut self) {
        for lane in &mut self.lanes {
            lane.floor.track(&lane.spectrum, self.tracked);
        }
        if self.ready {
            self.watch_spurs();
        }
        self.tracked += 1;
        if self.tracked.is_multiple_of(UPDATE_FRAMES) {
            self.ready = true;
            self.update_weights();
        }
    }

    fn watch_spurs(&mut self) {
        let (lanes, matches, overlaps) = (&self.lanes, &self.matches, &mut self.overlaps);
        for pair in matches {
            let (low, high) = (&lanes[pair.low].floor, &lanes[pair.high].floor);
            for entry in &mut overlaps[pair.start..pair.end] {
                let low_above = low.above_db(entry.low_bin);
                let high_above = high.above_db(entry.high_bin);
                entry.low_seen = remember(
                    entry.low_seen,
                    low_above > SPUR_ABOVE_DB && high_above < PARTNER_ABOVE_DB,
                );
                entry.high_seen = remember(
                    entry.high_seen,
                    high_above > SPUR_ABOVE_DB && low_above < PARTNER_ABOVE_DB,
                );
            }
        }
    }

    fn synthesise(&mut self) {
        self.output.fill(Complex::default());
        for lane in &self.lanes {
            let factor = lane.rotation(self.frame_start) * lane.correction;
            for tap in &lane.taps {
                self.output[tap.out_bin] += lane.spectrum[tap.lane_bin] * factor * tap.weight;
            }
        }
        self.out_fft.inverse(&mut self.output);
    }
}

#[cfg(test)]
mod tests {
    use std::{f64::consts::PI, ops::RangeInclusive};

    use super::*;

    const RATE: f64 = 2048.0;
    const PLAIN: StitchOptions = StitchOptions {
        equalise: false,
        flatten: false,
        snr_blend: false,
        spur_reject: false,
        match_phase: true,
    };
    const EQUALISE: StitchOptions = StitchOptions {
        equalise: true,
        ..PLAIN
    };
    const FLATTEN: StitchOptions = StitchOptions {
        flatten: true,
        ..EQUALISE
    };
    const SNR_BLEND: StitchOptions = StitchOptions {
        snr_blend: true,
        ..PLAIN
    };
    const SPUR_REJECT: StitchOptions = StitchOptions {
        spur_reject: true,
        ..PLAIN
    };

    fn plain(lanes: usize) -> Stitcher {
        with_options(lanes, PLAIN)
    }

    fn with_options(lanes: usize, options: StitchOptions) -> Stitcher {
        let mut stitcher = Stitcher::new(lanes, RATE).expect("stitcher");
        stitcher.set_options(options);
        stitcher
    }

    fn tone(len: usize, hz: f64, gain: Complex<f32>) -> Vec<Complex<f32>> {
        (0..len)
            .map(|n| gain * Complex::from_polar(1.0, (TAU * hz * n as f64 / RATE) as f32))
            .collect()
    }

    fn lanes_hearing(
        hz: f64,
        offsets: &[f64],
        len: usize,
        gains: &[Complex<f32>],
    ) -> Vec<Vec<Complex<f32>>> {
        offsets
            .iter()
            .zip(gains)
            .map(|(offset, gain)| tone(len, hz - offset, *gain))
            .collect()
    }

    fn stitch(stitcher: &mut Stitcher, lanes: &[Vec<Complex<f32>>]) -> Vec<Complex<f32>> {
        let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
        let mut out = Vec::new();
        let written = stitcher.process(&views, &mut out).expect("lanes");
        assert_eq!(written, out.len());
        out
    }

    fn fit(out: &[Complex<f32>], hz: f64, lanes: usize, skip: usize) -> (Complex<f32>, f32) {
        let rate = RATE * lanes as f64;
        let origin = (STITCH_FFT / 4) as f64 / RATE;
        let expected = |j: usize| {
            let t = origin + j as f64 / rate;
            Complex::from_polar(1.0f32, (TAU * hz * t) as f32)
        };
        let span = &out[skip..];
        let amplitude = span
            .iter()
            .enumerate()
            .map(|(j, y)| y * expected(j + skip).conj())
            .sum::<Complex<f32>>()
            / span.len() as f32;
        let residual = span
            .iter()
            .enumerate()
            .map(|(j, y)| (y - amplitude * expected(j + skip)).norm_sqr())
            .sum::<f32>()
            / span.len() as f32;
        (amplitude, residual)
    }

    fn noise(len: usize, seed: u32) -> Vec<Complex<f32>> {
        let mut state = seed;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as f32 / u32::MAX as f32 - 0.5
        };
        (0..len).map(|_| Complex::new(next(), next())).collect()
    }

    fn louder(samples: Vec<Complex<f32>>, gain: f32) -> Vec<Complex<f32>> {
        samples.into_iter().map(|sample| sample * gain).collect()
    }

    fn rolled_off_db(bin: i64) -> f64 {
        -3.0 * (bin as f64 / 870.0).powi(4)
    }

    fn multitone(offsets: &[f64], len: usize, gains: &[Complex<f32>]) -> Vec<Vec<Complex<f32>>> {
        let mut fft = FftPair::new(STITCH_FFT);
        offsets
            .iter()
            .zip(gains)
            .map(|(offset, gain)| {
                let mut period: Vec<Complex<f32>> = (0..STITCH_FFT)
                    .map(|index| {
                        let bin = signed_bin(index, STITCH_FFT);
                        let absolute = bin as f64 + offset;
                        let phase = PI * absolute * absolute / STITCH_FFT as f64;
                        let tone =
                            Complex::from_polar(10f64.powf(rolled_off_db(bin) / 20.0), phase);
                        gain * Complex::new(tone.re as f32, tone.im as f32)
                    })
                    .collect();
                fft.inverse_scaled(&mut period);
                (0..len).map(|n| period[n % STITCH_FFT]).collect()
            })
            .collect()
    }

    fn psd(samples: &[Complex<f32>], size: usize) -> Vec<f64> {
        let window: Vec<f32> = (0..size)
            .map(|i| (0.5 - 0.5 * (TAU * i as f64 / size as f64).cos()) as f32)
            .collect();
        let mut fft = FftPair::new(size);
        let mut power = vec![0.0f64; size];
        let mut chunk = vec![Complex::default(); size];
        let mut count = 0.0;
        for start in (0..=samples.len() - size).step_by(size / 2) {
            for ((slot, sample), weight) in chunk.iter_mut().zip(&samples[start..]).zip(&window) {
                *slot = sample * weight;
            }
            fft.forward(&mut chunk);
            for (sum, value) in power.iter_mut().zip(&chunk) {
                *sum += f64::from(value.norm_sqr());
            }
            count += 1.0;
        }
        power.iter().map(|sum| sum / count).collect()
    }

    fn band_db(power: &[f64], bins: RangeInclusive<i64>) -> f64 {
        let len = power.len() as i64;
        let (sum, count) = bins.fold((0.0, 0.0), |(sum, count), bin| {
            (sum + power[bin.rem_euclid(len) as usize], count + 1.0)
        });
        10.0 * (sum / count).log10()
    }

    fn share_at_centre(stitcher: &Stitcher, lane: usize) -> f32 {
        stitcher.lanes[lane]
            .taps
            .iter()
            .find(|tap| tap.out_bin == 0)
            .map_or(0.0, |tap| tap.share)
    }

    fn share_sums(stitcher: &Stitcher) -> Vec<f32> {
        let mut sums = vec![0.0; stitcher.out_len];
        for tap in stitcher.lanes.iter().flat_map(|lane| &lane.taps) {
            sums[tap.out_bin] += tap.share;
        }
        sums
    }

    #[test]
    fn a_tone_in_one_lane_lands_where_it_belongs() {
        let offsets = auto_offsets(3, RATE);
        let hz = offsets[2] + 100.0;
        let mut stitcher = plain(3);
        let heard = [
            Complex::default(),
            Complex::default(),
            Complex::new(1.0, 0.0),
        ];
        let out = stitch(&mut stitcher, &lanes_hearing(hz, &offsets, 20_000, &heard));
        let (amplitude, residual) = fit(&out, hz, 3, 3 * STITCH_FFT);
        assert!(
            (amplitude - Complex::new(1.0, 0.0)).norm() < 0.02,
            "{amplitude}"
        );
        assert!(residual < 1e-3, "{residual}");
    }

    #[test]
    fn a_manual_layout_off_the_bin_grid_still_lands_the_tone() {
        let offsets = [-700.3, 650.7];
        let hz = offsets[0] - 123.4;
        let mut stitcher = plain(2);
        stitcher.retune(&offsets).expect("layout");
        let heard = [Complex::new(1.0, 0.0), Complex::default()];
        let out = stitch(&mut stitcher, &lanes_hearing(hz, &offsets, 20_000, &heard));
        let centre = stitcher.output_center_offset_hz();
        let (amplitude, residual) = fit(&out, hz - centre, 2, 2 * STITCH_FFT);
        assert!(
            (amplitude - Complex::new(1.0, 0.0)).norm() < 0.02,
            "{amplitude}"
        );
        assert!(residual < 1e-3, "{residual}");
    }

    #[test]
    fn the_output_centres_between_the_outermost_lanes() {
        let offsets = [0.0, 1740.0];
        let hz = 1500.0;
        let mut stitcher = plain(2);
        stitcher.retune(&offsets).expect("layout");
        assert_eq!(stitcher.output_center_offset_hz(), 870.0);
        let heard = [Complex::default(), Complex::new(1.0, 0.0)];
        let out = stitch(&mut stitcher, &lanes_hearing(hz, &offsets, 20_000, &heard));
        let (amplitude, residual) = fit(&out, hz - 870.0, 2, 2 * STITCH_FFT);
        assert!(
            (amplitude - Complex::new(1.0, 0.0)).norm() < 0.02,
            "{amplitude}"
        );
        assert!(residual < 1e-3, "{residual}");
        assert_eq!(plain(3).output_center_offset_hz(), 0.0);
    }

    #[test]
    fn the_output_runs_at_lanes_times_the_input() {
        let mut stitcher = plain(4);
        let len = 50 * HOP;
        let out = stitch(&mut stitcher, &vec![vec![Complex::default(); len]; 4]);
        assert_eq!(out.len(), 4 * (len - HOP));
        assert_eq!(stitcher.output_rate(), 4.0 * RATE);
    }

    #[test]
    fn a_tone_in_an_overlap_survives_a_lane_with_its_own_phase_and_gain() {
        let offsets = auto_offsets(2, RATE);
        let hz = 3.0;
        let gains = [Complex::new(1.0, 0.0), Complex::from_polar(0.6, 2.3)];
        let mut stitcher = plain(2);
        let out = stitch(&mut stitcher, &lanes_hearing(hz, &offsets, 60_000, &gains));
        let skip = out.len() / 2;
        let (amplitude, residual) = fit(&out, hz, 2, skip);
        assert!(
            (amplitude - Complex::new(1.0, 0.0)).norm() < 0.02,
            "{amplitude}"
        );
        assert!(residual < 1e-3, "{residual}");
    }

    #[test]
    fn auto_offsets_sit_side_by_side_on_the_bin_grid() {
        for lanes in 2..=6 {
            let offsets = auto_offsets(lanes, RATE);
            let bin_hz = RATE / STITCH_FFT as f64;
            for (low, high) in offsets.iter().zip(offsets.iter().rev()) {
                assert!((low + high).abs() < 1e-9);
            }
            for offset in &offsets {
                assert!((offset / bin_hz - (offset / bin_hz).round()).abs() < 1e-9);
            }
            for pair in offsets.windows(2) {
                let step = pair[1] - pair[0];
                assert!(step > 0.0 && step < (STITCH_KEEP / 2.0 + RAMP / 2.0) * 2.0 * RATE);
                assert!(step >= (STITCH_KEEP - RAMP) * RATE);
            }
        }
    }

    #[test]
    fn a_gap_between_lanes_stays_empty() {
        let mut stitcher = plain(2);
        assert_eq!(
            stitcher.retune(&[-1.2 * RATE, 1.2 * RATE]),
            Err(StitchError::NoOverlap(0, 1))
        );
        let out = stitch(&mut stitcher, &[noise(30_000, 1), noise(30_000, 7)]);
        let n = 4096;
        let mut chunk: Vec<Complex<f32>> = out[out.len() - n..]
            .iter()
            .enumerate()
            .map(|(i, y)| y * (0.5 - 0.5 * (TAU * i as f64 / n as f64).cos()) as f32)
            .collect();
        FftPair::new(n).forward(&mut chunk);
        let (mut gap, mut band) = (0.0f32, 0.0f32);
        for (index, value) in chunk.iter().enumerate() {
            let f = signed_bin(index, n) as f64 / n as f64 * 2.0;
            if f.abs() < 0.6 {
                gap += value.norm_sqr();
            } else if f.abs() > 0.8 {
                band += value.norm_sqr();
            }
        }
        assert!(gap < band * 1e-4, "gap {gap} band {band}");
    }

    #[test]
    fn block_size_does_not_change_the_output() {
        let offsets = auto_offsets(3, RATE);
        let len = 150 * HOP;
        let lanes = [noise(len, 3), louder(noise(len, 5), 2.0), noise(len, 9)];
        for options in [PLAIN, StitchOptions::default()] {
            let mut whole = with_options(3, options);
            let expected = stitch(&mut whole, &lanes);
            let mut pieces = with_options(3, options);
            pieces.retune(&offsets).expect("layout");
            let mut out = Vec::new();
            for start in (0..len).step_by(777) {
                let end = (start + 777).min(len);
                let views: Vec<&[Complex<f32>]> =
                    lanes.iter().map(|lane| &lane[start..end]).collect();
                pieces.process(&views, &mut out).expect("lanes");
            }
            assert_eq!(out.len(), expected.len());
            for (a, b) in out.iter().zip(&expected) {
                assert!((a - b).norm() < 1e-5, "{options:?}");
            }
        }
    }

    #[test]
    fn bad_setups_are_refused() {
        assert_eq!(Stitcher::new(1, RATE).err(), Some(StitchError::Lanes(1)));
        assert_eq!(Stitcher::new(17, RATE).err(), Some(StitchError::Lanes(17)));
        assert!(matches!(
            Stitcher::new(2, f64::NAN).err(),
            Some(StitchError::Rate(_))
        ));
        assert_eq!(Stitcher::new(2, 0.0).err(), Some(StitchError::Rate(0.0)));
    }

    #[test]
    fn a_lane_count_mismatch_is_an_error() {
        let mut stitcher = plain(3);
        let lane = noise(4 * HOP, 2);
        let mut out = Vec::new();
        assert_eq!(
            stitcher.process(&[&lane, &lane], &mut out),
            Err(StitchError::LaneCount {
                expected: 3,
                got: 2
            })
        );
        assert!(out.is_empty());
        assert_eq!(
            stitcher.retune(&[0.0; 4]),
            Err(StitchError::LaneCount {
                expected: 3,
                got: 4
            })
        );
    }

    #[test]
    fn lanes_of_unequal_length_are_an_error() {
        let mut stitcher = plain(2);
        let lane = noise(4 * HOP, 2);
        let mut out = Vec::new();
        assert_eq!(
            stitcher.process(&[&lane, &lane[1..]], &mut out),
            Err(StitchError::LaneLength {
                lane: 1,
                expected: 4 * HOP,
                got: 4 * HOP - 1
            })
        );
        assert!(out.is_empty());
    }

    #[test]
    fn an_offset_out_of_range_is_refused_and_keeps_the_layout() {
        let mut stitcher = plain(2);
        stitcher.retune(&[0.0, 1740.0]).expect("layout");
        assert!(matches!(
            stitcher.retune(&[0.0, f64::NAN]),
            Err(StitchError::Offset(offset)) if offset.is_nan()
        ));
        assert_eq!(
            stitcher.retune(&[-1e300, 1e300]),
            Err(StitchError::Offset(-1e300))
        );
        assert_eq!(
            stitcher.retune(&[0.0, f64::INFINITY]),
            Err(StitchError::Offset(f64::INFINITY))
        );
        assert_eq!(stitcher.output_center_offset_hz(), 870.0);
        assert_eq!(stitcher.retune(&[1e9, 1e9 + 1740.0]), Ok(()));
        assert_eq!(stitcher.output_center_offset_hz(), 1e9 + 870.0);
    }

    #[test]
    fn lanes_without_overlap_are_refused() {
        let mut stitcher = plain(2);
        assert_eq!(
            stitcher.retune(&[-1.2 * RATE, 1.2 * RATE]),
            Err(StitchError::NoOverlap(0, 1))
        );
        assert_eq!(
            stitcher.retune(&[1.2 * RATE, -1.2 * RATE]),
            Err(StitchError::NoOverlap(1, 0))
        );
        assert_eq!(
            stitcher.retune(&[-907.0, 906.0]),
            Err(StitchError::NoOverlap(0, 1))
        );
        assert_eq!(stitcher.retune(&[-900.0, 900.0]), Ok(()));
        assert_eq!(stitcher.retune(&auto_offsets(2, RATE)), Ok(()));
    }

    #[test]
    fn noise_equalisation_removes_a_6_db_step_in_a_noise_only_overlap() {
        let len = 330 * HOP;
        let lanes = [noise(len, 11), louder(noise(len, 23), 2.0)];
        let step_db = |options| {
            let mut stitcher = with_options(2, options);
            let out = stitch(&mut stitcher, &lanes);
            let power = psd(&out[200 * 2 * HOP..], 1024);
            let step = band_db(&power, 25..=150) - band_db(&power, -150..=-25);
            (step, stitcher.lane_state(0), stitcher.lane_state(1))
        };
        let (before, _, _) = step_db(PLAIN);
        let (after, quiet, loud) = step_db(EQUALISE);
        assert!(before > 5.0, "{before}");
        assert!(after.abs() < 1.0, "{after}");
        assert!(
            (loud.noise_db - quiet.noise_db - 6.02).abs() < 0.3,
            "{quiet:?} {loud:?}"
        );
        assert!((quiet.gain_db - 3.01).abs() < 0.3, "{quiet:?}");
        assert!((loud.gain_db + 3.01).abs() < 0.3, "{loud:?}");
    }

    #[test]
    fn rolloff_flattening_fills_the_seam_dip() {
        let offsets = auto_offsets(2, RATE);
        let gains = [Complex::new(1.0, 0.0), Complex::from_polar(0.8, 1.1)];
        let lanes = multitone(&offsets, 260 * HOP, &gains);
        let dip = |options| {
            let mut stitcher = with_options(2, options);
            let out = stitch(&mut stitcher, &lanes);
            let start = 220 * 2 * HOP;
            let mut period = out[start..start + 2 * STITCH_FFT].to_vec();
            let len = period.len() as i64;
            FftPair::new(period.len()).forward(&mut period);
            let level = |bin: i64| {
                10.0 * f64::from(period[bin.rem_euclid(len) as usize].norm_sqr()).log10()
            };
            let mut levels: Vec<f64> = (-1600..=1600).map(level).collect();
            levels.sort_by(f64::total_cmp);
            let reference = levels[levels.len() / 2];
            let lowest = (-60..=60).map(level).fold(f64::INFINITY, f64::min);
            reference - lowest
        };
        let rolled = dip(EQUALISE);
        let flattened = dip(FLATTEN);
        assert!(rolled > 2.0, "{rolled}");
        assert!(flattened < 1.0, "{flattened}");
    }

    #[test]
    fn a_spur_in_one_lane_does_not_reach_the_output() {
        let offsets = auto_offsets(2, RATE);
        let spur_hz = -20.0;
        let len = 360 * HOP;
        let mut lanes = [noise(len, 5), noise(len, 13)];
        let spur = tone(len, spur_hz - offsets[0], Complex::new(0.29, 0.0));
        for (sample, extra) in lanes[0].iter_mut().zip(&spur) {
            *sample += extra;
        }
        let run = |options| {
            let mut stitcher = with_options(2, options);
            let out = stitch(&mut stitcher, &lanes);
            let power = psd(&out[160 * 2 * HOP..], 8192);
            let spurs = (
                stitcher.lane_state(0).spur_bins,
                stitcher.lane_state(1).spur_bins,
            );
            (band_db(&power, -40..=-40), spurs)
        };
        let (kept, _) = run(PLAIN);
        for options in [SPUR_REJECT, StitchOptions::default()] {
            let (rejected, spurs) = run(options);
            assert!(kept - rejected > 20.0, "kept {kept} rejected {rejected}");
            assert_eq!(spurs, (1, 0), "{options:?}");
        }
    }

    #[test]
    fn a_tone_both_lanes_hear_is_not_a_spur() {
        let offsets = auto_offsets(2, RATE);
        let len = 360 * HOP;
        let heard = [Complex::new(0.29, 0.0); 2];
        let mut lanes = lanes_hearing(-20.0, &offsets, len, &heard);
        for (lane, seed) in lanes.iter_mut().zip([5, 13]) {
            for (sample, extra) in lane.iter_mut().zip(noise(len, seed)) {
                *sample += extra;
            }
        }
        let run = |options| {
            let mut stitcher = with_options(2, options);
            let out = stitch(&mut stitcher, &lanes);
            let power = psd(&out[160 * 2 * HOP..], 8192);
            let spurs = (
                stitcher.lane_state(0).spur_bins,
                stitcher.lane_state(1).spur_bins,
            );
            (band_db(&power, -40..=-40), spurs)
        };
        let (plain, _) = run(PLAIN);
        let (kept, spurs) = run(StitchOptions::default());
        assert_eq!(spurs, (0, 0));
        assert!((plain - kept).abs() < 1.0, "plain {plain} kept {kept}");
    }

    #[test]
    fn snr_blend_prefers_the_quieter_lane() {
        let len = 100 * HOP;
        let lanes = [noise(len, 3), louder(noise(len, 7), 2.0)];
        let mut stitcher = with_options(2, SNR_BLEND);
        stitch(&mut stitcher, &lanes);
        let (quiet, loud) = (share_at_centre(&stitcher, 0), share_at_centre(&stitcher, 1));
        assert!(loud < 0.3, "{loud}");
        assert!((quiet + loud - 1.0).abs() < 1e-3, "{quiet} {loud}");
        let mut even = with_options(2, PLAIN);
        stitch(&mut even, &lanes);
        assert!((share_at_centre(&even, 1) - 0.5).abs() < 0.01);
    }

    #[test]
    fn snr_blend_keeps_the_crossfade_sum() {
        let len = 100 * HOP;
        let lanes = [noise(len, 3), louder(noise(len, 7), 2.0)];
        let layout = [-900.0, 900.0];
        let mut blended = with_options(2, SNR_BLEND);
        blended.retune(&layout).expect("layout");
        stitch(&mut blended, &lanes);
        let mut even = plain(2);
        even.retune(&layout).expect("layout");
        let (blended, even) = (share_sums(&blended), share_sums(&even));
        assert!(even.iter().any(|sum| *sum > 0.01 && *sum < 0.9));
        for (a, b) in blended.iter().zip(&even) {
            assert!((a - b).abs() < 1e-4, "{a} {b}");
        }
    }

    #[test]
    fn option_changes_apply_at_once() {
        let len = 100 * HOP;
        let lanes = [noise(len, 3), louder(noise(len, 7), 2.0)];
        let mut stitcher = with_options(2, SNR_BLEND);
        stitch(&mut stitcher, &lanes);
        stitcher.set_options(StitchOptions {
            equalise: true,
            ..SNR_BLEND
        });
        assert!((share_at_centre(&stitcher, 1) - 0.5).abs() < 0.05);
        assert!((stitcher.lane_state(1).gain_db + 3.01).abs() < 0.3);
        stitcher.set_options(SNR_BLEND);
        assert!(share_at_centre(&stitcher, 1) < 0.3);
        assert_eq!(stitcher.lane_state(1).gain_db, 0.0);
        stitcher.set_options(PLAIN);
        assert!((share_at_centre(&stitcher, 1) - 0.5).abs() < 0.01);
    }

    #[test]
    fn lane_state_reports_the_match() {
        let offsets = auto_offsets(2, RATE);
        let gains = [Complex::new(1.0, 0.0), Complex::from_polar(0.5, 1.1)];
        let lanes = multitone(&offsets, 120 * HOP, &gains);
        let mut stitcher = plain(2);
        stitch(&mut stitcher, &lanes);
        let state = stitcher.lane_state(1);
        assert!(
            (state.phase_deg + 1.1f32.to_degrees()).abs() < 1.0,
            "{state:?}"
        );
        assert!((state.gain_db - 6.02).abs() < 0.2, "{state:?}");
        assert!(
            state.coherence.is_some_and(|value| value > 0.9),
            "{state:?}"
        );
        assert_eq!(stitcher.lane_state(0).coherence, None);
        assert_eq!(stitcher.lane_state(0).phase_deg, 0.0);
        assert_eq!(stitcher.lane_state(2), LaneState::default());
    }

    #[test]
    fn a_retune_starts_the_floor_estimate_over() {
        let len = 100 * HOP;
        let lanes = [noise(len, 3), louder(noise(len, 7), 2.0)];
        let mut stitcher = with_options(2, EQUALISE);
        stitch(&mut stitcher, &lanes);
        assert!(stitcher.lane_state(1).gain_db < -2.5);
        stitcher.retune(&[-900.0, 900.0]).expect("layout");
        assert_eq!(stitcher.lane_state(1).gain_db, 0.0);
        assert_eq!(stitcher.lane_state(1).noise_db, POWER_FLOOR_DB);
        stitch(&mut stitcher, &lanes);
        assert!(stitcher.lane_state(1).gain_db < -2.5);
        stitcher.reset();
        assert_eq!(stitcher.lane_state(1).gain_db, 0.0);
        assert_eq!(stitcher.lane_state(1).noise_db, POWER_FLOOR_DB);
    }

    #[test]
    fn equalise_takes_only_the_phase_of_the_match() {
        let offsets = auto_offsets(2, RATE);
        let gains = [Complex::new(1.0, 0.0), Complex::from_polar(0.5, 1.1)];
        let lanes = multitone(&offsets, 200 * HOP, &gains);
        let mut stitcher = with_options(2, EQUALISE);
        stitch(&mut stitcher, &lanes);
        let (low, high) = (stitcher.lane_state(0), stitcher.lane_state(1));
        assert!(
            (high.phase_deg + 1.1f32.to_degrees()).abs() < 1.0,
            "{high:?}"
        );
        assert!((low.gain_db + 3.01).abs() < 0.3, "{low:?}");
        assert!((high.gain_db - 3.01).abs() < 0.3, "{high:?}");
        assert!((stitcher.lanes[1].correction.norm() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn a_weak_match_holds_the_last_phase() {
        let offsets = auto_offsets(2, RATE);
        let gains = [Complex::new(1.0, 0.0), Complex::from_polar(0.5, 1.1)];
        let mut stitcher = with_options(2, EQUALISE);
        stitch(&mut stitcher, &multitone(&offsets, 100 * HOP, &gains));
        let locked = stitcher.lane_state(1).phase_deg;
        stitch(&mut stitcher, &[noise(100 * HOP, 4), noise(100 * HOP, 8)]);
        let held = stitcher.lane_state(1);
        assert!(
            held.coherence.is_some_and(|value| value < MATCH_COHERENCE),
            "{held:?}"
        );
        assert!((held.phase_deg - locked).abs() < 0.5, "{locked} {held:?}");
    }

    #[test]
    fn without_phase_matching_lanes_keep_their_own_phase() {
        let offsets = auto_offsets(2, RATE);
        let gains = [Complex::new(1.0, 0.0), Complex::from_polar(0.6, 2.3)];
        let options = StitchOptions {
            match_phase: false,
            ..PLAIN
        };
        let mut stitcher = with_options(2, options);
        let out = stitch(&mut stitcher, &lanes_hearing(3.0, &offsets, 60_000, &gains));
        let (amplitude, _) = fit(&out, 3.0, 2, out.len() / 2);
        assert!(
            (amplitude - Complex::new(1.0, 0.0)).norm() > 0.3,
            "{amplitude}"
        );
        let state = stitcher.lane_state(1);
        assert_eq!((state.phase_deg, state.gain_db), (0.0, 0.0));
        assert!(
            state.coherence.is_some_and(|value| value > 0.9),
            "{state:?}"
        );
    }

    #[test]
    fn power_db_matches_log10() {
        for exponent in -29..30 {
            for step in 0..50 {
                let power = 10f32.powf(exponent as f32 + step as f32 / 50.0);
                let exact = 10.0 * power.log10();
                assert!((power_db(power) - exact).abs() < 1e-3, "{power}");
            }
        }
        assert!((power_db(0.0) - POWER_FLOOR_DB).abs() < 1e-3);
        assert!((power_db(f32::NAN) - POWER_FLOOR_DB).abs() < 1e-3);
    }
}
