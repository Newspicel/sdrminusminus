use std::f64::consts::TAU;

use num_complex::Complex;

use crate::fft::FftPair;

pub const STITCH_FFT: usize = 2048;
pub const STITCH_KEEP: f64 = 0.85;

const MIN_LANES: usize = 2;
const MAX_LANES: usize = 16;
const RAMP: f64 = 0.05;
const DC_CORNER: f64 = 1e-5;
const MATCH_SMOOTHING: f32 = 0.2;
const MATCH_COHERENCE: f32 = 0.3;
const HOP: usize = STITCH_FFT / 2;

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum StitchError {
    #[error("a stitch takes {MIN_LANES} to {MAX_LANES} lanes, not {0}")]
    Lanes(usize),
    #[error("the input rate must be positive and finite, got {0} Hz")]
    Rate(f64),
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

fn lane_weight(bin: i64) -> f32 {
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

#[derive(Clone, Copy)]
struct Tap {
    lane_bin: usize,
    out_bin: usize,
    weight: f32,
}

#[derive(Clone, Copy, Default)]
struct Match {
    low: usize,
    high: usize,
    cross: Complex<f32>,
    low_power: f32,
    high_power: f32,
    gain: Complex<f32>,
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
        if self.cross.norm_sqr() / product > MATCH_COHERENCE {
            self.gain = self.cross / self.high_power;
        }
    }
}

struct Lane {
    offset_hz: f64,
    shift_bins: i64,
    residual: Complex<f64>,
    phasor: Complex<f64>,
    dc: Complex<f32>,
    frame: Vec<Complex<f32>>,
    spectrum: Vec<Complex<f32>>,
    taps: Vec<Tap>,
    overlap: Vec<(usize, usize)>,
    correction: Complex<f32>,
}

impl Lane {
    fn new() -> Self {
        Self {
            offset_hz: f64::NAN,
            shift_bins: 0,
            residual: Complex::new(1.0, 0.0),
            phasor: Complex::new(1.0, 0.0),
            dc: Complex::default(),
            frame: vec![Complex::default(); STITCH_FFT],
            spectrum: vec![Complex::default(); STITCH_FFT],
            taps: Vec::with_capacity(STITCH_FFT),
            overlap: Vec::with_capacity(STITCH_FFT),
            correction: Complex::new(1.0, 0.0),
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
        let turns = (self.shift_bins.rem_euclid(len) * start).rem_euclid(len);
        let phase = TAU * turns as f64 / STITCH_FFT as f64;
        Complex::from_polar(1.0, phase as f32)
    }
}

pub struct Stitcher {
    input_rate: f64,
    out_len: usize,
    dc_alpha: f32,
    lanes: Vec<Lane>,
    order: Vec<usize>,
    matches: Vec<Match>,
    previous: Vec<Match>,
    sums: Vec<f32>,
    output: Vec<Complex<f32>>,
    lane_fft: FftPair,
    out_fft: FftPair,
    fill: usize,
    frame_start: u64,
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
            lanes: (0..lanes).map(|_| Lane::new()).collect(),
            order: (0..lanes).collect(),
            matches: Vec::with_capacity(lanes),
            previous: Vec::with_capacity(lanes),
            sums: vec![0.0; out_len],
            output: vec![Complex::default(); out_len],
            lane_fft: FftPair::new(STITCH_FFT),
            out_fft: FftPair::new(out_len),
            fill: 0,
            frame_start: 0,
        };
        stitcher.retune(&auto_offsets(lanes, input_rate));
        Ok(stitcher)
    }

    #[must_use]
    pub fn output_rate(&self) -> f64 {
        output_rate(self.lanes.len(), self.input_rate)
    }

    pub fn retune(&mut self, offsets_hz: &[f64]) {
        let bin_hz = self.input_rate / STITCH_FFT as f64;
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
        if !changed.iter().any(|flag| *flag) {
            return;
        }
        self.place_taps();
        self.normalise_taps();
        self.pair_lanes(&changed);
    }

    pub fn reset(&mut self) {
        self.fill = 0;
        self.frame_start = 0;
        for lane in &mut self.lanes {
            lane.phasor = Complex::new(1.0, 0.0);
            lane.dc = Complex::default();
        }
    }

    pub fn process(&mut self, lanes: &[&[Complex<f32>]], out: &mut Vec<Complex<f32>>) {
        if lanes.len() != self.lanes.len() {
            return;
        }
        let len = lanes.iter().map(|lane| lane.len()).min().unwrap_or(0);
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
    }

    fn place_taps(&mut self) {
        let half = self.out_len as i64 / 2;
        self.sums.fill(0.0);
        for lane in &mut self.lanes {
            lane.taps.clear();
            for lane_bin in 0..STITCH_FFT {
                let bin = signed_bin(lane_bin, STITCH_FFT);
                let weight = lane_weight(bin);
                let target = bin + lane.shift_bins;
                if weight <= 0.0 || target < -half || target >= half {
                    continue;
                }
                let out_bin = target.rem_euclid(self.out_len as i64) as usize;
                self.sums[out_bin] += weight;
                lane.taps.push(Tap {
                    lane_bin,
                    out_bin,
                    weight,
                });
            }
        }
    }

    fn normalise_taps(&mut self) {
        let scale = 1.0 / STITCH_FFT as f32;
        for lane in &mut self.lanes {
            for tap in &mut lane.taps {
                tap.weight = tap.weight / self.sums[tap.out_bin].max(1.0) * scale;
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
        for window in 0..self.order.len().saturating_sub(1) {
            let (low, high) = (self.order[window], self.order[window + 1]);
            if !self.find_overlap(low, high) {
                continue;
            }
            let kept = self
                .previous
                .iter()
                .find(|old| old.low == low && old.high == high && !changed[low] && !changed[high]);
            self.matches
                .push(kept.copied().unwrap_or_else(|| Match::fresh(low, high)));
        }
    }

    fn find_overlap(&mut self, low: usize, high: usize) -> bool {
        self.sums.fill(-1.0);
        for tap in &self.lanes[low].taps {
            self.sums[tap.out_bin] = tap.lane_bin as f32;
        }
        let (sums, lanes) = (&self.sums, &mut self.lanes);
        let mut pairs = std::mem::take(&mut lanes[high].overlap);
        pairs.clear();
        for tap in &lanes[high].taps {
            let found = sums[tap.out_bin];
            if found >= 0.0 {
                pairs.push((found as usize, tap.lane_bin));
            }
        }
        let any = !pairs.is_empty();
        lanes[high].overlap = pairs;
        any
    }

    fn run_frame(&mut self, out: &mut Vec<Complex<f32>>) {
        for lane in &mut self.lanes {
            lane.spectrum.copy_from_slice(&lane.frame);
            self.lane_fft.forward(&mut lane.spectrum);
            lane.frame.copy_within(HOP.., 0);
        }
        self.fill = STITCH_FFT - HOP;
        self.match_lanes();
        self.synthesise();
        let quarter = self.out_len / 4;
        out.extend_from_slice(&self.output[quarter..self.out_len - quarter]);
        self.frame_start += HOP as u64;
    }

    fn match_lanes(&mut self) {
        let start = self.frame_start;
        for pair in &mut self.matches {
            let low = &self.lanes[pair.low];
            let high = &self.lanes[pair.high];
            let (low_turn, high_turn) = (low.rotation(start), high.rotation(start));
            let mut cross = Complex::default();
            let (mut low_power, mut high_power) = (0.0f32, 0.0f32);
            for (low_bin, high_bin) in &high.overlap {
                let a = low.spectrum[*low_bin] * low_turn;
                let b = high.spectrum[*high_bin] * high_turn;
                cross += a * b.conj();
                low_power += a.norm_sqr();
                high_power += b.norm_sqr();
            }
            pair.update(cross, low_power, high_power);
        }
        for lane in &mut self.lanes {
            lane.correction = Complex::new(1.0, 0.0);
        }
        for pair in &self.matches {
            let base = self.lanes[pair.low].correction;
            self.lanes[pair.high].correction = base * pair.gain;
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
    use super::*;

    const RATE: f64 = 2048.0;

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
        stitcher.process(&views, &mut out);
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

    #[test]
    fn a_tone_in_one_lane_lands_where_it_belongs() {
        let offsets = auto_offsets(3, RATE);
        let hz = offsets[2] + 100.0;
        let mut stitcher = Stitcher::new(3, RATE).expect("stitcher");
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
        let mut stitcher = Stitcher::new(2, RATE).expect("stitcher");
        stitcher.retune(&offsets);
        let heard = [Complex::new(1.0, 0.0), Complex::default()];
        let out = stitch(&mut stitcher, &lanes_hearing(hz, &offsets, 20_000, &heard));
        let (amplitude, residual) = fit(&out, hz, 2, 2 * STITCH_FFT);
        assert!(
            (amplitude - Complex::new(1.0, 0.0)).norm() < 0.02,
            "{amplitude}"
        );
        assert!(residual < 1e-3, "{residual}");
    }

    #[test]
    fn the_output_runs_at_lanes_times_the_input() {
        let mut stitcher = Stitcher::new(4, RATE).expect("stitcher");
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
        let mut stitcher = Stitcher::new(2, RATE).expect("stitcher");
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

    #[test]
    fn a_gap_between_lanes_stays_empty() {
        let mut stitcher = Stitcher::new(2, RATE).expect("stitcher");
        stitcher.retune(&[-1.2 * RATE, 1.2 * RATE]);
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
        let lanes = [noise(12_000, 3), noise(12_000, 5), noise(12_000, 9)];
        let mut whole = Stitcher::new(3, RATE).expect("stitcher");
        let expected = stitch(&mut whole, &lanes);
        let mut pieces = Stitcher::new(3, RATE).expect("stitcher");
        pieces.retune(&offsets);
        let mut out = Vec::new();
        for start in (0..12_000).step_by(777) {
            let end = (start + 777).min(12_000);
            let views: Vec<&[Complex<f32>]> = lanes.iter().map(|lane| &lane[start..end]).collect();
            pieces.process(&views, &mut out);
        }
        assert_eq!(out.len(), expected.len());
        for (a, b) in out.iter().zip(&expected) {
            assert!((a - b).norm() < 1e-5);
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
}
