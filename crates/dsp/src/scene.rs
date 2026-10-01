pub mod echo;

use std::f64::consts::{PI, TAU};

use num_complex::Complex;

pub use echo::SceneEcho;

use crate::manifold::{
    Direction, Geometry, MAX_ELEMENTS, ManifoldError, ManifoldTable, steer, widen,
};
use crate::special::{bessel_i0, sinc};

const DELAY_TAPS: usize = 129;
const DELAY_CENTRE: i64 = 64;
const DELAY_BETA: f64 = 8.0;
const NOISE_TAPS: usize = 63;
const WAVE_PRE_ROLL: i64 = 1024;
const LANE_PRE_ROLL: i64 = 256;
const KEEP_SAMPLES: i64 = 4096;
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;
const XORSHIFT_STAR: u64 = 0x2545_F491_4F6C_DD1D;
const NOISE_SALT: u64 = 0xD1B5_4A32_D192_ED03;
const MAX_TABLE_AZIMUTHS: f64 = 3600.0;
const ECHO_FRACTIONS: usize = 256;

type C64 = Complex<f64>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SceneSignal {
    Tone {
        offset_hz: f64,
    },
    Noise {
        offset_hz: f64,
        bandwidth_hz: f64,
    },
    Fm {
        offset_hz: f64,
        deviation_hz: f64,
        rate_hz: f64,
    },
    NoiseFm {
        offset_hz: f64,
        deviation_hz: f64,
        bandwidth_hz: f64,
    },
    Broadband,
}

impl SceneSignal {
    #[must_use]
    pub const fn offset_hz(self) -> f64 {
        match self {
            Self::Tone { offset_hz }
            | Self::Noise { offset_hz, .. }
            | Self::Fm { offset_hz, .. }
            | Self::NoiseFm { offset_hz, .. } => offset_hz,
            Self::Broadband => 0.0,
        }
    }

    const fn is_random(self) -> bool {
        matches!(
            self,
            Self::Noise { .. } | Self::NoiseFm { .. } | Self::Broadband
        )
    }

