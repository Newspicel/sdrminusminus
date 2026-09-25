use super::view::above;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DbWindow {
    pub min: f64,
    pub max: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TraceMode {
    Peak,
    Average,
    Min,
}

pub const TRACE_MODES: [TraceMode; 3] = [TraceMode::Peak, TraceMode::Average, TraceMode::Min];

impl TraceMode {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Peak => "peak hold",
            Self::Average => "average",
            Self::Min => "min hold",
        }
    }
}

const AVERAGE_FRAMES: u32 = 32;

#[derive(Clone, Debug, PartialEq)]
pub struct TraceState {
    pub peak: Vec<f32>,
    pub average: Vec<f32>,
    pub min: Vec<f32>,
    pub frames: u32,
}

pub fn dequantize(bins: &[u8], window: DbWindow, out: &mut Vec<f32>) {
    let step = (window.max - window.min) / 255.0;
    out.clear();
    out.extend(
        bins.iter()
            .map(|bin| (window.min + f64::from(*bin) * step) as f32),
    );
}

pub fn requantize(bins: &[u8], from: DbWindow, to: DbWindow, out: &mut Vec<u8>) {
    out.clear();
    let span = to.max - to.min;
    if !above(span, 0.0) {
        out.resize(bins.len(), 0);
        return;
    }
    let step = (from.max - from.min) / 255.0;
    let scale = 255.0 / span;
    out.extend(bins.iter().map(|bin| {
        let db = from.min + f64::from(*bin) * step;
        ((db - to.min) * scale).round().clamp(0.0, 255.0) as u8
    }));
}

impl TraceState {
    #[must_use]
    pub fn new(bins: usize) -> Self {
        Self {
            peak: vec![f32::NEG_INFINITY; bins],
            average: vec![0.0; bins],
            min: vec![f32::INFINITY; bins],
            frames: 0,
        }
    }

    pub fn accumulate(held: &mut Option<Self>, db: &[f32]) {
        let state = match held {
            Some(state) if state.peak.len() == db.len() => state,
            _ => held.insert(Self::new(db.len())),
        };
        let first = state.frames == 0;
        let alpha = 1.0 / (state.frames + 1).min(AVERAGE_FRAMES) as f32;
        for (at, level) in db.iter().enumerate() {
            if *level > state.peak[at] {
                state.peak[at] = *level;
            }
            if *level < state.min[at] {
                state.min[at] = *level;
            }
            state.average[at] = if first {
                *level
            } else {
                state.average[at] + alpha * (*level - state.average[at])
            };
        }
        state.frames += 1;
    }

    #[must_use]
    pub fn trace(&self, mode: TraceMode) -> &[f32] {
        match mode {
            TraceMode::Peak => &self.peak,
            TraceMode::Min => &self.min,
            TraceMode::Average => &self.average,
        }
    }
}

#[must_use]
pub fn trace_unit(db: f64, window: DbWindow) -> f64 {
    let span = window.max - window.min;
    if !above(span, 0.0) || !db.is_finite() {
        return 0.0;
    }
    ((db - window.min) / span).clamp(0.0, 1.0)
}

pub const DB_LIMIT: DbWindow = DbWindow {
    min: -180.0,
    max: 20.0,
};
pub const DB_MIN_SPAN: f64 = 5.0;

fn clamp_db(db: f64, fallback: f64) -> f64 {
    if db.is_finite() {
        db.round().clamp(DB_LIMIT.min, DB_LIMIT.max)
    } else {
        fallback
    }
}

#[must_use]
pub fn with_floor(window: DbWindow, db: f64) -> DbWindow {
    let min = clamp_db(db, DB_LIMIT.min).min(DB_LIMIT.max - DB_MIN_SPAN);
    DbWindow {
        min,
        max: clamp_db(window.max, DB_LIMIT.max).max(min + DB_MIN_SPAN),
    }
}

#[must_use]
pub fn with_ceiling(window: DbWindow, db: f64) -> DbWindow {
    let max = clamp_db(db, DB_LIMIT.max).max(DB_LIMIT.min + DB_MIN_SPAN);
    DbWindow {
        min: clamp_db(window.min, DB_LIMIT.min).min(max - DB_MIN_SPAN),
        max,
    }
}

#[must_use]
pub fn clamp_window(window: DbWindow) -> DbWindow {
    with_ceiling(with_floor(window, window.min), window.max)
}

pub const AVERAGE_CHOICES: [u32; 5] = [1, 2, 4, 8, 16];
pub const DEFAULT_AVERAGE: u32 = 4;

#[derive(Default)]
pub struct VideoAverage {
    power: Vec<f32>,
    out: Vec<f32>,
    primed: bool,
}

impl VideoAverage {
    pub fn reset(&mut self) {
        self.primed = false;
    }

