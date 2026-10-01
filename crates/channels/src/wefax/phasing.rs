use super::{PULSE_FRACTION, track::Track};

const MIN_LINES: u32 = 5;
const PULSE_LEVEL: f64 = 0.6;
const REST_LEVEL: f64 = 0.15;
const SPACING_TOLERANCE: f64 = 0.02;
const PERIOD_TOLERANCE: f64 = 0.02;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Lock {
    pub origin: f64,
    pub period: f64,
    pub first: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Scan {
    Waiting,
    Searched,
    Locked(Lock),
}

#[derive(Clone, Copy, Default)]
struct Fit {
    count: u32,
    first: f64,
    last: f64,
    k: f64,
    p: f64,
    kk: f64,
    kp: f64,
}

impl Fit {
    fn add(&mut self, center: f64) {
        if self.count == 0 {
            self.first = center;
        }
        let k = f64::from(self.count);
        let p = center - self.first;
        self.k += k;
        self.p += p;
        self.kk += k * k;
        self.kp += k * p;
        self.count += 1;
        self.last = center;
    }

    fn lock(&self, nominal: f64) -> Lock {
        let n = f64::from(self.count);
        let slope = (n * self.kp - self.k * self.p) / (n * self.kk - self.k * self.k);
        let period = if slope.is_finite() && (slope / nominal - 1.0).abs() <= PERIOD_TOLERANCE {
            slope
        } else {
            nominal
        };
        let intercept = (self.p - period * self.k) / n;
        let lead = period * PULSE_FRACTION / 2.0;
        Lock {
            origin: self.first + intercept + period * n - lead,
            period,
            first: (self.first - lead).max(0.0) as u64,
        }
    }
}

struct Pulse {
    center: u64,
    level: f64,
    rest: f64,
}

pub(super) struct Phasing {
    cursor: u64,
    fit: Fit,
}

impl Phasing {
    pub(super) fn new() -> Self {
        Self {
            cursor: 0,
            fit: Fit::default(),
        }
    }

    pub(super) fn reset(&mut self, cursor: u64) {
        self.cursor = cursor;
        self.fit = Fit::default();
    }

    pub(super) fn idle_past(&self, index: u64) -> bool {
        self.fit.count == 0 && self.cursor >= index
    }

    pub(super) fn scan(&mut self, track: &Track, period: f64) -> Scan {
        let half = (period / 2.0).round() as u64;
        let span = period.round() as u64;
        self.cursor = self.cursor.max(half);
        let from = self.cursor.saturating_sub(half);
        if track.aged_out(from) {
            self.reset(track.oldest() + half);
            return Scan::Searched;
        }
        if !track.buffered(from, self.cursor + span + half) {
            return Scan::Waiting;
        }
        let pulse = find_pulse(track, self.cursor, span, half, period);
        let phasing_like = pulse.level >= PULSE_LEVEL && pulse.rest <= REST_LEVEL;
        if phasing_like && self.continues(pulse.center as f64, period) {
            self.fit.add(pulse.center as f64);
            self.cursor = (pulse.center as f64 + period / 2.0).round() as u64;
            return Scan::Searched;
        }
        if self.fit.count >= MIN_LINES {
            let lock = self.fit.lock(period);
            self.reset(self.cursor + span);
            return Scan::Locked(lock);
        }
        self.reset(self.cursor + span);
        Scan::Searched
    }

    fn continues(&mut self, center: f64, period: f64) -> bool {
        let expected = self.fit.last + period;
        if self.fit.count == 0 || (center - expected).abs() <= period * SPACING_TOLERANCE {
            return true;
        }
        if self.fit.count >= MIN_LINES {
            return false;
        }
        self.fit = Fit::default();
        true
    }
}

fn find_pulse(track: &Track, cursor: u64, span: u64, half: u64, period: f64) -> Pulse {
    let width = (period * PULSE_FRACTION).round().max(1.0) as u64;
    let lead = width / 2;
    let mut box_sum = track.sum(cursor - lead, cursor - lead + width);
    let mut best = (cursor, box_sum);
    for center in cursor + 1..cursor + span {
        box_sum += track.at(center - lead + width - 1) - track.at(center - lead - 1);
        if box_sum > best.1 {
            best = (center, box_sum);
        }
    }
    let (center, pulse_sum) = best;
    let guard = lead;
    let outer = track.sum(center - half, center + half);
    let inner = track.sum(center - lead - guard, center - lead + width + guard);
    let rest_len = (2 * half).saturating_sub(width + 2 * guard).max(1);
    Pulse {
        center,
        level: pulse_sum / width as f64,
        rest: (outer - inner) / rest_len as f64,
    }
}
