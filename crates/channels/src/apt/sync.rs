use super::{
    geometry::{SYNC_A, SYNC_WORDS, WORD_RATE_HZ},
    track::Track,
};

const ACQUIRE_MATCH: f32 = 0.5;
const ACQUIRE_SYNCS: u8 = 3;
const ACQUIRE_TOLERANCE_WORDS: f64 = 4.0;
const TRACK_MATCH: f32 = 0.3;
const SEARCH_WORDS: f64 = 8.0;
const PHASE_GAIN: f64 = 0.5;
const FREQ_GAIN: f64 = 0.05;
const MAX_DRIFT: f64 = 0.005;
const FLAT: f32 = 1e-9;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Peak {
    pub(crate) position: f64,
    pub(crate) score: f32,
}

pub(crate) struct SyncDetector {
    template: Vec<f32>,
    words: f64,
}

impl SyncDetector {
    pub(crate) fn new(rate: f64) -> Self {
        let words = rate / WORD_RATE_HZ;
        let len = (SYNC_WORDS as f64 * words).round() as usize;
        let mut template: Vec<f32> = (0..len)
            .map(|k| {
                let word = (((k as f64 + 0.5) / words) as usize).min(SYNC_WORDS - 1);
                if SYNC_A[word] { 1.0 } else { 0.0 }
            })
            .collect();
        let mean = template.iter().sum::<f32>() / len as f32;
        for value in &mut template {
            *value -= mean;
        }
        let norm = template.iter().map(|v| v * v).sum::<f32>().sqrt();
        for value in &mut template {
            *value /= norm;
        }
        Self { template, words }
    }

    pub(crate) fn len(&self) -> u64 {
        self.template.len() as u64
    }

    pub(crate) fn search(&self) -> f64 {
        SEARCH_WORDS * self.words
    }

    pub(crate) fn tolerance(&self) -> f64 {
        ACQUIRE_TOLERANCE_WORDS * self.words
    }

    pub(crate) fn score(&self, track: &Track, start: u64) -> f32 {
        let mut dot = 0.0f32;
        let mut sum = 0.0f32;
        let mut square = 0.0f32;
        for (k, &weight) in self.template.iter().enumerate() {
            let value = track.get(start + k as u64);
            dot += weight * value;
            sum += value;
            square += value * value;
        }
        let spread = square - sum * sum / self.template.len() as f32;
        if spread <= FLAT {
            return 0.0;
        }
        dot / spread.sqrt()
    }

    pub(crate) fn best(&self, track: &Track, from: u64, to: u64) -> Peak {
        let mut best_at = from;
        let mut best = f32::MIN;
        for start in from..to.max(from + 1) {
            let score = self.score(track, start);
            if score > best {
                best = score;
                best_at = start;
            }
        }
        let before = self.score(track, best_at.saturating_sub(1));
        let after = self.score(track, best_at + 1);
        let curve = before - 2.0 * best + after;
        let shift = if curve < 0.0 {
            (0.5 * (before - after) / curve).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        Peak {
            position: best_at as f64 + f64::from(shift),
            score: best,
        }
    }
}

#[derive(Default)]
pub(crate) struct Acquisition {
    pub(crate) from: u64,
    streak: u8,
    first: f64,
    last: f64,
}

impl Acquisition {
    pub(crate) fn restart(&mut self, from: u64) {
        self.from = from;
        self.streak = 0;
    }

    pub(crate) fn observe(&mut self, peak: Peak, line: f64, tolerance: f64) -> Option<(f64, f64)> {
        if peak.score < ACQUIRE_MATCH {
            self.streak = 0;
            return None;
        }
        let spacing = peak.position - self.last;
        if self.streak > 0 && (spacing - line).abs() <= tolerance {
            self.streak += 1;
        } else {
            self.streak = 1;
            self.first = peak.position;
        }
        self.last = peak.position;
        if self.streak < ACQUIRE_SYNCS {
            return None;
        }
        let measured = (self.last - self.first) / f64::from(self.streak - 1);
        self.streak = 0;
        Some((self.first, clamp_line(measured, line)))
    }
}

fn clamp_line(len: f64, nominal: f64) -> f64 {
    len.clamp(nominal * (1.0 - MAX_DRIFT), nominal * (1.0 + MAX_DRIFT))
}

pub(crate) struct LineClock {
    pub(crate) start: f64,
    pub(crate) len: f64,
    pub(crate) lost: u16,
    nominal: f64,
}

impl LineClock {
    pub(crate) fn new(nominal: f64) -> Self {
        Self {
            start: 0.0,
            len: nominal,
            lost: 0,
            nominal,
        }
    }

    pub(crate) fn lock(&mut self, start: f64, len: f64) {
        self.start = start;
        self.len = clamp_line(len, self.nominal);
        self.lost = 0;
    }

    pub(crate) fn observe(&mut self, peak: Peak) -> bool {
        if peak.score < TRACK_MATCH {
            self.lost = self.lost.saturating_add(1);
            return false;
        }
        let error = peak.position - self.start;
        self.start += PHASE_GAIN * error;
        self.len = clamp_line(self.len + FREQ_GAIN * error, self.nominal);
        self.lost = 0;
        true
    }

    pub(crate) fn next_line(&mut self) {
        self.start += self.len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 12_000.0;

    fn track_with_sync_at(offset: usize) -> Track {
        let words = RATE / WORD_RATE_HZ;
        let mut track = Track::new();
        for k in 0..2_000 {
            let word = ((k as f64 - offset as f64) / words).floor();
            let high = word >= 0.0 && (word as usize) < SYNC_WORDS && SYNC_A[word as usize];
            track.push(if high { 1.0 } else { 0.2 });
        }
        track
    }

    #[test]
    fn the_detector_finds_sync_a_where_it_was_placed() {
        let detector = SyncDetector::new(RATE);
        let track = track_with_sync_at(700);
        let peak = detector.best(&track, 0, 1_500);
        assert!(
            (peak.position - 700.0).abs() < 0.6,
            "found at {}",
            peak.position
        );
        assert!(peak.score > 0.85, "score {}", peak.score);
    }

    #[test]
    fn three_evenly_spaced_syncs_lock() {
        let mut acquisition = Acquisition::default();
        let peak = |position| Peak {
            position,
            score: 0.9,
        };
        assert!(acquisition.observe(peak(100.0), 6_000.0, 10.0).is_none());
        assert!(acquisition.observe(peak(6_102.0), 6_000.0, 10.0).is_none());
        let (first, len) = acquisition
            .observe(peak(12_104.0), 6_000.0, 10.0)
            .expect("locks");
        assert_eq!(first, 100.0);
        assert_eq!(len, 6_002.0);
    }

    #[test]
    fn an_uneven_sync_restarts_the_streak() {
        let mut acquisition = Acquisition::default();
        let peak = |position| Peak {
            position,
            score: 0.9,
        };
        acquisition.observe(peak(100.0), 6_000.0, 10.0);
        acquisition.observe(peak(6_100.0), 6_000.0, 10.0);
        assert!(acquisition.observe(peak(12_400.0), 6_000.0, 10.0).is_none());
    }
}
