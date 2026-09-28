use std::f64::consts::SQRT_2;

use crate::special::{erf, norm_deg, wrap_deg};

pub const SWEEP_BINS: usize = 72;

const LEVEL_CAPACITY: usize = 1024;
const HEADING_CAPACITY: usize = 2048;
const TEMPLATE_POINTS: usize = 360;
const LIKELIHOOD_POINTS: usize = 360;
const HEADING_REACH_S: f64 = 0.5;
const RATE_WINDOW_S: f64 = 0.3;
const FLIP_HOLD_S: f64 = 0.3;
const MAX_SPAN_DEG: f64 = 390.0;
const LEVEL_PERIOD_S: f64 = 0.05;
const START_LAG_S: f64 = 0.1;
const MAX_LAG_S: f64 = 0.5;
const LAG_GAIN: f64 = 0.5;
const PAIR_WINDOW_S: f64 = 30.0;
const PAIR_WINDOW_DEG: f64 = 30.0;
const LIKELIHOOD_FLOOR: f64 = 0.02;
const CONFIDENCE_WINDOW_DEG: f64 = 5.0;
const BIN_WIDTH_DEG: f64 = 360.0 / SWEEP_BINS as f64;
const CURVATURE_STEP_DEG: f64 = 0.5;
const MIN_FIT_SAMPLES: usize = 4;
const WIDEST_SIGMA_DEG: f64 = 180.0;
const ONSET_DEG: f64 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SweepConfig {
    pub beamwidth_deg: f64,
    pub front_back_db: f64,
    pub min_span_deg: f64,
    pub min_contrast_db: f32,
    pub min_fit: f32,
    pub max_heading_sigma_deg: f32,
    pub stop_s: f64,
    pub min_rate_dps: f64,
    pub max_rate_dps: f64,
    pub sample_correlation: f64,
}