    pub fn apply<'a>(&'a mut self, db: &'a [f32], frames: u32) -> &'a [f32] {
        if frames <= 1 {
            self.primed = false;
            return db;
        }
        if db.len() != self.power.len() {
            self.power = vec![0.0; db.len()];
            self.out = vec![0.0; db.len()];
            self.primed = false;
        }
        let weight = if self.primed {
            1.0 / frames as f32
        } else {
            1.0
        };
        for (at, level) in db.iter().enumerate() {
            let power = 10f32.powf(*level / 10.0);
            let held = self.power[at] + (power - self.power[at]) * weight;
            self.power[at] = held;
            self.out[at] = 10.0 * (held + 1e-30).log10();
        }
        self.primed = true;
        &self.out
    }
}

pub fn quantize_db(db: &[f32], window: DbWindow, out: &mut Vec<u8>) {
    out.clear();
    let span = window.max - window.min;
    if !above(span, 0.0) {
        out.resize(db.len(), 0);
        return;
    }
    let scale = 255.0 / span;
    out.extend(db.iter().map(|level| {
        ((f64::from(*level) - window.min) * scale)
            .round()
            .clamp(0.0, 255.0) as u8
    }));
}

const FIRST_INTERVAL_MS: f64 = 1000.0 / 30.0;
const MIN_INTERVAL_MS: f64 = 5.0;
const MAX_INTERVAL_MS: f64 = 200.0;
const INTERVAL_GLIDE: f64 = 0.2;

pub struct FrameTween {
    from: Vec<f32>,
    to: Vec<f32>,
    shown: Vec<f32>,
    arrived: f64,
    interval: f64,
}

impl Default for FrameTween {
    fn default() -> Self {
        Self {
            from: Vec::new(),
            to: Vec::new(),
            shown: Vec::new(),
            arrived: 0.0,
            interval: FIRST_INTERVAL_MS,
        }
    }
}

impl FrameTween {
    pub fn push(&mut self, db: &[f32], now: f64) {
        if db.len() != self.to.len() {
            self.jump(db, now);
            return;
        }
        self.sample(now);
        self.from.copy_from_slice(&self.shown);
        self.to.copy_from_slice(db);
        let gap = (now - self.arrived).clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS);
        self.interval += (gap - self.interval) * INTERVAL_GLIDE;
        self.arrived = now;
    }

    pub fn jump(&mut self, db: &[f32], now: f64) {
        self.from = db.to_vec();
        self.to = db.to_vec();
        self.shown = db.to_vec();
        self.arrived = now;
    }

    pub fn sample(&mut self, now: f64) -> &[f32] {
        let t = ((now - self.arrived) / self.interval).clamp(0.0, 1.0) as f32;
        for ((shown, from), to) in self.shown.iter_mut().zip(&self.from).zip(&self.to) {
            *shown = from + (to - from) * t;
        }
        &self.shown
    }

    #[must_use]
    pub fn settled(&self, now: f64) -> bool {
        now - self.arrived >= self.interval
    }
}

const REFRESH_MS: f64 = 250.0;
const SETTLE_MS: f64 = 400.0;

pub struct ReadoutHold {
    bin: Option<usize>,
    power: f64,
    settled: f64,
    refreshed: f64,
    shown: f64,
}

impl Default for ReadoutHold {
    fn default() -> Self {
        Self {
            bin: None,
            power: 0.0,
            settled: 0.0,
            refreshed: 0.0,
            shown: f64::NEG_INFINITY,
        }
    }
}

