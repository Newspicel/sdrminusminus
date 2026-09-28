use super::RadarDspError;
use super::assign::Assignment;

pub const MAX_TRACKS: usize = 64;
pub const MAX_TENTATIVE: usize = 128;
pub const MAX_MEASUREMENTS: usize = 128;
pub const MAX_WINDOW: u32 = 16;
pub const TRAIL_LEN: usize = 16;
pub const AOA_HISTORY: usize = 5;

const MIN_NOISE_FACTOR: f64 = 0.05;
const MAX_NOISE_FACTOR: f64 = 0.5;
const ACCEL_GATE_SIGMAS: f64 = 3.0;

type Mat3 = [[f64; 3]; 3];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackerConfig {
    pub wavelength_m: f64,
    pub confirm_hits: u32,
    pub confirm_window: u32,
    pub coast_looks: u32,
    pub max_accel: f64,
    pub gate: f64,
    pub jerk: f64,
    pub range_resolution_m: f64,
    pub doppler_resolution_hz: f64,
}

impl TrackerConfig {
    #[must_use]
    pub fn valid(&self) -> bool {
        let positive = |value: f64| value.is_finite() && value > 0.0;
        positive(self.wavelength_m)
            && positive(self.max_accel)
            && positive(self.gate)
            && positive(self.jerk)
            && positive(self.range_resolution_m)
            && positive(self.doppler_resolution_hz)
            && self.confirm_hits >= 1
            && self.confirm_hits <= self.confirm_window
            && self.confirm_window <= MAX_WINDOW
    }

    fn noise(&self, snr: f64) -> (f64, f64) {
        let factor = if snr > 0.0 && snr.is_finite() {
            (1.0 / (2.0 * snr).sqrt()).clamp(MIN_NOISE_FACTOR, MAX_NOISE_FACTOR)
        } else {
            MAX_NOISE_FACTOR
        };
        (
            factor * self.range_resolution_m,
            factor * self.doppler_resolution_hz,
        )
    }