impl Default for SweepConfig {
    fn default() -> Self {
        Self {
            beamwidth_deg: 60.0,
            front_back_db: 15.0,
            min_span_deg: 180.0,
            min_contrast_db: 6.0,
            min_fit: 0.5,
            max_heading_sigma_deg: 30.0,
            stop_s: 0.7,
            min_rate_dps: 10.0,
            max_rate_dps: 360.0,
            sample_correlation: 4.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelSample {
    pub t_s: f64,
    pub level_db: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadingSample {
    pub t_s: f64,
    pub heading_deg: f64,
    pub sigma_deg: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rotation {
    Clockwise,
    Counterclockwise,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SweepReject {
    ShortSpan,
    LowContrast,
    PoorFit,
    HeadingPoor,
    TooFast,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SweepPhase {
    Idle,
    Sweeping { span_deg: f32, rate_dps: f32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SweepBearing {
    pub bearing_deg: f64,
    pub sigma_deg: f32,
    pub confidence: f32,
    pub fit: f32,
    pub contrast_db: f32,
    pub span_deg: f32,
    pub rate_dps: f32,
    pub rotation: Rotation,
    pub samples: u16,
    pub lag_s: f32,
    pub likelihood: [f32; LIKELIHOOD_POINTS],
}

impl Default for SweepBearing {
    fn default() -> Self {
        Self {
            bearing_deg: 0.0,
            sigma_deg: 0.0,
            confidence: 0.0,
            fit: 0.0,
            contrast_db: 0.0,
            span_deg: 0.0,
            rate_dps: 0.0,
            rotation: Rotation::Clockwise,
            samples: 0,
            lag_s: 0.0,
            likelihood: [0.0; LIKELIHOOD_POINTS],
        }
    }
}

struct HeadingRing {
    t: [f64; HEADING_CAPACITY],
    heading: [f64; HEADING_CAPACITY],
    sigma: [f32; HEADING_CAPACITY],
    start: usize,
    len: usize,
    last_raw: f64,
    stale: u64,
}

impl HeadingRing {
    fn new() -> Self {
        Self {
            t: [0.0; HEADING_CAPACITY],
            heading: [0.0; HEADING_CAPACITY],
            sigma: [0.0; HEADING_CAPACITY],
            start: 0,
            len: 0,
            last_raw: 0.0,
            stale: 0,
        }
    }

    fn clear(&mut self) {
        self.start = 0;
        self.len = 0;
    }

    fn slot(&self, index: usize) -> usize {
        (self.start + index) % HEADING_CAPACITY
    }

    fn push(&mut self, sample: HeadingSample) {
        let valid = sample.t_s.is_finite() && sample.heading_deg.is_finite();
        let newest = self.len.checked_sub(1).map(|last| self.t[self.slot(last)]);
        if !valid || newest.is_some_and(|newest| sample.t_s <= newest) {
            self.stale += 1;
            return;
        }
        let unwrapped = match self.len.checked_sub(1) {
            None => norm_deg(sample.heading_deg),
            Some(last) => {
                self.heading[self.slot(last)] + wrap_deg(sample.heading_deg - self.last_raw)
            }
        };
        self.last_raw = sample.heading_deg;
        if self.len == HEADING_CAPACITY {
            self.start = (self.start + 1) % HEADING_CAPACITY;
            self.len -= 1;
        }
        let slot = self.slot(self.len);
        self.t[slot] = sample.t_s;
        self.heading[slot] = unwrapped;
        self.sigma[slot] = sample.sigma_deg;
        self.len += 1;
    }

    fn at(&self, index: usize) -> (f64, f64, f32) {
        let slot = self.slot(index);
        (self.t[slot], self.heading[slot], self.sigma[slot])
    }

    fn unwrapped_at(&self, t_s: f64) -> Option<(f64, f32)> {
        let last = self.len.checked_sub(1)?;
        let (t_first, h_first, s_first) = self.at(0);
        let (t_last, h_last, s_last) = self.at(last);
        if t_s >= t_last {
            if t_s - t_last > HEADING_REACH_S {
                return None;
            }
            let rate = match last.checked_sub(1).map(|before| self.at(before)) {
                Some((t_before, h_before, _)) if t_last > t_before => {
                    (h_last - h_before) / (t_last - t_before)
                }
                _ => 0.0,
            };
            return Some((h_last + rate * (t_s - t_last), s_last));
        }
        if t_s <= t_first {
            return (t_first - t_s <= HEADING_REACH_S).then_some((h_first, s_first));
        }
        let (mut low, mut high) = (0, last);
        while high - low > 1 {
            let mid = (low + high) / 2;
            if self.at(mid).0 <= t_s {
                low = mid;
            } else {
                high = mid;
            }
        }
        let (t0, h0, s0) = self.at(low);
        let (t1, h1, s1) = self.at(high);
        if (t_s - t0).min(t1 - t_s) > HEADING_REACH_S {
            return None;
        }
        let weight = (t_s - t0) / (t1 - t0);
        let sigma = f64::from(s0) + weight * (f64::from(s1) - f64::from(s0));
        Some((h0 + weight * (h1 - h0), sigma as f32))
    }
}

struct Track {
    t: [f64; LEVEL_CAPACITY],
    level: [f32; LEVEL_CAPACITY],
    heading: [f64; LEVEL_CAPACITY],
    sigma: [f32; LEVEL_CAPACITY],
    len: usize,
}

impl Track {
    fn new() -> Self {
        Self {
            t: [0.0; LEVEL_CAPACITY],
            level: [0.0; LEVEL_CAPACITY],
            heading: [0.0; LEVEL_CAPACITY],
            sigma: [0.0; LEVEL_CAPACITY],
            len: 0,
        }
    }

    fn push(&mut self, t: f64, level: f32, heading: f64, sigma: f32) {
        let at = self.len;
        self.t[at] = t;
        self.level[at] = level;
        self.heading[at] = heading;
        self.sigma[at] = sigma;
        self.len += 1;
    }

    fn drop_front(&mut self, count: usize) {
        let count = count.min(self.len);
        let len = self.len;
        self.t.copy_within(count..len, 0);
        self.level.copy_within(count..len, 0);
        self.heading.copy_within(count..len, 0);
        self.sigma.copy_within(count..len, 0);
        self.len -= count;
    }

    fn last(&self) -> usize {
        self.len.saturating_sub(1)
    }

    fn window_start(&self) -> usize {
        let newest = self.t[self.last()];
        let mut start = self.last();
        while start > 0 && newest - self.t[start - 1] <= RATE_WINDOW_S {
            start -= 1;
        }
        start
    }

    fn rate_dps(&self) -> f64 {
        let last = self.last();
        let start = self.window_start();
        if start == last {
            return 0.0;
        }
        (self.heading[last] - self.heading[start]) / (self.t[last] - self.t[start])
    }

    fn onset(&self, within_deg: f64) -> usize {
        let first = self.heading[0];
        (1..self.len)
            .find(|&index| (self.heading[index] - first).abs() > within_deg)
            .map_or(0, |moved| moved - 1)
    }

    fn extreme(&self, sign: f64) -> usize {
        (1..self.len).fold(0, |best, index| {
            if sign * self.heading[index] > sign * self.heading[best] {
                index
            } else {
                best
            }
        })
    }
}

#[derive(Clone, Copy)]
struct Segment {
    sign: f64,
    still_since: Option<f64>,
    flip_since: Option<f64>,
}

#[derive(Clone, Copy)]
enum Motion {
    Idle,
    Sweeping(Segment),
    Spinning,
}

#[derive(Clone, Copy)]
enum Close {
    Stop,
    Flip,
    Span,
}

#[derive(Clone, Copy)]
struct PastSweep {
    bearing_deg: f64,
    rate_dps: f64,
    lag_s: f64,
    at_s: f64,
}

struct Fit {
    bearing_deg: f64,
    fit: f64,
    sigma_deg: f64,
    curvature: f64,
    s_ll: f64,
    rss_best: f64,
    noise_var: f64,
}

pub struct SweepEstimator {
    config: SweepConfig,
    template: [f32; TEMPLATE_POINTS],
    headings: Box<HeadingRing>,
    track: Box<Track>,
    motion: Motion,
    bins: [f32; SWEEP_BINS],
    lag_s: f64,
    last: [Option<PastSweep>; 2],
    scratch: Box<[f32; LEVEL_CAPACITY]>,
    correlation: [f64; LIKELIHOOD_POINTS],
    dropped_levels: u64,
}

impl SweepEstimator {
    #[must_use]
    pub fn new(config: SweepConfig) -> Self {
        let mut estimator = Self {
            config,
            template: [0.0; TEMPLATE_POINTS],
            headings: Box::new(HeadingRing::new()),
            track: Box::new(Track::new()),
            motion: Motion::Idle,
            bins: [f32::NEG_INFINITY; SWEEP_BINS],
            lag_s: START_LAG_S,
            last: [None; 2],
            scratch: Box::new([0.0; LEVEL_CAPACITY]),
            correlation: [0.0; LIKELIHOOD_POINTS],
            dropped_levels: 0,
        };
        estimator.configure(config);
        estimator
    }

    pub fn configure(&mut self, config: SweepConfig) {
        self.config = config;
        let half = (config.beamwidth_deg / 2.0).to_radians().cos();
        let power = 0.5f64.ln() / ((1.0 + half) / 2.0).ln();
        for (degree, value) in self.template.iter_mut().enumerate() {
            let cosine = (degree as f64).to_radians().cos();
            let gain = 10.0 * power * ((1.0 + cosine) / 2.0).log10();
            *value = gain.max(-config.front_back_db) as f32;
        }
    }

    pub fn reset(&mut self) {
        self.headings.clear();
        self.track.len = 0;
        self.motion = Motion::Idle;
        self.bins = [f32::NEG_INFINITY; SWEEP_BINS];
        self.last = [None; 2];
    }

    pub fn push_heading(&mut self, sample: HeadingSample) {
        self.headings.push(sample);
    }

    pub fn push_level(
        &mut self,
        sample: LevelSample,
        out: &mut SweepBearing,
    ) -> Option<Result<(), SweepReject>> {
        let late = self.track.len > 0 && sample.t_s <= self.track.t[self.track.last()];
        if !sample.t_s.is_finite() || !sample.level_db.is_finite() || late {
            self.dropped_levels += 1;
            return None;
        }
        let Some((heading, sigma)) = self.headings.unwrapped_at(sample.t_s - self.lag_s) else {
            self.dropped_levels += 1;
            self.motion = Motion::Idle;
            self.track.len = 0;
            return None;
        };
        let overflow = if self.track.len == LEVEL_CAPACITY {
            self.make_room(out)
        } else {
            None
        };
        self.track.push(sample.t_s, sample.level_db, heading, sigma);
        overflow.or_else(|| self.advance(out))
    }

    #[must_use]
    pub fn phase(&self) -> SweepPhase {
        match self.motion {
            Motion::Sweeping(segment) if self.track.len > 0 => {
                let last = self.track.last();
                SweepPhase::Sweeping {
                    span_deg: (segment.sign * (self.track.heading[last] - self.track.heading[0]))
                        .max(0.0) as f32,
                    rate_dps: self.track.rate_dps().abs() as f32,
                }
            }
            _ => SweepPhase::Idle,
        }
    }

    #[must_use]
    pub const fn bins(&self) -> &[f32; SWEEP_BINS] {
        &self.bins
    }

    #[must_use]
    pub fn covered_deg(&self) -> f32 {
        let covered = self.bins.iter().filter(|value| value.is_finite()).count();
        (covered as f64 * BIN_WIDTH_DEG) as f32
    }

    #[must_use]
    pub const fn lag_s(&self) -> f64 {
        self.lag_s
    }

    #[must_use]
    pub const fn stale_headings(&self) -> u64 {
        self.headings.stale
    }

    #[must_use]
    pub const fn dropped_levels(&self) -> u64 {
        self.dropped_levels
    }

    #[must_use]
    pub fn heading_at(&self, t_s: f64) -> Option<HeadingSample> {
        self.headings
            .unwrapped_at(t_s)
            .map(|(heading, sigma)| HeadingSample {
                t_s,
                heading_deg: norm_deg(heading),
                sigma_deg: sigma,
            })
    }

    fn make_room(&mut self, out: &mut SweepBearing) -> Option<Result<(), SweepReject>> {
        match self.motion {
            Motion::Sweeping(segment) => Some(self.close(segment, Close::Span, out)),
            Motion::Idle | Motion::Spinning => {
                self.track.drop_front(1);
                None
            }
        }
    }

    fn advance(&mut self, out: &mut SweepBearing) -> Option<Result<(), SweepReject>> {
        let rate = self.track.rate_dps();
        let now = self.track.t[self.track.last()];
        let config = self.config;
        match self.motion {
            Motion::Idle | Motion::Spinning if rate.abs() > config.max_rate_dps => {
                let was_idle = matches!(self.motion, Motion::Idle);
                self.motion = Motion::Spinning;
                self.keep_window();
                was_idle.then_some(Err(SweepReject::TooFast))
            }
            Motion::Idle | Motion::Spinning => {
                if rate.abs() >= config.min_rate_dps {
                    self.start_sweep(rate.signum());
                } else {
                    self.motion = Motion::Idle;
                    self.keep_window();
                }
                None
            }
            Motion::Sweeping(_) if rate.abs() > config.max_rate_dps => {
                self.motion = Motion::Spinning;
                self.keep_window();
                Some(Err(SweepReject::TooFast))
            }
            Motion::Sweeping(segment) => self.follow(segment, rate, now, out),
        }
    }

    fn follow(
        &mut self,
        mut segment: Segment,
        rate: f64,
        now: f64,
        out: &mut SweepBearing,
    ) -> Option<Result<(), SweepReject>> {
        let last = self.track.last();
        self.add_to_bins(last);
        let along = segment.sign * rate;
        let close = if along >= self.config.min_rate_dps {
            segment.still_since = None;
            segment.flip_since = None;
            None
        } else if -along >= self.config.min_rate_dps {
            segment.still_since = None;
            let since = *segment.flip_since.get_or_insert(now);
            (now - since >= FLIP_HOLD_S).then_some(Close::Flip)
        } else {
            segment.flip_since = None;
            let since = *segment.still_since.get_or_insert(now);
            (now - since >= self.config.stop_s).then_some(Close::Stop)
        };
        let span = segment.sign * (self.track.heading[last] - self.track.heading[0]);
        let close = close.or((span > MAX_SPAN_DEG).then_some(Close::Span));
        self.motion = Motion::Sweeping(segment);
        close.map(|reason| self.close(segment, reason, out))
    }

    fn start_sweep(&mut self, sign: f64) {
        let start = self.track.window_start();
        self.track.drop_front(start);
        self.motion = Motion::Sweeping(Segment {
            sign,
            still_since: None,
            flip_since: None,
        });
        self.rebuild_bins();
    }

    fn keep_window(&mut self) {
        let start = self.track.window_start();
        self.track.drop_front(start);
    }

    fn close(
        &mut self,
        segment: Segment,
        reason: Close,
        out: &mut SweepBearing,
    ) -> Result<(), SweepReject> {
        let end = match reason {
            Close::Stop | Close::Flip => self.track.extreme(segment.sign),
            Close::Span => self.track.last(),
        };
        let result = self.evaluate(end, segment.sign, out);
        match reason {
            Close::Stop => {
                self.motion = Motion::Idle;
                self.keep_window();
            }
            Close::Flip => {
                self.track.drop_front(end);
                self.start_segment(-segment.sign);
            }
            Close::Span => {
                self.track.drop_front(end);
                self.start_segment(segment.sign);
            }
        }
        result
    }

    fn start_segment(&mut self, sign: f64) {
        self.motion = Motion::Sweeping(Segment {
            sign,
            still_since: None,
            flip_since: None,
        });
        self.rebuild_bins();
    }

    fn rebuild_bins(&mut self) {
        self.bins = [f32::NEG_INFINITY; SWEEP_BINS];
        for index in 0..self.track.len {
            self.add_to_bins(index);
        }
    }

    fn add_to_bins(&mut self, index: usize) {
        let bin = (norm_deg(self.track.heading[index]) / BIN_WIDTH_DEG) as usize % SWEEP_BINS;
        self.bins[bin] = self.bins[bin].max(self.track.level[index]);
    }

    fn evaluate(
        &mut self,
        end: usize,
        sign: f64,
        out: &mut SweepBearing,
    ) -> Result<(), SweepReject> {
        let count = end + 1;
        let headings = &self.track.heading[..count];
        let lowest = headings.iter().copied().fold(f64::INFINITY, f64::min);
        let highest = headings.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let span = highest - lowest;
        if span < self.config.min_span_deg {
            return Err(SweepReject::ShortSpan);
        }
        let contrast = self.contrast(count);
        if contrast < self.config.min_contrast_db {
            return Err(SweepReject::LowContrast);
        }
        let mean_sigma = self.track.sigma[..count]
            .iter()
            .map(|&s| f64::from(s))
            .sum::<f64>()
            / count as f64;
        if mean_sigma > f64::from(self.config.max_heading_sigma_deg) {
            return Err(SweepReject::HeadingPoor);
        }
        let onset = self.track.onset(ONSET_DEG);
        let duration = self.track.t[end] - self.track.t[onset.min(end)];
        let rate = if duration > 0.0 { span / duration } else { 0.0 };
        let fit = self.fit(count, rate, mean_sigma)?;
        self.fill_likelihood(&fit, out);
        out.bearing_deg = norm_deg(fit.bearing_deg);
        out.sigma_deg = fit.sigma_deg as f32;
        out.confidence = (erf(CONFIDENCE_WINDOW_DEG / (fit.sigma_deg * SQRT_2)) * fit.fit) as f32;
        out.fit = fit.fit as f32;
        out.contrast_db = contrast;
        out.span_deg = span as f32;
        out.rate_dps = rate as f32;
        out.rotation = if sign > 0.0 {
            Rotation::Clockwise
        } else {
            Rotation::Counterclockwise
        };
        out.samples = count.min(usize::from(u16::MAX)) as u16;
        out.lag_s = self.lag_s as f32;
        self.learn_lag(out.bearing_deg, rate, out.rotation, self.track.t[end]);
        Ok(())
    }

    fn contrast(&mut self, count: usize) -> f32 {
        let levels = &mut self.scratch[..count];
        levels.copy_from_slice(&self.track.level[..count]);
        levels.sort_unstable_by(f32::total_cmp);
        let at = |fraction: f64| levels[((count - 1) as f64 * fraction).round() as usize];
        at(0.95) - at(0.05)
    }

    fn template_at(&self, offset_deg: f64) -> f64 {
        let position = norm_deg(offset_deg);
        let low = (position as usize).min(TEMPLATE_POINTS - 1);
        let high = (low + 1) % TEMPLATE_POINTS;
        let weight = position - low as f64;
        f64::from(self.template[low]) * (1.0 - weight) + f64::from(self.template[high]) * weight
    }

    fn correlation_at(&self, count: usize, bearing_deg: f64, mean_level: f64, s_ll: f64) -> f64 {
        let (mut sum_g, mut sum_gg, mut sum_lg) = (0.0, 0.0, 0.0);
        for (&heading, &level) in self.track.heading[..count]
            .iter()
            .zip(&self.track.level[..count])
        {
            let g = self.template_at(heading - bearing_deg);
            sum_g += g;
            sum_gg += g * g;
            sum_lg += (f64::from(level) - mean_level) * g;
        }
        let s_gg = sum_gg - sum_g * sum_g / count as f64;
        if s_gg <= 0.0 || s_ll <= 0.0 {
            return 0.0;
        }
        sum_lg / (s_ll * s_gg).sqrt()
    }

    fn level_moments(&self, count: usize) -> (f64, f64) {
        let levels = &self.track.level[..count];
        let mean = levels.iter().map(|&l| f64::from(l)).sum::<f64>() / count as f64;
        let s_ll = levels
            .iter()
            .map(|&l| (f64::from(l) - mean).powi(2))
            .sum::<f64>();
        (mean, s_ll)
    }

    fn fit(&mut self, count: usize, rate: f64, mean_sigma: f64) -> Result<Fit, SweepReject> {
        if count < MIN_FIT_SAMPLES {
            return Err(SweepReject::PoorFit);
        }
        let (mean, s_ll) = self.level_moments(count);
        for degree in 0..LIKELIHOOD_POINTS {
            self.correlation[degree] = self.correlation_at(count, degree as f64, mean, s_ll);
        }
        let best = (0..LIKELIHOOD_POINTS)
            .max_by(|&a, &b| self.correlation[a].total_cmp(&self.correlation[b]))
            .unwrap_or(0);
        if self.correlation[best] <= 0.0 {
            return Err(SweepReject::PoorFit);
        }
        let before = self.correlation[(best + LIKELIHOOD_POINTS - 1) % LIKELIHOOD_POINTS];
        let after = self.correlation[(best + 1) % LIKELIHOOD_POINTS];
        let bend = before - 2.0 * self.correlation[best] + after;
        let shift = if bend < 0.0 {
            (0.5 * (before - after) / bend).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        let bearing = best as f64 + shift;
        let fit = self
            .correlation_at(count, bearing, mean, s_ll)
            .max(self.correlation[best]);
        if fit < f64::from(self.config.min_fit) {
            return Err(SweepReject::PoorFit);
        }
        let rss = |rho: f64| s_ll * (1.0 - rho.max(0.0).powi(2));
        let rss_best = rss(fit);
        let noise_var = (rss_best / (count as f64 - 3.0).max(1.0)).max(1e-12 * s_ll);
        let scale = 2.0 * noise_var * self.config.sample_correlation;
        let raw = |b: f64| -(rss(self.correlation_at(count, b, mean, s_ll)) - rss_best) / scale;
        let h = CURVATURE_STEP_DEG;
        let curvature = -(raw(bearing + h) + raw(bearing - h)) / (h * h);
        let raw_var = if curvature > 0.0 {
            1.0 / curvature
        } else {
            WIDEST_SIGMA_DEG * WIDEST_SIGMA_DEG
        };
        let smear = rate * LEVEL_PERIOD_S;
        let sigma = (raw_var + mean_sigma * mean_sigma + smear * smear).sqrt();
        Ok(Fit {
            bearing_deg: bearing,
            fit,
            sigma_deg: sigma.min(WIDEST_SIGMA_DEG),
            curvature,
            s_ll,
            rss_best,
            noise_var,
        })
    }

    fn fill_likelihood(&self, fit: &Fit, out: &mut SweepBearing) {
        let sigma_sq = fit.sigma_deg * fit.sigma_deg;
        let temper = if fit.curvature > 0.0 {
            (1.0 / (fit.curvature * sigma_sq)).min(1.0)
        } else {
            1.0
        };
        let floor = LIKELIHOOD_FLOOR.ln();
        let scale = 2.0 * fit.noise_var * self.config.sample_correlation;
        for (value, &rho) in out.likelihood.iter_mut().zip(&self.correlation) {
            let rss = fit.s_ll * (1.0 - rho.max(0.0).powi(2));
            let raw = -(rss - fit.rss_best).max(0.0) / scale;
            *value = (temper * raw).max(floor) as f32;
        }
    }

    fn learn_lag(&mut self, bearing_deg: f64, rate_dps: f64, rotation: Rotation, at_s: f64) {
        let this = PastSweep {
            bearing_deg,
            rate_dps,
            lag_s: self.lag_s,
            at_s,
        };
        let (mine, other) = match rotation {
            Rotation::Clockwise => (0, 1),
            Rotation::Counterclockwise => (1, 0),
        };
        if let Some(opposite) = self.last[other]
            && (at_s - opposite.at_s).abs() <= PAIR_WINDOW_S
            && wrap_deg(bearing_deg - opposite.bearing_deg).abs() <= PAIR_WINDOW_DEG
        {
            let (cw, ccw) = if mine == 0 {
                (this, opposite)
            } else {
                (opposite, this)
            };
            let rates = cw.rate_dps + ccw.rate_dps;
            if rates > 0.0 {
                let split = wrap_deg(cw.bearing_deg - ccw.bearing_deg);
                let lag = (split + cw.rate_dps * cw.lag_s + ccw.rate_dps * ccw.lag_s) / rates;
                self.lag_s = (self.lag_s + LAG_GAIN * (lag - self.lag_s)).clamp(0.0, MAX_LAG_S);
            }
        }
        self.last[mine] = Some(this);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK_S: f64 = 0.05;

    struct Rng(u64);

    impl Rng {
        fn uniform(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }

        fn normal(&mut self) -> f64 {
            let u = self.uniform().max(1e-300);
            let v = self.uniform();
            (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
        }
    }

    struct Walk {
        legs: Vec<(f64, f64)>,
        start_deg: f64,
    }

    impl Walk {
        fn new(start_deg: f64) -> Self {
            Self {
                legs: Vec::new(),
                start_deg,
            }
        }

        fn hold(mut self, seconds: f64) -> Self {
            self.legs.push((seconds, 0.0));
            self
        }

        fn turn(mut self, degrees: f64, rate_dps: f64) -> Self {
            self.legs
                .push((degrees.abs() / rate_dps, degrees.signum() * rate_dps));
            self
        }

        fn duration(&self) -> f64 {
            self.legs.iter().map(|leg| leg.0).sum()
        }

        fn heading(&self, t: f64) -> f64 {
            let mut heading = self.start_deg;
            let mut left = t.max(0.0);
            for &(seconds, rate) in &self.legs {
                let used = left.min(seconds);
                heading += rate * used;
                left -= used;
            }
            heading
        }
    }

    struct Run {
        lag_s: f64,
        bearing_deg: f64,
        noise_db: f64,
        sigma_deg: f32,
        pattern_gain: f64,
        seed: u64,
    }

    impl Default for Run {
        fn default() -> Self {
            Self {
                lag_s: 0.1,
                bearing_deg: 123.0,
                noise_db: 2.0,
                sigma_deg: 1.0,
                pattern_gain: 1.0,
                seed: 17,
            }
        }
    }

    fn pattern(offset_deg: f64) -> f64 {
        let estimator = SweepEstimator::new(SweepConfig::default());
        estimator.template_at(offset_deg)
    }

    fn sweep(
        estimator: &mut SweepEstimator,
        walk: &Walk,
        run: &Run,
    ) -> Vec<Result<SweepBearing, SweepReject>> {
        let mut rng = Rng(run.seed);
        let mut out = SweepBearing::default();
        let mut results = Vec::new();
        let ticks = (walk.duration() / TICK_S).ceil() as usize + 1;
        for tick in 0..ticks {
            let t = tick as f64 * TICK_S;
            estimator.push_heading(HeadingSample {
                t_s: t,
                heading_deg: norm_deg(walk.heading(t)),
                sigma_deg: run.sigma_deg,
            });
            let heard = t + TICK_S / 2.0;
            let level = -60.0
                + run.pattern_gain * pattern(walk.heading(heard - run.lag_s) - run.bearing_deg)
                + run.noise_db * rng.normal();
            let sample = LevelSample {
                t_s: heard,
                level_db: level as f32,
            };
            if let Some(result) = estimator.push_level(sample, &mut out) {
                results.push(result.map(|()| out.clone()));
            }
        }
        results
    }

    fn one_sweep(run: &Run) -> SweepBearing {
        let walk = Walk::new(10.0).hold(1.0).turn(270.0, 90.0).hold(1.5);
        let mut estimator = SweepEstimator::new(SweepConfig::default());
        let results = sweep(&mut estimator, &walk, run);
        assert_eq!(results.len(), 1, "{results:?}");
        results[0].clone().unwrap()
    }

    #[test]
    fn a_sixty_degree_yagi_sweep_finds_the_bearing() {
        let bearing = one_sweep(&Run::default());
        assert!(
            wrap_deg(bearing.bearing_deg - 123.0).abs() < 3.0,
            "{bearing:?}"
        );
        assert_eq!(bearing.rotation, Rotation::Clockwise);
        assert!(bearing.fit > 0.8);
        assert!((bearing.span_deg - 270.0).abs() < 10.0);
        assert!((bearing.rate_dps - 90.0).abs() < 10.0);
        assert!(bearing.confidence > 0.3 && bearing.confidence < 1.0);
    }

    #[test]
    fn opposite_sweeps_estimate_and_remove_the_lag() {
        let walk = (0..3)
            .fold(Walk::new(10.0).hold(1.0), |walk, _| {
                walk.turn(270.0, 90.0).turn(-270.0, 90.0)
            })
            .turn(270.0, 90.0)
            .hold(1.5);
        let run = Run {
            lag_s: 0.15,
            noise_db: 0.5,
            ..Run::default()
        };
        let mut estimator = SweepEstimator::new(SweepConfig::default());
        let mut rng = Rng(3);
        let mut out = SweepBearing::default();
        let mut bearings = Vec::new();
        let mut lags = Vec::new();
        let ticks = (walk.duration() / TICK_S).ceil() as usize + 1;
        for tick in 0..ticks {
            let t = tick as f64 * TICK_S;
            estimator.push_heading(HeadingSample {
                t_s: t,
                heading_deg: norm_deg(walk.heading(t)),
                sigma_deg: 1.0,
            });
            let heard = t + TICK_S / 2.0;
            let level = -60.0
                + pattern(walk.heading(heard - run.lag_s) - run.bearing_deg)
                + run.noise_db * rng.normal();
            let sample = LevelSample {
                t_s: heard,
                level_db: level as f32,
            };
            if let Some(result) = estimator.push_level(sample, &mut out) {
                assert_eq!(result, Ok(()));
                bearings.push(out.bearing_deg);
                lags.push(estimator.lag_s());
            }
        }
        assert_eq!(bearings.len(), 7, "{bearings:?}");
        let first_bias = wrap_deg(bearings[0] - 123.0);
        assert!((first_bias - 4.5).abs() < 1.0, "first bias {first_bias}");
        assert!((lags[1] - 0.125).abs() < 0.01, "{lags:?}");
        assert!((lags[5] - 0.15).abs() < 0.01, "{lags:?}");
        assert!(wrap_deg(bearings[6] - 123.0).abs() < 3.0, "{bearings:?}");
    }

    #[test]
    fn flat_levels_are_rejected_as_low_contrast() {
        let walk = Walk::new(10.0).hold(1.0).turn(270.0, 90.0).hold(1.5);
        let run = Run {
            noise_db: 0.5,
            pattern_gain: 0.0,
            ..Run::default()
        };
        let mut estimator = SweepEstimator::new(SweepConfig::default());
        let results = sweep(&mut estimator, &walk, &run);
        assert_eq!(results, vec![Err(SweepReject::LowContrast)]);
    }

    #[test]
    fn a_short_swing_is_rejected() {
        let walk = Walk::new(80.0).hold(1.0).turn(90.0, 90.0).hold(1.5);
        let mut estimator = SweepEstimator::new(SweepConfig::default());
        let results = sweep(&mut estimator, &walk, &Run::default());
        assert_eq!(results, vec![Err(SweepReject::ShortSpan)]);
        assert!(estimator.covered_deg() >= 90.0);
        assert!(estimator.covered_deg() <= 110.0);
        assert_eq!(estimator.phase(), SweepPhase::Idle);
    }

    #[test]
    fn heading_sigma_widens_the_bearing_sigma() {
        let sharp = one_sweep(&Run::default());
        let vague = one_sweep(&Run {
            sigma_deg: 10.0,
            ..Run::default()
        });
        assert!(
            vague.sigma_deg >= sharp.sigma_deg + 5.0,
            "{} vs {}",
            vague.sigma_deg,
            sharp.sigma_deg
        );
        assert!(vague.confidence < sharp.confidence);
    }

    #[test]
    fn the_sweep_likelihood_respects_front_to_back() {
        let bearing = one_sweep(&Run::default());
        let floor = LIKELIHOOD_FLOOR.ln() as f32;
        let peak = bearing.bearing_deg.round() as usize % 360;
        let back = (peak + 180) % 360;
        assert!(
            bearing.likelihood[back] - floor < 0.05,
            "{}",
            bearing.likelihood[back]
        );
        assert!(
            bearing.likelihood[peak] > -0.5,
            "{}",
            bearing.likelihood[peak]
        );
        assert!(
            bearing
                .likelihood
                .iter()
                .all(|&value| value <= 0.0 && value >= floor)
        );
    }

    #[test]
    fn a_fast_spin_is_rejected_as_too_fast() {
        let walk = Walk::new(0.0).hold(1.0).turn(720.0, 600.0).hold(1.5);
        let mut estimator = SweepEstimator::new(SweepConfig::default());
        let results = sweep(&mut estimator, &walk, &Run::default());
        assert_eq!(results.first(), Some(&Err(SweepReject::TooFast)));
    }

    #[test]
    fn levels_without_a_heading_are_dropped() {
        let mut estimator = SweepEstimator::new(SweepConfig::default());
        let mut out = SweepBearing::default();
        let sample = LevelSample {
            t_s: 3.0,
            level_db: -50.0,
        };
        assert_eq!(estimator.push_level(sample, &mut out), None);
        assert_eq!(estimator.phase(), SweepPhase::Idle);
        assert_eq!(estimator.dropped_levels(), 1);
        estimator.push_heading(HeadingSample {
            t_s: 1.0,
            heading_deg: 350.0,
            sigma_deg: 2.0,
        });
        estimator.push_heading(HeadingSample {
            t_s: 1.5,
            heading_deg: 10.0,
            sigma_deg: 4.0,
        });
        estimator.push_heading(HeadingSample {
            t_s: 1.2,
            heading_deg: 0.0,
            sigma_deg: 4.0,
        });
        assert_eq!(estimator.stale_headings(), 1);
        let middle = estimator.heading_at(1.25).unwrap();
        assert!(middle.heading_deg.abs() < 1e-9 || (middle.heading_deg - 360.0).abs() < 1e-9);
        assert!((middle.sigma_deg - 3.0).abs() < 1e-6);
        assert!(estimator.heading_at(2.1).is_none());
        assert!((estimator.heading_at(1.7).unwrap().heading_deg - 18.0).abs() < 1e-9);
    }

    #[test]
    fn opposite_turns_inside_one_sweep_close_it_at_the_turn() {
        let walk = Walk::new(10.0)
            .hold(1.0)
            .turn(270.0, 90.0)
            .turn(-270.0, 90.0)
            .hold(1.5);
        let mut estimator = SweepEstimator::new(SweepConfig::default());
        let results = sweep(&mut estimator, &walk, &Run::default());
        assert_eq!(results.len(), 2, "{results:?}");
        let rotations: Vec<_> = results
            .iter()
            .map(|result| result.as_ref().map(|bearing| bearing.rotation))
            .collect();
        assert_eq!(
            rotations,
            vec![Ok(Rotation::Clockwise), Ok(Rotation::Counterclockwise)]
        );
    }
}