impl ReadoutHold {
    pub fn read(&mut self, bin: usize, db: f64, now: f64) -> f64 {
        if !db.is_finite() {
            self.bin = None;
            return db;
        }
        let level = 10f64.powf(db / 10.0);
        if self.bin != Some(bin) {
            self.bin = Some(bin);
            self.power = level;
            self.settled = now;
            self.refreshed = now;
            self.shown = db;
            return db;
        }
        let weight = 1.0 - (-(now - self.settled).max(0.0) / SETTLE_MS).exp();
        self.power += (level - self.power) * weight;
        self.settled = now;
        if now - self.refreshed >= REFRESH_MS {
            self.refreshed = now;
            self.shown = 10.0 * (self.power + 1e-30).log10();
        }
        self.shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: DbWindow = DbWindow {
        min: -100.0,
        max: -20.0,
    };

    fn window(min: f64, max: f64) -> DbWindow {
        DbWindow { min, max }
    }

    #[test]
    fn dequantize_maps_the_byte_range_onto_the_frame_window() {
        let mut db = Vec::new();
        dequantize(&[0, 128, 255], WINDOW, &mut db);
        assert_eq!(db[0], -100.0);
        assert!((db[1] + 59.8).abs() < 0.05);
        assert_eq!(db[2], -20.0);
    }

    #[test]
    fn requantize_keeps_a_level_in_place_and_clamps_outside() {
        let mut out = Vec::new();
        requantize(&[159], WINDOW, window(-100.0, 0.0), &mut out);
        assert_eq!(out, [127]);
        requantize(&[0, 64, 200, 255], WINDOW, WINDOW, &mut out);
        assert_eq!(out, [0, 64, 200, 255]);
        requantize(&[0, 255], window(-140.0, 0.0), WINDOW, &mut out);
        assert_eq!(out, [0, 255]);
        requantize(&[0, 128, 255], WINDOW, window(-50.0, -50.0), &mut out);
        assert_eq!(out, [0, 0, 0]);
    }

    #[test]
    fn traces_track_the_loudest_quietest_and_mean_level() {
        let mut state = None;
        TraceState::accumulate(&mut state, &[-60.0, -30.0]);
        TraceState::accumulate(&mut state, &[-40.0, -50.0]);
        let state = state.expect("a state");
        assert_eq!(state.trace(TraceMode::Peak), [-40.0, -30.0]);
        assert_eq!(state.trace(TraceMode::Min), [-60.0, -50.0]);
        assert_eq!(state.trace(TraceMode::Average), [-50.0, -40.0]);
    }

    #[test]
    fn the_first_frame_is_the_average_outright() {
        let mut state = None;
        TraceState::accumulate(&mut state, &[-77.0]);
        let state = state.expect("a state");
        assert_eq!(state.average[0], -77.0);
        assert_eq!(state.frames, 1);
    }

    #[test]
    fn the_average_converges_without_overshoot() {
        let mut state = None;
        for at in 0..200 {
            TraceState::accumulate(&mut state, &[if at == 0 { -20.0 } else { -80.0 }]);
        }
        let state = state.expect("a state");
        assert!((state.average[0] + 80.0).abs() < 0.1);
        assert_eq!(state.peak[0], -20.0);
    }

    #[test]
    fn traces_restart_when_the_bin_count_changes() {
        let mut state = None;
        TraceState::accumulate(&mut state, &[-60.0, -60.0]);
        TraceState::accumulate(&mut state, &[-50.0, -50.0]);
        assert_eq!(state.as_ref().map(|state| state.frames), Some(2));
        TraceState::accumulate(&mut state, &[-50.0, -50.0, -50.0]);
        assert_eq!(state.as_ref().map(|state| state.frames), Some(1));
    }

    #[test]
    fn a_level_is_placed_on_the_unit_height_and_clamped() {
        assert!((trace_unit(-60.0, WINDOW) - 0.5).abs() < 1e-6);
        assert_eq!(trace_unit(-100.0, WINDOW), 0.0);
        assert_eq!(trace_unit(-20.0, WINDOW), 1.0);
        assert_eq!(trace_unit(-140.0, WINDOW), 0.0);
        assert_eq!(trace_unit(10.0, WINDOW), 1.0);
        assert_eq!(trace_unit(f64::NEG_INFINITY, WINDOW), 0.0);
        assert_eq!(trace_unit(-60.0, window(-20.0, -20.0)), 0.0);
    }

    #[test]
    fn a_byte_reads_back_through_its_own_window() {
        let mut db = Vec::new();
        dequantize(&[0, 255], window(-110.0, -10.0), &mut db);
        assert_eq!(db, [-110.0, -10.0]);
    }

    #[test]
    fn the_floor_moves_pushes_the_ceiling_and_stays_in_limits() {
        let held = window(-110.0, -40.0);
        assert_eq!(with_floor(held, -95.0), window(-95.0, -40.0));
        assert_eq!(with_floor(held, -20.0), window(-20.0, -20.0 + DB_MIN_SPAN));
        assert_eq!(with_floor(held, -400.0).min, DB_LIMIT.min);
        assert_eq!(with_floor(held, 900.0).min, DB_LIMIT.max - DB_MIN_SPAN);
        assert_eq!(with_floor(held, -94.6).min, -95.0);
    }

    #[test]
    fn the_ceiling_moves_pushes_the_floor_and_stays_in_limits() {
        let held = window(-110.0, -40.0);
        assert_eq!(with_ceiling(held, -60.0), window(-110.0, -60.0));
        assert_eq!(
            with_ceiling(held, -120.0),
            window(-120.0 - DB_MIN_SPAN, -120.0)
        );
        assert_eq!(with_ceiling(held, 900.0).max, DB_LIMIT.max);
        assert_eq!(with_ceiling(held, -400.0).max, DB_LIMIT.min + DB_MIN_SPAN);
    }

    #[test]
    fn a_window_is_brought_inside_the_limits() {
        assert_eq!(clamp_window(window(-110.0, -40.0)), window(-110.0, -40.0));
        let silent = clamp_window(window(-250.0, -180.0));
        assert!(silent.min >= DB_LIMIT.min);
        assert!(silent.max - silent.min >= DB_MIN_SPAN);
        assert_eq!(clamp_window(window(f64::NEG_INFINITY, f64::NAN)), DB_LIMIT);
    }

    #[test]
    fn the_video_average_passes_through_when_off() {
        let mut average = VideoAverage::default();
        assert_eq!(average.apply(&[-80.0], 1), [-80.0]);
    }

    #[test]
    fn the_video_average_moves_a_share_per_frame_and_holds_steady() {
        let mut average = VideoAverage::default();
        assert!((average.apply(&[-80.0], 4)[0] + 80.0).abs() < 1e-3);
        let next = f64::from(average.apply(&[-70.0], 4)[0]);
        let expected = 10.0 * (1e-8 + (1e-7 - 1e-8) / 4.0f64).log10();
        assert!((next - expected).abs() < 1e-3);
        let mut steady = VideoAverage::default();
        for _ in 0..20 {
            steady.apply(&[-50.0, -90.0], 8);
        }
        let out = steady.apply(&[-50.0, -90.0], 8);
        assert!((out[0] + 50.0).abs() < 1e-2);
        assert!((out[1] + 90.0).abs() < 1e-2);
    }

    #[test]
    fn the_video_average_forgets_on_reset() {
        let mut average = VideoAverage::default();
        average.apply(&[-80.0], 8);
        average.reset();
        assert!((average.apply(&[-20.0], 8)[0] + 20.0).abs() < 1e-3);
    }

    #[test]
    fn quantize_spreads_the_window_over_the_bytes() {
        let mut out = Vec::new();
        quantize_db(
            &[-130.0, -120.0, 8.0, 135.0, 140.0],
            window(-120.0, 135.0),
            &mut out,
        );
        assert_eq!(out, [0, 0, 128, 255, 255]);
    }

    #[test]
    fn the_tween_shows_the_first_frame_as_it_came() {
        let mut tween = FrameTween::default();
        tween.push(&[-80.0, -40.0], 0.0);
        assert_eq!(tween.sample(0.0), [-80.0, -40.0]);
    }

    #[test]
    fn the_tween_blends_over_one_frame_interval() {
        let mut tween = FrameTween::default();
        tween.push(&[-80.0], 0.0);
        let start = 1000.0 / 30.0;
        tween.push(&[-60.0], start);
        assert!((tween.sample(start)[0] + 80.0).abs() < 1e-3);
        assert!((tween.sample(start + 1000.0 / 60.0)[0] + 70.0).abs() < 0.5);
        assert!((tween.sample(start + 1000.0)[0] + 60.0).abs() < 1e-3);
    }

    #[test]
    fn the_tween_starts_the_next_blend_where_the_trace_is() {
        let mut tween = FrameTween::default();
        tween.push(&[-80.0], 0.0);
        tween.push(&[-60.0], 1000.0 / 30.0);
        let mid = 1000.0 / 30.0 + 1000.0 / 60.0;
        let shown = tween.sample(mid)[0];
        tween.push(&[-20.0], mid);
        assert!((tween.sample(mid)[0] - shown).abs() < 1e-3);
    }

    #[test]
    fn the_tween_jumps_after_a_retune_and_restarts_on_a_new_width() {
        let mut tween = FrameTween::default();
        tween.push(&[-80.0], 0.0);
        tween.jump(&[-10.0], 5.0);
        assert_eq!(tween.sample(5.0), [-10.0]);
        tween.push(&[-50.0, -40.0], 10.0);
        assert_eq!(tween.sample(10.0), [-50.0, -40.0]);
    }

    #[test]
    fn the_readout_shows_a_new_bin_at_once_and_holds_between_refreshes() {
        let mut hold = ReadoutHold::default();
        assert_eq!(hold.read(3, -70.0, 0.0), -70.0);
        assert_eq!(hold.read(4, -40.0, 16.0), -40.0);
        let mut held = ReadoutHold::default();
        held.read(3, -70.0, 0.0);
        assert_eq!(held.read(3, -50.0, 16.0), -70.0);
        assert_eq!(held.read(3, -90.0, 200.0), -70.0);
    }

    #[test]
    fn the_readout_settles_on_the_mean_power() {
        let mut hold = ReadoutHold::default();
        let mut shown = hold.read(3, -80.0, 0.0);
        let mut now = 16.0;
        while now < 5000.0 {
            let level = if (now as u32).is_multiple_of(32) {
                -77.0
            } else {
                -83.0
            };
            shown = hold.read(3, level, now);
            now += 16.0;
        }
        assert!(shown > -80.0 && shown < -78.5);
        assert_eq!(
            ReadoutHold::default().read(3, f64::NEG_INFINITY, 0.0),
            f64::NEG_INFINITY
        );
    }
}