    const fn window_mask(&self) -> u32 {
        (1u32 << self.confirm_window) - 1
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrackAoa {
    pub azimuth_deg: f32,
    pub quality: f32,
    pub sigma_deg: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measurement {
    pub range_m: f64,
    pub doppler_hz: f64,
    pub snr: f64,
    pub aoa: Option<TrackAoa>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackView {
    pub id: u32,
    pub coasting: bool,
    pub range_m: f64,
    pub range_rate_mps: f64,
    pub accel_mps2: f64,
    pub range_sigma_m: f64,
    pub rate_sigma_mps: f64,
    pub doppler_hz: f64,
    pub snr: f64,
    pub looks: u32,
    pub misses: u32,
    pub aoa: Option<TrackAoa>,
    pub trail: [(f32, f32); TRAIL_LEN],
    pub trail_len: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackerStats {
    pub dropped: u32,
    pub ignored: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Kalman {
    x: [f64; 3],
    p: Mat3,
}

struct Innovation {
    residual: [f64; 2],
    inverse: [[f64; 2]; 2],
    distance: f64,
    log_det: f64,
}

impl Kalman {
    fn start(measurement: &Measurement, config: &TrackerConfig) -> Self {
        let (sigma_range, sigma_doppler) = config.noise(measurement.snr);
        let lambda = config.wavelength_m;
        let rate_sigma = lambda * sigma_doppler;
        let accel_sigma = config.max_accel / 2.0;
        Self {
            x: [measurement.range_m, -lambda * measurement.doppler_hz, 0.0],
            p: [
                [sigma_range * sigma_range, 0.0, 0.0],
                [0.0, rate_sigma * rate_sigma, 0.0],
                [0.0, 0.0, accel_sigma * accel_sigma],
            ],
        }
    }

    fn predict(&mut self, dt: f64, jerk: f64) {
        let f = [[1.0, dt, 0.5 * dt * dt], [0.0, 1.0, dt], [0.0, 0.0, 1.0]];
        let x = self.x;
        for (row, out) in f.iter().zip(self.x.iter_mut()) {
            *out = row[0] * x[0] + row[1] * x[1] + row[2] * x[2];
        }
        let mut p = multiply_transposed(&multiply(&f, &self.p), &f);
        let (t2, t3) = (dt * dt, dt * dt * dt);
        let (t4, t5) = (t3 * dt, t3 * t2);
        let q = [
            [t5 / 20.0, t4 / 8.0, t3 / 6.0],
            [t4 / 8.0, t3 / 3.0, t2 / 2.0],
            [t3 / 6.0, t2 / 2.0, dt],
        ];
        for (row, q_row) in p.iter_mut().zip(&q) {
            for (value, q_value) in row.iter_mut().zip(q_row) {
                *value += jerk * q_value;
            }
        }
        self.p = p;
    }

    fn doppler_hz(&self, lambda: f64) -> f64 {
        -self.x[1] / lambda
    }

    fn innovation(&self, measurement: &Measurement, config: &TrackerConfig) -> Option<Innovation> {
        let lambda = config.wavelength_m;
        let (sigma_range, sigma_doppler) = config.noise(measurement.snr);
        let p = &self.p;
        let s = [
            [p[0][0] + sigma_range * sigma_range, -p[0][1] / lambda],
            [
                -p[1][0] / lambda,
                p[1][1] / (lambda * lambda) + sigma_doppler * sigma_doppler,
            ],
        ];
        let det = s[0][0] * s[1][1] - s[0][1] * s[1][0];
        if !(det > 0.0 && det.is_finite()) {
            return None;
        }
        let inverse = [
            [s[1][1] / det, -s[0][1] / det],
            [-s[1][0] / det, s[0][0] / det],
        ];
        let residual = [
            measurement.range_m - self.x[0],
            measurement.doppler_hz - self.doppler_hz(lambda),
        ];
        let distance = residual[0] * (inverse[0][0] * residual[0] + inverse[0][1] * residual[1])
            + residual[1] * (inverse[1][0] * residual[0] + inverse[1][1] * residual[1]);
        Some(Innovation {
            residual,
            inverse,
            distance,
            log_det: det.ln(),
        })
    }

    fn correct(&mut self, measurement: &Measurement, config: &TrackerConfig) -> bool {
        let Some(innovation) = self.innovation(measurement, config) else {
            return false;
        };
        let lambda = config.wavelength_m;
        let (sigma_range, sigma_doppler) = config.noise(measurement.snr);
        let p = self.p;
        let pht = [
            [p[0][0], -p[0][1] / lambda],
            [p[1][0], -p[1][1] / lambda],
            [p[2][0], -p[2][1] / lambda],
        ];
        let inverse = innovation.inverse;
        let mut gain = [[0.0; 2]; 3];
        for (row, source) in gain.iter_mut().zip(&pht) {
            for (col, value) in row.iter_mut().enumerate() {
                *value = source[0] * inverse[0][col] + source[1] * inverse[1][col];
            }
        }
        let mut x = self.x;
        for (value, row) in x.iter_mut().zip(&gain) {
            *value += row[0] * innovation.residual[0] + row[1] * innovation.residual[1];
        }
        let mut reduce = [[0.0; 3]; 3];
        for (i, row) in reduce.iter_mut().enumerate() {
            row[i] = 1.0;
            row[0] -= gain[i][0];
            row[1] += gain[i][1] / lambda;
        }
        let mut updated = multiply_transposed(&multiply(&reduce, &p), &reduce);
        let noise = [sigma_range * sigma_range, sigma_doppler * sigma_doppler];
        for (i, row) in updated.iter_mut().enumerate() {
            for (j, value) in row.iter_mut().enumerate() {
                *value += gain[i][0] * noise[0] * gain[j][0] + gain[i][1] * noise[1] * gain[j][1];
            }
        }
        let total: f64 = x.iter().chain(updated.iter().flatten()).sum();
        if !total.is_finite() {
            return false;
        }
        self.x = x;
        self.p = updated;
        true
    }
}

fn multiply(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, value) in row.iter_mut().enumerate() {
            *value = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

fn multiply_transposed(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, value) in row.iter_mut().enumerate() {
            *value = (0..3).map(|k| a[i][k] * b[j][k]).sum();
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Track {
    id: u32,
    filter: Kalman,
    history: u32,
    looks: u32,
    misses: u32,
    snr: f64,
    hit: Option<usize>,
    aoa: [TrackAoa; AOA_HISTORY],
    aoa_len: u8,
    aoa_next: u8,
    trail: [(f32, f32); TRAIL_LEN],
    trail_len: u8,
    trail_next: u8,
}

impl Track {
    fn start(measurement: &Measurement, index: usize, config: &TrackerConfig) -> Self {
        let mut track = Self {
            id: 0,
            filter: Kalman::start(measurement, config),
            history: 1,
            looks: 1,
            misses: 0,
            snr: measurement.snr,
            hit: Some(index),
            aoa: [TrackAoa::default(); AOA_HISTORY],
            aoa_len: 0,
            aoa_next: 0,
            trail: [(0.0, 0.0); TRAIL_LEN],
            trail_len: 0,
            trail_next: 0,
        };
        track.remember(measurement.aoa);
        track.mark(config.wavelength_m);
        track
    }

    fn cost(&self, measurement: &Measurement, config: &TrackerConfig, dt: f64) -> Option<f64> {
        let innovation = self.filter.innovation(measurement, config)?;
        if innovation.distance.is_nan() || innovation.distance > config.gate {
            return None;
        }
        if self.id == 0 {
            let (_, sigma_doppler) = config.noise(measurement.snr);
            let reach =
                config.max_accel * dt / config.wavelength_m + ACCEL_GATE_SIGMAS * sigma_doppler;
            if innovation.residual[1].abs() > reach {
                return None;
            }
        }
        Some(innovation.distance + innovation.log_det)
    }

    fn look(&mut self, measurement: Option<(usize, &Measurement)>, config: &TrackerConfig) {
        self.looks = self.looks.saturating_add(1);
        self.history <<= 1;
        self.hit = None;
        match measurement {
            Some((index, measurement)) if self.filter.correct(measurement, config) => {
                self.history |= 1;
                self.misses = 0;
                self.snr = measurement.snr;
                self.hit = Some(index);
                self.remember(measurement.aoa);
            }
            _ => self.misses += 1,
        }
        self.history &= config.window_mask();
        self.mark(config.wavelength_m);
    }

    fn remember(&mut self, aoa: Option<TrackAoa>) {
        if let Some(aoa) = aoa.filter(|aoa| aoa.azimuth_deg.is_finite() && aoa.quality > 0.0) {
            self.aoa[usize::from(self.aoa_next)] = aoa;
            self.aoa_next = (self.aoa_next + 1) % AOA_HISTORY as u8;
            self.aoa_len = (self.aoa_len + 1).min(AOA_HISTORY as u8);
        }
    }

    fn mark(&mut self, lambda: f64) {
        let point = (
            (self.filter.x[0] / 1000.0) as f32,
            self.filter.doppler_hz(lambda) as f32,
        );
        self.trail[usize::from(self.trail_next)] = point;
        self.trail_next = (self.trail_next + 1) % TRAIL_LEN as u8;
        self.trail_len = (self.trail_len + 1).min(TRAIL_LEN as u8);
    }

    fn mean_aoa(&self) -> Option<TrackAoa> {
        let held = &self.aoa[..usize::from(self.aoa_len)];
        if held.is_empty() {
            return None;
        }
        let (mut sin, mut cos, mut quality) = (0.0f64, 0.0f64, 0.0f64);
        let mut sharpest = f32::INFINITY;
        for aoa in held {
            let (s, c) = f64::from(aoa.azimuth_deg).to_radians().sin_cos();
            let weight = f64::from(aoa.quality);
            sin += weight * s;
            cos += weight * c;
            quality += weight;
            sharpest = sharpest.min(aoa.sigma_deg);
        }
        let count = held.len() as f64;
        Some(TrackAoa {
            azimuth_deg: crate::special::norm_deg(sin.atan2(cos).to_degrees()) as f32,
            quality: (quality / count) as f32,
            sigma_deg: (f64::from(sharpest) / count.sqrt()) as f32,
        })
    }

    fn view(&self, lambda: f64) -> TrackView {
        let mut trail = [(0.0f32, 0.0f32); TRAIL_LEN];
        let len = usize::from(self.trail_len);
        let start = (usize::from(self.trail_next) + TRAIL_LEN - len) % TRAIL_LEN;
        for (offset, point) in trail.iter_mut().take(len).enumerate() {
            *point = self.trail[(start + offset) % TRAIL_LEN];
        }
        let p = &self.filter.p;
        TrackView {
            id: self.id,
            coasting: self.misses > 0,
            range_m: self.filter.x[0],
            range_rate_mps: self.filter.x[1],
            accel_mps2: self.filter.x[2],
            range_sigma_m: p[0][0].max(0.0).sqrt(),
            rate_sigma_mps: p[1][1].max(0.0).sqrt(),
            doppler_hz: self.filter.doppler_hz(lambda),
            snr: self.snr,
            looks: self.looks,
            misses: self.misses,
            aoa: self.mean_aoa(),
            trail,
            trail_len: self.trail_len,
        }
    }
}

pub struct Tracker {
    config: TrackerConfig,
    confirmed: Vec<Track>,
    tentative: Vec<Track>,
    assignment: Assignment,
    matches: Vec<Option<usize>>,
    free: Vec<usize>,
    used: Vec<bool>,
    next_id: u32,
}

impl Tracker {
    pub fn new(config: TrackerConfig) -> Result<Self, RadarDspError> {
        if !config.valid() {
            return Err(RadarDspError::Setting);
        }
        let rows = MAX_TENTATIVE.max(MAX_TRACKS);
        Ok(Self {
            config,
            confirmed: Vec::with_capacity(MAX_TRACKS),
            tentative: Vec::with_capacity(MAX_TENTATIVE),
            assignment: Assignment::new(rows, MAX_MEASUREMENTS),
            matches: vec![None; rows],
            free: Vec::with_capacity(MAX_MEASUREMENTS),
            used: vec![false; MAX_MEASUREMENTS],
            next_id: 1,
        })
    }

    pub fn set_config(&mut self, config: TrackerConfig) -> Result<(), RadarDspError> {
        if !config.valid() {
            return Err(RadarDspError::Setting);
        }
        self.config = config;
        let mask = config.window_mask();
        for track in self.confirmed.iter_mut().chain(self.tentative.iter_mut()) {
            track.history &= mask;
        }
        Ok(())
    }

    #[must_use]
    pub const fn config(&self) -> TrackerConfig {
        self.config
    }

    #[must_use]
    pub fn confirmed(&self) -> usize {
        self.confirmed.len()
    }

    #[must_use]
    pub fn tentative(&self) -> usize {
        self.tentative.len()
    }

    #[must_use]
    pub const fn next_id(&self) -> u32 {
        self.next_id
    }

    pub fn resume_ids(&mut self, next_id: u32) {
        self.next_id = self.next_id.max(next_id);
    }

    pub fn reset(&mut self, ended: &mut Vec<u32>) {
        ended.clear();
        ended.extend(self.confirmed.iter().map(|track| track.id));
        self.confirmed.clear();
        self.tentative.clear();
    }

    pub fn update(
        &mut self,
        dt_s: f64,
        measurements: &[Measurement],
        assigned: &mut Vec<Option<u32>>,
        ended: &mut Vec<u32>,
    ) -> TrackerStats {
        let count = measurements.len().min(MAX_MEASUREMENTS);
        let mut stats = TrackerStats {
            dropped: 0,
            ignored: (measurements.len() - count) as u32,
        };
        let measurements = &measurements[..count];
        ended.clear();
        assigned.clear();
        assigned.resize(count, None);
        self.used[..count].fill(false);
        let dt = if dt_s.is_finite() && dt_s > 0.0 {
            dt_s
        } else {
            0.0
        };
        for track in self.confirmed.iter_mut().chain(self.tentative.iter_mut()) {
            track.filter.predict(dt, self.config.jerk);
        }
        stats.ignored += self.associate_confirmed(measurements, dt);
        stats.ignored += self.associate_tentative(measurements, dt);
        stats.ignored += self.unsound(measurements);
        stats.dropped += self.start_tentatives(measurements);
        stats.dropped += self.promote(ended);
        self.retire(ended);
        for track in &self.confirmed {
            if let Some(slot) = track.hit.and_then(|index| assigned.get_mut(index)) {
                *slot = Some(track.id);
            }
        }
        stats
    }

    pub fn for_each_confirmed(&self, mut f: impl FnMut(&TrackView)) {
        for track in &self.confirmed {
            f(&track.view(self.config.wavelength_m));
        }
    }

    fn associate_confirmed(&mut self, measurements: &[Measurement], dt: f64) -> u32 {
        let rows = self.confirmed.len();
        let config = self.config;
        let tracks = &self.confirmed;
        let solved = self.assignment.solve(
            rows,
            measurements.len(),
            |row, col| tracks[row].cost(&measurements[col], &config, dt),
            &mut self.matches[..rows],
        );
        let ignored = if solved.is_err() {
            self.matches[..rows].fill(None);
            measurements.len() as u32
        } else {
            0
        };
        for (track, pick) in self.confirmed.iter_mut().zip(&self.matches) {
            if let Some(col) = *pick {
                self.used[col] = true;
            }
            track.look(pick.map(|col| (col, &measurements[col])), &config);
        }
        ignored
    }

    fn associate_tentative(&mut self, measurements: &[Measurement], dt: f64) -> u32 {
        self.free.clear();
        let used = &self.used;
        self.free
            .extend((0..measurements.len()).filter(|&index| !used[index]));
        let rows = self.tentative.len();
        let config = self.config;
        let tracks = &self.tentative;
        let free = &self.free;
        let solved = self.assignment.solve(
            rows,
            free.len(),
            |row, col| tracks[row].cost(&measurements[free[col]], &config, dt),
            &mut self.matches[..rows],
        );
        let ignored = if solved.is_err() {
            self.matches[..rows].fill(None);
            free.len() as u32
        } else {
            0
        };
        for (track, pick) in self.tentative.iter_mut().zip(&self.matches) {
            let hit = pick.map(|col| self.free[col]);
            if let Some(index) = hit {
                self.used[index] = true;
            }
            track.look(hit.map(|index| (index, &measurements[index])), &config);
        }
        ignored
    }

    fn unsound(&self, measurements: &[Measurement]) -> u32 {
        measurements
            .iter()
            .zip(&self.used)
            .filter(|(measurement, used)| !**used && !measurement_is_sound(measurement))
            .count() as u32
    }

    fn start_tentatives(&mut self, measurements: &[Measurement]) -> u32 {
        let mut dropped = 0;
        for (index, measurement) in measurements.iter().enumerate() {
            if self.used[index] || !measurement_is_sound(measurement) {
                continue;
            }
            let track = Track::start(measurement, index, &self.config);
            if self.tentative.len() < MAX_TENTATIVE {
                self.tentative.push(track);
                continue;
            }
            dropped += 1;
            if let Some(weak) = weakest(&self.tentative)
                && self.tentative[weak].snr < track.snr
            {
                self.tentative[weak] = track;
            }
        }
        dropped
    }

    fn promote(&mut self, ended: &mut Vec<u32>) -> u32 {
        let mut dropped = 0;
        let hits = self.config.confirm_hits;
        let mut index = 0;
        while index < self.tentative.len() {
            if self.tentative[index].history.count_ones() < hits {
                index += 1;
                continue;
            }
            let mut track = self.tentative.remove(index);
            if self.confirmed.len() >= MAX_TRACKS {
                dropped += 1;
                match weakest(&self.confirmed) {
                    Some(weak) if self.confirmed[weak].snr < track.snr => {
                        ended.push(self.confirmed[weak].id);
                        self.confirmed.remove(weak);
                    }
                    _ => continue,
                }
            }
            track.id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1).max(1);
            self.confirmed.push(track);
        }
        dropped
    }

    fn retire(&mut self, ended: &mut Vec<u32>) {
        let window = self.config.confirm_window;
        self.tentative.retain(|track| track.looks < window);
        let limit = self.config.coast_looks;
        self.confirmed.retain(|track| {
            let alive = track.misses <= limit;
            if !alive {
                ended.push(track.id);
            }
            alive
        });
    }
}

fn measurement_is_sound(measurement: &Measurement) -> bool {
    measurement.range_m.is_finite() && measurement.doppler_hz.is_finite()
}

fn weakest(tracks: &[Track]) -> Option<usize> {
    tracks
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.snr.total_cmp(&b.1.snr))
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAMBDA: f64 = 2.997_924_58;
    const DT: f64 = 0.5;

    fn config() -> TrackerConfig {
        TrackerConfig {
            wavelength_m: LAMBDA,
            confirm_hits: 3,
            confirm_window: 5,
            coast_looks: 10,
            max_accel: 30.0,
            gate: 11.8,
            jerk: 5.0,
            range_resolution_m: 1124.22,
            doppler_resolution_hz: 3.0,
        }
    }

    fn measure(range_m: f64, rate_mps: f64) -> Measurement {
        Measurement {
            range_m,
            doppler_hz: -rate_mps / LAMBDA,
            snr: 100.0,
            aoa: None,
        }
    }

    fn target(look: u32, range_m: f64, rate_mps: f64) -> Measurement {
        measure(range_m + rate_mps * DT * f64::from(look), rate_mps)
    }

    struct Run {
        tracker: Tracker,
        assigned: Vec<Option<u32>>,
        ended: Vec<u32>,
    }

    impl Run {
        fn new(config: TrackerConfig) -> Self {
            Self {
                tracker: Tracker::new(config).unwrap(),
                assigned: Vec::new(),
                ended: Vec::new(),
            }
        }

        fn step(&mut self, measurements: &[Measurement]) -> TrackerStats {
            self.tracker
                .update(DT, measurements, &mut self.assigned, &mut self.ended)
        }

        fn views(&self) -> Vec<TrackView> {
            let mut views = Vec::new();
            self.tracker.for_each_confirmed(|view| views.push(*view));
            views
        }
    }

    #[test]
    fn three_of_five_confirms_and_names() {
        let mut run = Run::new(config());
        for look in 0..2 {
            run.step(&[target(look, 20_000.0, -100.0)]);
            assert!(run.views().is_empty());
            assert_eq!(run.assigned, vec![None]);
        }
        run.step(&[target(2, 20_000.0, -100.0)]);
        let views = run.views();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].id, 1);
        assert_eq!(views[0].looks, 3);
        assert_eq!(run.assigned, vec![Some(1)]);
        assert!((views[0].range_rate_mps + 100.0).abs() < 2.0);
        run.step(&[]);
        run.step(&[target(4, 20_000.0, -100.0)]);
        let views = run.views();
        assert_eq!(views[0].id, 1);
        assert_eq!(views[0].trail_len, 5);
        assert!((views[0].trail[4].0 - 19.8).abs() < 0.05);
    }

    #[test]
    fn tentatives_do_not_consume_ids() {
        let mut run = Run::new(config());
        for look in 0..6u32 {
            let clutter: Vec<Measurement> = (0..5)
                .map(|k| {
                    let spread = f64::from(look * 5 + k);
                    measure(3_000.0 + 7_000.0 * spread, 150.0 - 11.0 * spread)
                })
                .collect();
            run.step(&clutter);
            assert!(run.views().is_empty());
        }
        for look in 0..3 {
            run.step(&[target(look, 50_000.0, 80.0)]);
        }
        assert_eq!(run.views()[0].id, 1);
    }

    #[test]
    fn range_rate_comes_from_doppler_on_the_first_look() {
        let mut run = Run::new(TrackerConfig {
            confirm_hits: 1,
            confirm_window: 1,
            ..config()
        });
        run.step(&[measure(30_000.0, 0.0), measure(40_000.0, 0.0)]);
        let mut measurement = measure(12_345.0, 0.0);
        measurement.doppler_hz = 40.0;
        run.step(&[measurement]);
        let views = run.views();
        let view = views.iter().find(|view| view.id == 3).unwrap();
        assert!((view.range_rate_mps + LAMBDA * 40.0).abs() < 1e-9);
        assert!((view.doppler_hz - 40.0).abs() < 1e-9);
        assert!((view.range_m - 12_345.0).abs() < 1e-9);
    }

    #[test]
    fn a_turn_within_max_accel_is_followed() {
        let mut run = Run::new(config());
        let accel = 20.0;
        for look in 0..30u32 {
            let t = DT * f64::from(look);
            let rate = -150.0 + accel * t;
            let range = 40_000.0 - 150.0 * t + 0.5 * accel * t * t;
            run.step(&[measure(range, rate)]);
            if look >= 2 {
                let views = run.views();
                assert_eq!(views.len(), 1, "look {look}");
                assert_eq!(views[0].id, 1);
                assert_eq!(views[0].misses, 0, "look {look}");
            }
        }
        let view = run.views()[0];
        assert!((view.accel_mps2 - accel).abs() < 5.0, "{}", view.accel_mps2);
    }

    #[test]
    fn an_impossible_doppler_jump_starts_no_track() {
        let jumping = |look: u32| Measurement {
            doppler_hz: 7.0 * f64::from(look),
            ..measure(20_000.0, 0.0)
        };
        let mut run = Run::new(config());
        for look in 0..12u32 {
            run.step(&[jumping(look)]);
        }
        assert!(run.views().is_empty());
        let mut agile = Run::new(TrackerConfig {
            max_accel: 200.0,
            ..config()
        });
        for look in 0..3u32 {
            agile.step(&[jumping(look)]);
        }
        assert_eq!(agile.views().len(), 1);
    }

    #[test]
    fn coasting_tracks_are_reported_then_ended() {
        let mut run = Run::new(TrackerConfig {
            coast_looks: 3,
            ..config()
        });
        for look in 0..3 {
            run.step(&[target(look, 20_000.0, 50.0)]);
        }
        for miss in 1..=3u32 {
            run.step(&[]);
            let views = run.views();
            assert_eq!(views.len(), 1);
            assert!(views[0].coasting);
            assert_eq!(views[0].misses, miss);
            assert!(run.ended.is_empty());
        }
        run.step(&[]);
        assert!(run.views().is_empty());
        assert_eq!(run.ended, vec![1]);
    }

    #[test]
    fn crossing_targets_keep_their_ids() {
        let mut run = Run::new(config());
        let mut opener_id = None;
        for look in 0..40u32 {
            let opening = target(look, 30_000.0, 120.0);
            let closing = target(look, 34_000.0, -120.0);
            run.step(&[closing, opening]);
            if look == 2 {
                opener_id = run.assigned[1];
            }
            if look >= 2 {
                let views = run.views();
                assert_eq!(views.len(), 2, "look {look}");
                assert_eq!(run.assigned[1], opener_id, "look {look}");
                let opener = views.iter().find(|view| Some(view.id) == opener_id);
                assert!(opener.is_some_and(|view| view.range_rate_mps > 100.0));
            }
        }
    }

    #[test]
    fn ended_ids_are_listed_once() {
        let mut run = Run::new(TrackerConfig {
            coast_looks: 0,
            ..config()
        });
        for look in 0..3 {
            run.step(&[target(look, 20_000.0, 50.0)]);
        }
        run.step(&[]);
        assert_eq!(run.ended, vec![1]);
        for _ in 0..5 {
            run.step(&[]);
            assert!(run.ended.is_empty());
        }
    }

    #[test]
    fn reset_ends_every_confirmed_track() {
        let mut run = Run::new(config());
        for look in 0..3 {
            run.step(&[target(look, 20_000.0, 50.0), target(look, 60_000.0, -70.0)]);
        }
        assert_eq!(run.views().len(), 2);
        let mut ended = Vec::new();
        run.tracker.reset(&mut ended);
        ended.sort_unstable();
        assert_eq!(ended, vec![1, 2]);
        assert!(run.views().is_empty());
        for look in 0..3 {
            run.step(&[target(look, 20_000.0, 50.0)]);
        }
        assert_eq!(run.views()[0].id, 3);
    }

    #[test]
    fn resumed_ids_continue_after_a_rebuild() {
        let mut first = Run::new(config());
        for look in 0..3 {
            first.step(&[target(look, 20_000.0, 50.0)]);
        }
        assert_eq!(first.tracker.next_id(), 2);
        let mut second = Run::new(config());
        second.tracker.resume_ids(first.tracker.next_id());
        second.tracker.resume_ids(1);
        for look in 0..3 {
            second.step(&[target(look, 30_000.0, 40.0)]);
        }
        assert_eq!(second.views()[0].id, 2);
    }

    #[test]
    fn too_many_tentatives_drop_the_weakest_and_count() {
        let mut run = Run::new(config());
        let flood: Vec<Measurement> = (0..MAX_MEASUREMENTS + 10)
            .map(|k| Measurement {
                snr: 10.0 + k as f64,
                ..measure(1_000.0 + 3_000.0 * k as f64, 0.0)
            })
            .collect();
        let stats = run.step(&flood);
        assert_eq!(stats.ignored, 10);
        assert_eq!(run.tracker.tentative(), MAX_TENTATIVE);
        let strong = [Measurement {
            snr: 1e6,
            ..measure(900_000.0, 0.0)
        }];
        let stats = run.step(&strong);
        assert_eq!(stats.dropped, 1);
        assert_eq!(run.tracker.tentative(), MAX_TENTATIVE);
    }

    #[test]
    fn aoa_is_a_quality_weighted_circular_mean() {
        let mut run = Run::new(config());
        for (look, azimuth) in [359.0f32, 1.0, 3.0].into_iter().enumerate() {
            let measurement = Measurement {
                aoa: Some(TrackAoa {
                    azimuth_deg: azimuth,
                    quality: 0.9,
                    sigma_deg: 4.0,
                }),
                ..target(look as u32, 20_000.0, 50.0)
            };
            run.step(&[measurement]);
        }
        let aoa = run.views()[0].aoa.unwrap();
        assert!((aoa.azimuth_deg - 1.0).abs() < 1e-3, "{}", aoa.azimuth_deg);
        assert!((aoa.sigma_deg - 4.0 / 3f32.sqrt()).abs() < 1e-4);
    }

    #[test]
    fn unsound_measurements_are_counted_not_tracked() {
        let mut run = Run::new(config());
        let broken = Measurement {
            range_m: f64::NAN,
            ..measure(20_000.0, 50.0)
        };
        let stats = run.step(&[broken, measure(30_000.0, 0.0)]);
        assert_eq!(stats.ignored, 1);
        assert_eq!(run.tracker.tentative(), 1);
        assert_eq!(run.assigned, vec![None, None]);
    }

    #[test]
    fn invalid_configs_are_refused() {
        let over = TrackerConfig {
            confirm_hits: 6,
            ..config()
        };
        assert!(Tracker::new(over).is_err());
        let flat = TrackerConfig {
            wavelength_m: 0.0,
            ..config()
        };
        assert!(Tracker::new(flat).is_err());
        let mut tracker = Tracker::new(config()).unwrap();
        let wide = TrackerConfig {
            confirm_window: 17,
            ..config()
        };
        assert!(tracker.set_config(wide).is_err());
    }
}