    fn is_valid(self) -> bool {
        match self {
            Self::Tone { offset_hz } => offset_hz.is_finite(),
            Self::Noise {
                offset_hz,
                bandwidth_hz,
            } => offset_hz.is_finite() && bandwidth_hz.is_finite() && bandwidth_hz > 0.0,
            Self::Fm {
                offset_hz,
                deviation_hz,
                rate_hz,
            } => offset_hz.is_finite() && deviation_hz.is_finite() && rate_hz.is_finite(),
            Self::NoiseFm {
                offset_hz,
                deviation_hz,
                bandwidth_hz,
            } => {
                offset_hz.is_finite()
                    && deviation_hz.is_finite()
                    && bandwidth_hz.is_finite()
                    && bandwidth_hz > 0.0
            }
            Self::Broadband => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneCopy {
    pub source: usize,
    pub amplitude: f32,
    pub phase_deg: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneSource {
    pub direction: Direction,
    pub power_db: f32,
    pub signal: SceneSignal,
    pub copy_of: Option<SceneCopy>,
    pub delay_samples: f32,
}

impl SceneSource {
    #[must_use]
    pub const fn new(direction: Direction, power_db: f32, signal: SceneSignal) -> Self {
        Self {
            direction,
            power_db,
            signal,
            copy_of: None,
            delay_samples: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HeadingTrack {
    Fixed(f64),
    Rotating { start_deg: f64, rate_dps: f64 },
}

impl HeadingTrack {
    #[must_use]
    pub fn at(self, t_s: f64) -> f64 {
        match self {
            Self::Fixed(heading) => heading,
            Self::Rotating {
                start_deg,
                rate_dps,
            } => start_deg + rate_dps * t_s,
        }
    }

    const fn is_finite(self) -> bool {
        match self {
            Self::Fixed(heading) => heading.is_finite(),
            Self::Rotating {
                start_deg,
                rate_dps,
            } => start_deg.is_finite() && rate_dps.is_finite(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
pub enum SceneError {
    #[error(transparent)]
    Manifold(#[from] ManifoldError),
    #[error("sample rate must be positive and finite")]
    SampleRate,
    #[error("{0} needs one value per lane or none")]
    LaneSetting(&'static str),
    #[error("source {0} copies a missing source or another copy")]
    Copy(usize),
    #[error("source {0} has a non-finite or empty setting")]
    Source(usize),
    #[error("heading must be finite")]
    Heading,
    #[error("echo {0} copies a missing source or another copy, or has a non-finite setting")]
    Echo(usize),
}

pub type Distortion = fn(usize, f64) -> Complex<f32>;

#[derive(Clone, Debug)]
pub struct ArrayScene {
    pub geometry: Geometry,
    pub center_hz: f64,
    pub sample_rate: f64,
    pub noise_db: Vec<f32>,
    pub sources: Vec<SceneSource>,
    pub lane_phase_deg: Vec<f32>,
    pub lane_gain_db: Vec<f32>,
    pub lane_delay_samples: Vec<f32>,
    pub lane_dc: Vec<Complex<f32>>,
    pub echoes: Vec<SceneEcho>,
    pub distortion: Option<Distortion>,
    pub heading: HeadingTrack,
    pub seed: u64,
    state: Option<SceneState>,
}

impl ArrayScene {
    #[must_use]
    pub const fn new(geometry: Geometry, center_hz: f64, sample_rate: f64) -> Self {
        Self {
            geometry,
            center_hz,
            sample_rate,
            noise_db: Vec::new(),
            sources: Vec::new(),
            lane_phase_deg: Vec::new(),
            lane_gain_db: Vec::new(),
            lane_delay_samples: Vec::new(),
            lane_dc: Vec::new(),
            echoes: Vec::new(),
            distortion: None,
            heading: HeadingTrack::Fixed(0.0),
            seed: 1,
            state: None,
        }
    }

    #[must_use]
    pub fn with_source(mut self, source: SceneSource) -> Self {
        self.sources.push(source);
        self
    }

    #[must_use]
    pub fn with_echo(mut self, echo: SceneEcho) -> Self {
        self.echoes.push(echo);
        self
    }

    #[must_use]
    pub fn with_noise_db(mut self, db: f32) -> Self {
        self.noise_db = vec![db; self.geometry.len()];
        self
    }

    #[must_use]
    pub const fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    #[must_use]
    pub const fn with_heading(mut self, heading: HeadingTrack) -> Self {
        self.heading = heading;
        self
    }

    #[must_use]
    pub const fn lanes(&self) -> usize {
        self.geometry.len()
    }

    #[must_use]
    pub fn rendered(&self) -> i64 {
        self.state.as_ref().map_or(0, |state| state.cursor)
    }

    pub fn element_response(
        &self,
        freq_hz: f64,
        true_direction: Direction,
        t_s: f64,
        out: &mut [Complex<f32>],
    ) {
        let body = Direction::new(
            true_direction.azimuth_deg - self.heading.at(t_s),
            true_direction.elevation_deg,
        );
        steer(self.geometry.positions(), freq_hz, body, out);
        if let Some(distortion) = self.distortion {
            for (element, value) in out.iter_mut().enumerate().take(self.lanes()) {
                *value *= distortion(element, body.azimuth_deg);
            }
        }
    }

    pub fn render(&mut self, len: usize) -> Result<Vec<Vec<Complex<f32>>>, SceneError> {
        self.validate()?;
        let mut state = match self.state.take() {
            Some(state) if state.lanes.len() == self.lanes() => state,
            _ => SceneState::new(self.lanes(), self.seed),
        };
        state.sync_waves(&self.sources, self.seed, self.sample_rate);
        if !self.echoes.is_empty() && state.fractions.is_empty() {
            state.fractions = fraction_table();
        }
        let result = self.render_with(&mut state, len);
        self.state = Some(state);
        Ok(result)
    }

    pub fn distortion_table(
        &self,
        freqs_hz: &[f64],
        step_deg: f64,
    ) -> Result<ManifoldTable, SceneError> {
        let n = self.lanes();
        let azimuths = (360.0 / step_deg).round();
        if !(1.0..=MAX_TABLE_AZIMUTHS).contains(&azimuths) {
            return Err(ManifoldError::Table.into());
        }
        let azimuths = azimuths as usize;
        let mut data = vec![Complex::new(0.0f32, 0.0); freqs_hz.len() * azimuths * n];
        let fixed = Self {
            heading: HeadingTrack::Fixed(0.0),
            state: None,
            ..self.clone()
        };
        for (f, &freq) in freqs_hz.iter().enumerate() {
            for a in 0..azimuths {
                let at = (f * azimuths + a) * n;
                let direction = Direction::horizon(a as f64 * step_deg);
                fixed.element_response(freq, direction, 0.0, &mut data[at..at + n]);
            }
        }
        Ok(ManifoldTable::new(
            n,
            freqs_hz.to_vec(),
            step_deg,
            vec![0.0],
            data,
        )?)
    }

    fn validate(&self) -> Result<(), SceneError> {
        if !(self.sample_rate.is_finite() && self.sample_rate > 0.0) {
            return Err(SceneError::SampleRate);
        }
        if !(self.center_hz.is_finite() && self.center_hz > 0.0) {
            return Err(ManifoldError::Frequency.into());
        }
        let lanes = self.lanes();
        let fits = |len: usize| len == 0 || len == lanes;
        for (name, len) in [
            ("noise_db", self.noise_db.len()),
            ("lane_phase_deg", self.lane_phase_deg.len()),
            ("lane_gain_db", self.lane_gain_db.len()),
            ("lane_delay_samples", self.lane_delay_samples.len()),
            ("lane_dc", self.lane_dc.len()),
        ] {
            if !fits(len) {
                return Err(SceneError::LaneSetting(name));
            }
        }
        for (name, values) in [
            ("noise_db", &self.noise_db),
            ("lane_phase_deg", &self.lane_phase_deg),
            ("lane_gain_db", &self.lane_gain_db),
            ("lane_delay_samples", &self.lane_delay_samples),
        ] {
            if !values.iter().all(|value| value.is_finite()) {
                return Err(SceneError::LaneSetting(name));
            }
        }
        if !self.lane_dc.iter().all(|dc| dc.is_finite()) {
            return Err(SceneError::LaneSetting("lane_dc"));
        }
        if !self.heading.is_finite() {
            return Err(SceneError::Heading);
        }
        for (index, source) in self.sources.iter().enumerate() {
            let sound = source.delay_samples.is_finite()
                && source.power_db.is_finite()
                && source.signal.is_valid()
                && source.direction.azimuth_deg.is_finite()
                && source.direction.elevation_deg.is_finite();
            if !sound {
                return Err(SceneError::Source(index));
            }
            if let Some(copy) = source.copy_of {
                let original = self.sources.get(copy.source);
                if copy.source == index || original.is_none_or(|o| o.copy_of.is_some()) {
                    return Err(SceneError::Copy(index));
                }
            }
        }
        for (index, echo) in self.echoes.iter().enumerate() {
            let original = self.sources.get(echo.source);
            if !echo.is_valid() || original.is_none_or(|o| o.copy_of.is_some()) {
                return Err(SceneError::Echo(index));
            }
        }
        Ok(())
    }

    fn render_with(&self, state: &mut SceneState, len: usize) -> Vec<Vec<Complex<f32>>> {
        let lanes = self.lanes();
        let start = state.cursor;
        let end = start + len as i64;
        let source_plans: Vec<DelayPlan> = self
            .sources
            .iter()
            .map(|source| DelayPlan::new(f64::from(source.delay_samples)))
            .collect();
        let lane_plans: Vec<DelayPlan> = (0..lanes)
            .map(|lane| DelayPlan::new(lane_value(&self.lane_delay_samples, lane).into()))
            .collect();
        let needed = lane_plans
            .iter()
            .map(|plan| plan.reach(end - 1))
            .max()
            .unwrap_or(end - 1);
        self.fill_lanes(state, &source_plans, needed + 1);
        let mut out = vec![vec![Complex::new(0.0f32, 0.0); len]; lanes];
        for (lane, samples) in out.iter_mut().enumerate() {
            let gain = 10f64.powf(f64::from(lane_value(&self.lane_gain_db, lane)) / 20.0);
            let turn = C64::from_polar(
                gain,
                f64::from(lane_value(&self.lane_phase_deg, lane)).to_radians(),
            );
            let dc = widen(self.lane_dc.get(lane).copied().unwrap_or_default());
            let sigma = self
                .noise_db
                .get(lane)
                .map_or(0.0, |&db| 10f64.powf(f64::from(db) / 20.0));
            for (index, value) in samples.iter_mut().enumerate() {
                let n = start + index as i64;
                let signal = lane_plans[lane].read(&state.lanes[lane], n) * turn;
                let noise = state.noise[lane].complex_normal() * sigma;
                let total = signal + noise + dc;
                *value = Complex::new(total.re as f32, total.im as f32);
            }
        }
        state.cursor = end;
        let longest = source_plans
            .iter()
            .chain(&lane_plans)
            .map(|plan| plan.whole.abs())
            .chain(self.echoes.iter().map(|echo| {
                let delay = self.echo_delay(echo, end).abs().ceil() as i64;
                delay + DELAY_CENTRE
            }))
            .max()
            .unwrap_or(0);
        state.trim(end - KEEP_SAMPLES - longest);
        out
    }

    fn echo_carrier(&self, echo: &SceneEcho) -> f64 {
        self.center_hz
            + self
                .sources
                .get(echo.source)
                .map_or(0.0, |source| source.signal.offset_hz())
    }

    fn echo_delay(&self, echo: &SceneEcho, n: i64) -> f64 {
        let direct = self
            .sources
            .get(echo.source)
            .map_or(0.0, |source| f64::from(source.delay_samples));
        direct + echo.delay_at(n, self.echo_carrier(echo))
    }

    fn echo_value(&self, state: &SceneState, echo: &SceneEcho, n: i64) -> C64 {
        let source = &self.sources[echo.source];
        let delay = self.echo_delay(echo, n);
        state.waves[echo.source].at(n, delay, self.sample_rate, &state.fractions)
            * amplitude(source)
            * echo.rotation(n, self.sample_rate)
    }

    fn ensure_echo_history(&self, state: &mut SceneState, from: i64, upto: i64) {
        for echo in &self.echoes {
            let reach = [from, upto - 1]
                .iter()
                .map(|&n| n - self.echo_delay(echo, n).floor() as i64 + DELAY_CENTRE + 1)
                .max()
                .unwrap_or(upto);
            state.waves[echo.source].ensure(reach, self.sample_rate);
        }
    }

    fn fill_lanes(&self, state: &mut SceneState, plans: &[DelayPlan], upto: i64) {
        let from = state.lanes[0].end();
        if from >= upto {
            return;
        }
        for (index, source) in self.sources.iter().enumerate() {
            let wave = source.copy_of.map_or(index, |copy| copy.source);
            state.waves[wave].ensure(plans[index].reach(upto - 1), self.sample_rate);
        }
        self.ensure_echo_history(state, from, upto);
        let lanes = self.lanes();
        let rotating = matches!(self.heading, HeadingTrack::Rotating { .. });
        let mut responses = vec![[Complex::new(0.0f32, 0.0); MAX_ELEMENTS]; self.sources.len()];
        let mut echo_responses = vec![[Complex::new(0.0f32, 0.0); MAX_ELEMENTS]; self.echoes.len()];
        let fs = self.sample_rate;
        for n in from..upto {
            let t_s = n as f64 / fs;
            if rotating || n == from {
                for (response, source) in responses.iter_mut().zip(&self.sources) {
                    let freq = self.center_hz + self.source_signal(source).offset_hz();
                    self.element_response(freq, source.direction, t_s, &mut response[..lanes]);
                }
                for (response, echo) in echo_responses.iter_mut().zip(&self.echoes) {
                    let freq = self.echo_carrier(echo);
                    self.element_response(freq, echo.direction, t_s, &mut response[..lanes]);
                }
            }
            let mut sums = [C64::new(0.0, 0.0); MAX_ELEMENTS];
            for ((index, source), response) in self.sources.iter().enumerate().zip(&responses) {
                let value = self.source_value(state, plans, index, source, n);
                for (sum, element) in sums.iter_mut().zip(&response[..lanes]) {
                    *sum += widen(*element) * value;
                }
            }
            for (echo, response) in self.echoes.iter().zip(&echo_responses) {
                let value = self.echo_value(state, echo, n);
                for (sum, element) in sums.iter_mut().zip(&response[..lanes]) {
                    *sum += widen(*element) * value;
                }
            }
            for (history, sum) in state.lanes.iter_mut().zip(&sums) {
                history.data.push(*sum);
            }
        }
    }

    fn source_signal(&self, source: &SceneSource) -> SceneSignal {
        source
            .copy_of
            .and_then(|copy| self.sources.get(copy.source))
            .map_or(source.signal, |original| original.signal)
    }

    fn source_value(
        &self,
        state: &SceneState,
        plans: &[DelayPlan],
        index: usize,
        source: &SceneSource,
        n: i64,
    ) -> C64 {
        match source.copy_of {
            None => {
                state.waves[index].value(n, &plans[index], self.sample_rate) * amplitude(source)
            }
            Some(copy) => {
                let original = &self.sources[copy.source];
                let echo = C64::from_polar(
                    f64::from(copy.amplitude),
                    f64::from(copy.phase_deg).to_radians(),
                );
                state.waves[copy.source].value(n, &plans[index], self.sample_rate)
                    * amplitude(original)
                    * echo
            }
        }
    }
}

fn amplitude(source: &SceneSource) -> f64 {
    10f64.powf(f64::from(source.power_db) / 20.0)
}

fn lane_value(values: &[f32], lane: usize) -> f32 {
    values.get(lane).copied().unwrap_or(0.0)
}

#[derive(Clone, Debug)]
struct SceneState {
    cursor: i64,
    waves: Vec<Wave>,
    lanes: Vec<History>,
    noise: Vec<Rng>,
    fractions: Vec<Vec<f64>>,
}

impl SceneState {
    fn new(lanes: usize, seed: u64) -> Self {
        Self {
            cursor: 0,
            waves: Vec::new(),
            lanes: vec![History::starting_at(-LANE_PRE_ROLL); lanes],
            noise: (0..lanes)
                .map(|lane| Rng::seeded(seed ^ NOISE_SALT, lane as u64))
                .collect(),
            fractions: Vec::new(),
        }
    }

    fn sync_waves(&mut self, sources: &[SceneSource], seed: u64, sample_rate: f64) {
        self.waves.truncate(sources.len());
        for (index, source) in sources.iter().enumerate() {
            let stale = self
                .waves
                .get(index)
                .is_none_or(|wave| wave.signal != source.signal);
            if stale {
                let wave = Wave::new(source.signal, Rng::seeded(seed, index as u64), sample_rate);
                if index < self.waves.len() {
                    self.waves[index] = wave;
                } else {
                    self.waves.push(wave);
                }
            }
        }
    }

    fn trim(&mut self, keep_from: i64) {
        for wave in &mut self.waves {
            wave.history.trim(keep_from);
        }
        for lane in &mut self.lanes {
            lane.trim(keep_from);
        }
    }
}

#[derive(Clone, Debug)]
struct History {
    base: i64,
    data: Vec<C64>,
}

impl History {
    const fn starting_at(base: i64) -> Self {
        Self {
            base,
            data: Vec::new(),
        }
    }

    fn end(&self) -> i64 {
        self.base + self.data.len() as i64
    }

    fn get(&self, index: i64) -> C64 {
        usize::try_from(index - self.base)
            .ok()
            .and_then(|offset| self.data.get(offset))
            .copied()
            .unwrap_or_default()
    }

    fn trim(&mut self, keep_from: i64) {
        let drop = (keep_from - self.base).clamp(0, self.data.len() as i64) as usize;
        if drop > 0 {
            self.data.drain(..drop);
            self.base += drop as i64;
        }
    }
}

#[derive(Clone, Debug)]
struct DelayPlan {
    delay: f64,
    whole: i64,
    taps: Option<Vec<f64>>,
}

impl DelayPlan {
    fn new(delay: f64) -> Self {
        let whole = delay.floor();
        let fraction = delay - whole;
        let taps = (fraction > 0.0).then(|| fractional_taps(fraction));
        Self {
            delay,
            whole: whole as i64,
            taps,
        }
    }

    fn reach(&self, n: i64) -> i64 {
        let reach = n - self.whole;
        if self.taps.is_some() {
            reach + DELAY_CENTRE
        } else {
            reach
        }
    }

    fn read(&self, history: &History, n: i64) -> C64 {
        let at = n - self.whole;
        match &self.taps {
            None => history.get(at),
            Some(taps) => taps
                .iter()
                .enumerate()
                .map(|(k, &tap)| history.get(at - k as i64 + DELAY_CENTRE) * tap)
                .sum(),
        }
    }
}

fn fractional_taps(fraction: f64) -> Vec<f64> {
    let centre = DELAY_CENTRE as f64 + fraction;
    let half = DELAY_CENTRE as f64 + 1.0;
    let scale = bessel_i0(DELAY_BETA);
    let mut taps: Vec<f64> = (0..DELAY_TAPS)
        .map(|k| {
            let x = k as f64 - centre;
            let ratio = (x / half).clamp(-1.0, 1.0);
            sinc(x) * bessel_i0(DELAY_BETA * (1.0 - ratio * ratio).sqrt()) / scale
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    for tap in &mut taps {
        *tap /= sum;
    }
    taps
}

fn fraction_table() -> Vec<Vec<f64>> {
    (0..ECHO_FRACTIONS)
        .map(|step| fractional_taps(step as f64 / ECHO_FRACTIONS as f64))
        .collect()
}

#[derive(Clone, Debug)]
struct Wave {
    signal: SceneSignal,
    phase: f64,
    rng: Rng,
    lowpass: Vec<f64>,
    white: Vec<C64>,
    history: History,
}

impl Wave {
    fn new(signal: SceneSignal, mut rng: Rng, sample_rate: f64) -> Self {
        let phase = TAU * rng.uniform();
        let lowpass = match signal {
            SceneSignal::Noise { bandwidth_hz, .. } | SceneSignal::NoiseFm { bandwidth_hz, .. }
                if bandwidth_hz < sample_rate =>
            {
                lowpass_taps(bandwidth_hz / (2.0 * sample_rate))
            }
            _ => Vec::new(),
        };
        Self {
            signal,
            phase,
            rng,
            white: vec![C64::new(0.0, 0.0); lowpass.len()],
            lowpass,
            history: History::starting_at(-WAVE_PRE_ROLL),
        }
    }

    fn ensure(&mut self, upto: i64, sample_rate: f64) {
        if !self.signal.is_random() {
            return;
        }
        while self.history.end() <= upto {
            let n = self.history.end();
            let value = self.next_random(n, sample_rate);
            self.history.data.push(value);
        }
    }

    fn next_random(&mut self, n: i64, sample_rate: f64) -> C64 {
        let white = self.rng.complex_normal();
        let shaped = if self.lowpass.is_empty() {
            white
        } else {
            self.white.rotate_right(1);
            self.white[0] = white;
            self.white
                .iter()
                .zip(&self.lowpass)
                .map(|(sample, tap)| sample * tap)
                .sum()
        };
        let mix = TAU * self.signal.offset_hz() * n as f64 / sample_rate;
        if let SceneSignal::NoiseFm { deviation_hz, .. } = self.signal {
            let message = shaped.re * std::f64::consts::SQRT_2;
            self.phase = (self.phase + TAU * deviation_hz * message / sample_rate).rem_euclid(TAU);
            return C64::from_polar(1.0, self.phase + mix);
        }
        shaped * C64::from_polar(1.0, mix)
    }

    fn value(&self, n: i64, plan: &DelayPlan, sample_rate: f64) -> C64 {
        self.analytic(n, plan.delay, sample_rate)
            .unwrap_or_else(|| plan.read(&self.history, n))
    }

    fn at(&self, n: i64, delay: f64, sample_rate: f64, fractions: &[Vec<f64>]) -> C64 {
        if let Some(value) = self.analytic(n, delay, sample_rate) {
            return value;
        }
        let steps = ECHO_FRACTIONS as f64;
        let quantised = (delay * steps).round() / steps;
        let whole = quantised.floor();
        let step = ((quantised - whole) * steps).round() as usize;
        let at = n - whole as i64;
        match fractions.get(step).filter(|_| step > 0) {
            None => self.history.get(at),
            Some(taps) => taps
                .iter()
                .enumerate()
                .map(|(k, &tap)| self.history.get(at - k as i64 + DELAY_CENTRE) * tap)
                .sum(),
        }
    }

    fn analytic(&self, n: i64, delay: f64, sample_rate: f64) -> Option<C64> {
        let t = (n as f64 - delay) / sample_rate;
        match self.signal {
            SceneSignal::Tone { offset_hz } => {
                Some(C64::from_polar(1.0, TAU * offset_hz * t + self.phase))
            }
            SceneSignal::Fm {
                offset_hz,
                deviation_hz,
                rate_hz,
            } => {
                let swing = if rate_hz > 0.0 {
                    deviation_hz / rate_hz * (TAU * rate_hz * t).sin()
                } else {
                    0.0
                };
                Some(C64::from_polar(
                    1.0,
                    TAU * offset_hz * t + swing + self.phase,
                ))
            }
            SceneSignal::Noise { .. } | SceneSignal::NoiseFm { .. } | SceneSignal::Broadband => {
                None
            }
        }
    }
}

fn lowpass_taps(cutoff: f64) -> Vec<f64> {
    let middle = (NOISE_TAPS - 1) as f64 / 2.0;
    let mut taps: Vec<f64> = (0..NOISE_TAPS)
        .map(|k| {
            let x = k as f64 - middle;
            let window = 0.42 + 0.5 * (PI * x / middle).cos() + 0.08 * (TAU * x / middle).cos();
            2.0 * cutoff * sinc(2.0 * cutoff * x) * window
        })
        .collect();
    let energy: f64 = taps.iter().map(|tap| tap * tap).sum();
    let scale = if energy > 0.0 {
        energy.sqrt().recip()
    } else {
        0.0
    };
    for tap in &mut taps {
        *tap *= scale;
    }
    taps
}

#[derive(Clone, Debug)]
struct Rng(u64);

impl Rng {
    fn seeded(seed: u64, stream: u64) -> Self {
        let mut z = seed ^ stream.wrapping_add(1).wrapping_mul(GOLDEN);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Self(if z == 0 { GOLDEN } else { z })
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(XORSHIFT_STAR)
    }

    fn uniform(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn complex_normal(&mut self) -> C64 {
        let u = 1.0 - self.uniform();
        let v = self.uniform();
        let radius = (-u.ln()).sqrt();
        C64::from_polar(radius, TAU * v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifold::Winding;

    const FREQ: f64 = 433.92e6;

    fn kraken() -> Geometry {
        Geometry::uca(0.35, 5, 0.0, Winding::Clockwise).unwrap()
    }

    fn noisy_scene(seed: u64) -> ArrayScene {
        ArrayScene::new(kraken(), FREQ, 2.4e6)
            .with_source(SceneSource::new(
                Direction::horizon(137.0),
                0.0,
                SceneSignal::Noise {
                    offset_hz: 1e5,
                    bandwidth_hz: 2e5,
                },
            ))
            .with_source(SceneSource {
                delay_samples: 3.4,
                ..SceneSource::new(
                    Direction::horizon(20.0),
                    -6.0,
                    SceneSignal::Fm {
                        offset_hz: -3e5,
                        deviation_hz: 5e3,
                        rate_hz: 1e3,
                    },
                )
            })
            .with_noise_db(-20.0)
            .with_seed(seed)
    }

    #[test]
    fn scene_is_reproducible_from_its_seed() {
        let mut first = noisy_scene(5);
        let mut second = noisy_scene(5);
        second.lane_delay_samples = vec![0.0; 5];
        let whole = first.render(3000).unwrap();
        let mut pieces = vec![Vec::new(); 5];
        for len in [1000, 1, 1999] {
            for (lane, block) in second.render(len).unwrap().into_iter().enumerate() {
                pieces[lane].extend(block);
            }
        }
        assert_eq!(whole, pieces);
        assert_eq!(second.rendered(), 3000);
        let other = noisy_scene(6).render(3000).unwrap();
        assert_ne!(whole, other);
    }

    fn inner(a: &[Complex<f32>], b: &[Complex<f32>]) -> f32 {
        let dot: Complex<f32> = a.iter().zip(b).map(|(x, y)| x.conj() * y).sum();
        let norms = a.iter().map(Complex::norm_sqr).sum::<f32>()
            * b.iter().map(Complex::norm_sqr).sum::<f32>();
        dot.norm() / norms.sqrt()
    }

    fn snapshot(lanes: &[Vec<Complex<f32>>], index: usize) -> Vec<Complex<f32>> {
        lanes.iter().map(|lane| lane[index]).collect()
    }

    #[test]
    fn noise_fm_keeps_its_envelope_and_deviation() {
        let fs = 250e3;
        let deviation = 25e3;
        let signal = SceneSignal::NoiseFm {
            offset_hz: 0.0,
            deviation_hz: deviation,
            bandwidth_hz: 15e3,
        };
        let mut scene = ArrayScene::new(kraken(), FREQ, fs).with_source(SceneSource::new(
            Direction::horizon(0.0),
            0.0,
            signal,
        ));
        let lanes = scene.render(100_000).unwrap();
        assert!(lanes[0].iter().all(|x| (x.norm() - 1.0).abs() < 1e-4));
        let squares: f64 = lanes[0]
            .windows(2)
            .map(|pair| {
                let hz = f64::from((pair[1] * pair[0].conj()).arg()) * fs / TAU;
                hz * hz
            })
            .sum();
        let rms = (squares / (lanes[0].len() - 1) as f64).sqrt();
        assert!((rms / deviation - 1.0).abs() < 0.1, "{rms}");
    }

    #[test]
    fn scene_single_source_matches_the_steering_vector() {
        let offset = 2.5e4;
        let mut scene = ArrayScene::new(kraken(), FREQ, 1e6).with_source(SceneSource::new(
            Direction::new(222.0, 10.0),
            6.0,
            SceneSignal::Tone { offset_hz: offset },
        ));
        let lanes = scene.render(64).unwrap();
        let mut expected = [Complex::new(0.0f32, 0.0); 5];
        steer(
            kraken().positions(),
            FREQ + offset,
            Direction::new(222.0, 10.0),
            &mut expected,
        );
        for index in [0, 17, 63] {
            let x = snapshot(&lanes, index);
            let ratio = x[0] / expected[0];
            for (value, want) in x.iter().zip(&expected) {
                assert!((value - want * ratio).norm() < 1e-4, "{index}");
            }
            assert!((ratio.norm() - 10f32.powf(0.3)).abs() < 1e-3);
        }
        let step = snapshot(&lanes, 1)[0] / snapshot(&lanes, 0)[0];
        assert!((f64::from(step.arg()) - TAU * offset / 1e6).abs() < 1e-5);
    }

    #[test]
    fn scene_rotating_heading_moves_the_body_bearing() {
        let fs = 1e3;
        let mut scene = ArrayScene::new(kraken(), FREQ, fs)
            .with_source(SceneSource::new(
                Direction::horizon(30.0),
                0.0,
                SceneSignal::Tone { offset_hz: 0.0 },
            ))
            .with_heading(HeadingTrack::Rotating {
                start_deg: 0.0,
                rate_dps: 90.0,
            });
        let lanes = scene.render(1001).unwrap();
        let mut body = [Complex::new(0.0f32, 0.0); 5];
        let mut fixed = [Complex::new(0.0f32, 0.0); 5];
        steer(
            kraken().positions(),
            FREQ,
            Direction::horizon(30.0),
            &mut fixed,
        );
        for (index, body_deg) in [(0, 30.0), (500, -15.0), (1000, -60.0)] {
            steer(
                kraken().positions(),
                FREQ,
                Direction::horizon(body_deg),
                &mut body,
            );
            let x = snapshot(&lanes, index);
            assert!(inner(&body, &x) > 0.9999, "{index}");
            if index > 0 {
                assert!(inner(&fixed, &x) < 0.99, "{index}");
            }
        }
    }

    #[test]
    fn a_fractional_lane_delay_shifts_the_signal() {
        let geometry = Geometry::ula(0.01, 2, 90.0).unwrap();
        let mut scene = ArrayScene::new(geometry, 1e6, 1e5)
            .with_source(SceneSource::new(
                Direction::horizon(0.0),
                0.0,
                SceneSignal::Noise {
                    offset_hz: 0.0,
                    bandwidth_hz: 3e4,
                },
            ))
            .with_seed(3);
        scene.lane_delay_samples = vec![0.0, 1.37];
        let lanes = scene.render(20_000).unwrap();
        let mut best = (0.0f64, 0.0f32);
        for step in 0..=300 {
            let lag = 1.0 + f64::from(step) * 0.002;
            let plan = DelayPlan::new(lag);
            let history = History {
                base: 0,
                data: lanes[0].iter().map(|&x| widen(x)).collect(),
            };
            let score: Complex<f64> = (200..19_000)
                .map(|n| plan.read(&history, n).conj() * widen(lanes[1][n as usize]))
                .sum();
            let score = score.norm() as f32;
            if score > best.1 {
                best = (lag, score);
            }
        }
        assert!((best.0 - 1.37).abs() < 0.01, "{best:?}");
    }

    #[test]
    fn copies_dc_gain_and_distortion_shape_the_lanes() {
        fn tilt(element: usize, _azimuth: f64) -> Complex<f32> {
            Complex::from_polar(1.0, element as f32 * 0.1)
        }
        let geometry = Geometry::ula(0.3, 2, 90.0).unwrap();
        let mut scene = ArrayScene::new(geometry, 1e8, 1e4).with_source(SceneSource::new(
            Direction::horizon(0.0),
            0.0,
            SceneSignal::Tone { offset_hz: 100.0 },
        ));
        scene.lane_gain_db = vec![0.0, 6.0];
        scene.lane_phase_deg = vec![0.0, 90.0];
        scene.lane_dc = vec![Complex::new(0.5, 0.0), Complex::new(0.0, 0.0)];
        scene.distortion = Some(tilt);
        let lanes = scene.render(100).unwrap();
        let mean: Complex<f32> = lanes[0].iter().sum::<Complex<f32>>() / 100.0;
        assert!((mean - Complex::new(0.5, 0.0)).norm() < 0.05);
        let ratio = lanes[1][10] / (lanes[0][10] - Complex::new(0.5, 0.0));
        assert!((ratio.norm() - 10f32.powf(0.3)).abs() < 1e-3);
        assert!((ratio.arg() - (std::f32::consts::FRAC_PI_2 + 0.1)).abs() < 1e-3);
        let table = scene.distortion_table(&[1e8], 10.0).unwrap();
        let mut response = [Complex::new(0.0f32, 0.0); 2];
        table
            .response(1e8, Direction::horizon(0.0), &mut response)
            .unwrap();
        assert!(((response[1] / response[0]).arg() - 0.1).abs() < 1e-4);
    }

    #[test]
    fn invalid_scenes_are_refused() {
        let mut scene = noisy_scene(1);
        scene.lane_gain_db = vec![0.0; 3];
        assert_eq!(
            scene.render(10).unwrap_err(),
            SceneError::LaneSetting("lane_gain_db")
        );
        let mut scene = noisy_scene(1);
        scene.sources[1].copy_of = Some(SceneCopy {
            source: 7,
            amplitude: 1.0,
            phase_deg: 0.0,
        });
        assert_eq!(scene.render(10).unwrap_err(), SceneError::Copy(1));
        let mut scene = noisy_scene(1);
        scene.sources[0].signal = SceneSignal::Noise {
            offset_hz: 0.0,
            bandwidth_hz: 0.0,
        };
        assert_eq!(scene.render(10).unwrap_err(), SceneError::Source(0));
        let mut scene = noisy_scene(1);
        scene.sample_rate = 0.0;
        assert_eq!(scene.render(10).unwrap_err(), SceneError::SampleRate);
        let mut scene = noisy_scene(1);
        scene.noise_db[2] = f32::NAN;
        assert_eq!(
            scene.render(10).unwrap_err(),
            SceneError::LaneSetting("noise_db")
        );
        let mut scene = noisy_scene(1).with_heading(HeadingTrack::Fixed(f64::INFINITY));
        assert_eq!(scene.render(10).unwrap_err(), SceneError::Heading);
    }

    #[test]
    fn a_source_power_and_noise_power_are_per_sample() {
        let mut scene = ArrayScene::new(kraken(), FREQ, 1e6)
            .with_source(SceneSource::new(
                Direction::horizon(0.0),
                -3.0,
                SceneSignal::Broadband,
            ))
            .with_seed(2);
        let lanes = scene.render(40_000).unwrap();
        let power = lanes[2].iter().map(Complex::norm_sqr).sum::<f32>() / 40_000.0;
        assert!((10.0 * power.log10() + 3.0).abs() < 0.2, "{power}");
        let mut quiet = ArrayScene::new(kraken(), FREQ, 1e6).with_noise_db(-10.0);
        let lanes = quiet.render(40_000).unwrap();
        let power = lanes[4].iter().map(Complex::norm_sqr).sum::<f32>() / 40_000.0;
        assert!((10.0 * power.log10() + 10.0).abs() < 0.2, "{power}");
    }
}
