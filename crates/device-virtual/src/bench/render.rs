use std::{
    f32::consts::FRAC_1_SQRT_2,
    f64::consts::{PI, TAU},
    sync::OnceLock,
};

use num_complex::Complex;
use sdrmm_dsp::manifold::{Direction, LIGHT_SPEED_M_S, Vec3, steer};

use super::scene::{Echo, Emitter, LaneImpairments, Pilot, Scene, Waveform, mix};

type C32 = Complex<f32>;
type C64 = Complex<f64>;

const CHUNK: usize = 8_192;
const MASTER_BLOCK: usize = 4_096;
const MASTER_BLOCKS: usize = 3;
const TAPS: usize = 32;
const TAP_LEAD: i64 = 15;
const PHASES: usize = 1_024;
const KAISER_BETA: f64 = 8.0;
const CLUTTER_SALT: u64 = 0xC1_0773_2000;

#[derive(Clone, Debug)]
pub(crate) struct LaneSetup<'a> {
    pub(crate) position: [f64; 3],
    pub(crate) center_hz: f64,
    pub(crate) radio_center_hz: f64,
    pub(crate) sample_rate: f64,
    pub(crate) impairments: &'a LaneImpairments,
    pub(crate) gain_setting_db: f64,
    pub(crate) scramble_rad: f64,
    pub(crate) pilot: Option<Pilot>,
    pub(crate) noise_seed: Option<u64>,
}

impl LaneSetup<'_> {
    fn element(&self) -> Vec3 {
        Vec3::new(self.position[0], self.position[1], self.position[2])
    }

    fn in_band(&self, offset_hz: f64, half_bandwidth_hz: f64) -> bool {
        offset_hz.abs() + half_bandwidth_hz <= 0.5 * self.sample_rate
    }
}

#[must_use]
pub(crate) fn amplitude(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

fn cis(radians: f64) -> C64 {
    C64::from_polar(1.0, radians)
}

fn cycles(freq_hz: f64, time_s: f64) -> C64 {
    cis(TAU * (freq_hz * time_s).rem_euclid(1.0))
}

fn narrow(value: C64) -> C32 {
    C32::new(value.re as f32, value.im as f32)
}

fn widen(value: C32) -> C64 {
    C64::new(f64::from(value.re), f64::from(value.im))
}

struct Rotor {
    value: C64,
    step: C64,
}

impl Rotor {
    fn new(freq_hz: f64, start_s: f64, step_s: f64) -> Self {
        Self {
            value: cycles(freq_hz, start_s),
            step: cycles(freq_hz, step_s),
        }
    }

    fn next(&mut self) -> C64 {
        let value = self.value;
        self.value *= self.step;
        value
    }
}

pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(mix(seed) | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn open_unit(&mut self) -> f32 {
        ((self.next() >> 40) as f32 + 0.5) / (1u64 << 24) as f32
    }

    fn normal_pair(&mut self, sigma: f32) -> C32 {
        let radius = (-2.0 * self.open_unit().ln()).sqrt() * sigma;
        let (sin, cos) = (std::f32::consts::TAU * self.open_unit()).sin_cos();
        C32::new(radius * cos, radius * sin)
    }
}

struct SincTable {
    rows: Vec<[f32; TAPS]>,
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        (PI * x).sin() / (PI * x)
    }
}

fn bessel_i0(x: f64) -> f64 {
    let quarter = x * x / 4.0;
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..200 {
        term *= quarter / (k * k) as f64;
        sum += term;
        if term < sum * 1e-16 {
            break;
        }
    }
    sum
}

impl SincTable {
    fn shared() -> &'static Self {
        static TABLE: OnceLock<SincTable> = OnceLock::new();
        TABLE.get_or_init(Self::build)
    }

    fn build() -> Self {
        let half = TAPS as f64 / 2.0;
        let scale = bessel_i0(KAISER_BETA);
        let rows = (0..=PHASES)
            .map(|phase| {
                let mu = phase as f64 / PHASES as f64;
                let mut values = [0.0f64; TAPS];
                for (tap, value) in values.iter_mut().enumerate() {
                    let distance = tap as f64 - TAP_LEAD as f64 - mu;
                    let ratio = distance / half;
                    let window = if ratio.abs() < 1.0 {
                        bessel_i0(KAISER_BETA * (1.0 - ratio * ratio).sqrt()) / scale
                    } else {
                        0.0
                    };
                    *value = sinc(distance) * window;
                }
                let sum: f64 = values.iter().sum();
                values.map(|value| (value / sum) as f32)
            })
            .collect();
        Self { rows }
    }

    fn interpolate(&self, window: &[C32], mu: f64) -> C64 {
        let phase = ((mu * PHASES as f64).round() as usize).min(PHASES);
        let row = &self.rows[phase];
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (value, tap) in window.iter().zip(row) {
            re += value.re * tap;
            im += value.im * tap;
        }
        C64::new(f64::from(re), f64::from(im))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Symbols {
    Gaussian,
    Binary,
}

struct Master {
    seed: u64,
    rate: f64,
    symbols: Symbols,
    first_block: i64,
    filled: bool,
    data: Vec<C32>,
}

impl Master {
    fn new(seed: u64, rate: f64, symbols: Symbols) -> Self {
        Self {
            seed,
            rate,
            symbols,
            first_block: 0,
            filled: false,
            data: vec![C32::new(0.0, 0.0); MASTER_BLOCK * MASTER_BLOCKS],
        }
    }

    fn at(&mut self, time_s: f64) -> C64 {
        let x = time_s * self.rate;
        let floor = x.floor();
        let start = floor as i64 - TAP_LEAD;
        self.ensure(start);
        let offset = (start - self.first_block * MASTER_BLOCK as i64) as usize;
        SincTable::shared().interpolate(&self.data[offset..offset + TAPS], x - floor)
    }

    fn ensure(&mut self, start: i64) {
        let block_len = MASTER_BLOCK as i64;
        let block = start.div_euclid(block_len);
        let last = (start + TAPS as i64 - 1).div_euclid(block_len);
        let span = MASTER_BLOCKS as i64;
        if self.filled && block >= self.first_block && last < self.first_block + span {
            return;
        }
        let shift = block - self.first_block;
        let kept = if self.filled && (1..span).contains(&shift) {
            let shift = shift as usize;
            self.data.copy_within(shift * MASTER_BLOCK.., 0);
            MASTER_BLOCKS - shift
        } else {
            0
        };
        for slot in kept..MASTER_BLOCKS {
            self.fill(slot, block + slot as i64);
        }
        self.first_block = block;
        self.filled = true;
    }

    fn fill(&mut self, slot: usize, block: i64) {
        let mut rng = Rng::new(mix(self.seed ^ mix(block as u64)));
        let symbols = self.symbols;
        for value in &mut self.data[slot * MASTER_BLOCK..(slot + 1) * MASTER_BLOCK] {
            *value = match symbols {
                Symbols::Gaussian => rng.normal_pair(FRAC_1_SQRT_2),
                Symbols::Binary if rng.next() >> 63 == 0 => C32::new(1.0, 0.0),
                Symbols::Binary => C32::new(-1.0, 0.0),
            };
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Tone,
    Fm { beta: f64, rate_hz: f64 },
    Sampled,
}

#[derive(Clone, Copy, Debug)]
struct Fold {
    rate: f64,
    period_s: f64,
}

struct Ray {
    kind: Kind,
    offset_hz: f64,
    scale: f64,
    shift_s: f64,
    fold: Option<Fold>,
    gain: C64,
    doppler_hz: f64,
    master: Option<Master>,
}

#[derive(Clone, Copy, Debug)]
struct Wave {
    waveform: Waveform,
    offset_hz: f64,
    seed: u64,
}

impl Wave {
    fn of(emitter: &Emitter, offset_hz: f64, seed: u64) -> Self {
        Self {
            waveform: emitter.waveform,
            offset_hz,
            seed,
        }
    }

    fn ray(self, gain: C64, shift_s: f64) -> Ray {
        let (kind, master) = match self.waveform {
            Waveform::Tone => (Kind::Tone, None),
            Waveform::Fm {
                deviation_hz,
                rate_hz,
            } => (
                Kind::Fm {
                    beta: deviation_hz / rate_hz,
                    rate_hz,
                },
                None,
            ),
            Waveform::Noise { bandwidth_hz } => (
                Kind::Sampled,
                Some(Master::new(self.seed, bandwidth_hz, Symbols::Gaussian)),
            ),
            Waveform::Bpsk { symbol_rate } => (
                Kind::Sampled,
                Some(Master::new(self.seed, symbol_rate, Symbols::Binary)),
            ),
        };
        Ray {
            kind,
            offset_hz: self.offset_hz,
            scale: 1.0,
            shift_s,
            fold: None,
            gain,
            doppler_hz: 0.0,
            master,
        }
    }
}

impl Ray {
    fn shift_at(&self, arrival_s: f64) -> f64 {
        match self.fold {
            Some(fold) => {
                let cycles = (arrival_s / fold.period_s).floor();
                self.shift_s - fold.rate * cycles * fold.period_s
            }
            None => self.shift_s,
        }
    }

    fn add(&mut self, arrival_s: f64, step_s: f64, out: &mut [C32]) {
        let start = self.scale * arrival_s + self.shift_at(arrival_s);
        let step = self.scale * step_s;
        let mut carrier = Rotor::new(self.offset_hz, start, step);
        let mut doppler = Rotor::new(self.doppler_hz, arrival_s, step_s);
        let gain = self.gain;
        match (self.kind, self.master.as_mut()) {
            (Kind::Fm { beta, rate_hz }, _) => {
                let mut modulation = Rotor::new(rate_hz, start, step);
                for slot in out.iter_mut() {
                    let swing = (beta * modulation.next().im) as f32;
                    let (sin, cos) = swing.sin_cos();
                    let value = carrier.next() * C64::new(f64::from(cos), f64::from(sin));
                    *slot += narrow(gain * doppler.next() * value);
                }
            }
            (Kind::Sampled, Some(master)) => {
                for (n, slot) in out.iter_mut().enumerate() {
                    let value = carrier.next() * master.at(start + n as f64 * step);
                    *slot += narrow(gain * doppler.next() * value);
                }
            }
            (Kind::Tone | Kind::Sampled, _) => {
                for slot in out.iter_mut() {
                    *slot += narrow(gain * doppler.next() * carrier.next());
                }
            }
        }
    }
}

struct Arrival {
    steer: C64,
    lead_s: f64,
}

fn arrival(element: Vec3, center_hz: f64, direction: Direction) -> Arrival {
    let mut response = [C32::new(0.0, 0.0)];
    steer(&[element], center_hz, direction, &mut response);
    Arrival {
        steer: widen(response[0]),
        lead_s: element.dot(direction.unit()) / LIGHT_SPEED_M_S,
    }
}

fn emitter_seed(scene_seed: u64, emitter: usize) -> u64 {
    mix(scene_seed ^ mix(emitter as u64 + 1))
}

fn echo_ray(wave: Wave, setup: &LaneSetup<'_>, echo: &Echo, level: f64) -> Ray {
    let reach = arrival(
        setup.element(),
        setup.center_hz,
        Direction::horizon(echo.azimuth_deg),
    );
    let mut ray = wave.ray(
        reach.steer * level * amplitude(echo.gain_db),
        reach.lead_s - echo.delay_s,
    );
    let rate = echo.doppler_hz / setup.center_hz.max(1.0);
    ray.scale = 1.0 + rate;
    ray.doppler_hz = echo.doppler_hz;
    ray.fold = (rate != 0.0 && echo.delay_s > 0.0).then(|| Fold {
        rate,
        period_s: echo.delay_s / (2.0 * rate.abs()),
    });
    ray
}

fn rays(scene: &Scene, setup: &LaneSetup<'_>) -> Vec<Ray> {
    let element = setup.element();
    let shift_hz = setup.radio_center_hz - setup.center_hz;
    let mut rays = Vec::new();
    for (index, emitter) in scene.emitters.iter().enumerate() {
        let offset = emitter.offset_hz + shift_hz;
        if !setup.in_band(offset, emitter.waveform.half_bandwidth_hz()) {
            continue;
        }
        let wave = Wave::of(emitter, offset, emitter_seed(scene.seed, index));
        let level = amplitude(emitter.power_dbfs);
        let direct = arrival(
            element,
            setup.center_hz,
            Direction::new(emitter.azimuth_deg, emitter.elevation_deg),
        );
        rays.push(wave.ray(direct.steer * level, direct.lead_s));
        for path in &emitter.paths {
            let reach = arrival(
                element,
                setup.center_hz,
                Direction::new(path.azimuth_deg, path.elevation_deg),
            );
            let gain = reach.steer * amplitude(path.gain_db) * cis(path.phase_deg.to_radians());
            rays.push(wave.ray(gain * level, reach.lead_s - path.delay_s));
        }
        for echo in scene.echoes.iter().filter(|echo| echo.emitter == index) {
            rays.push(echo_ray(wave, setup, echo, level));
        }
    }
    rays
}

struct ClutterField {
    illuminator: Ray,
    taps: Vec<(usize, C64)>,
    reach: usize,
    history: Vec<C32>,
    next_h: Option<i64>,
    last_len: usize,
}

impl ClutterField {
    fn of(scene: &Scene, setup: &LaneSetup<'_>) -> Option<Self> {
        let clutter = scene.clutter;
        let emitter = scene.emitters.first()?;
        let offset = emitter.offset_hz + setup.radio_center_hz - setup.center_hz;
        if clutter.echoes == 0 || !setup.in_band(offset, emitter.waveform.half_bandwidth_hz()) {
            return None;
        }
        let reach = clutter.max_delay_samples as usize;
        let level = amplitude(emitter.power_dbfs) * amplitude(clutter.gain_db);
        let mut draw = Rng::new(scene.seed ^ CLUTTER_SALT);
        let taps = (0..clutter.echoes)
            .map(|_| {
                let delay = 1 + (draw.next() % u64::from(clutter.max_delay_samples)) as usize;
                let azimuth = f64::from(draw.open_unit()) * 360.0;
                let phase = f64::from(draw.open_unit()) * TAU;
                let reach = arrival(
                    setup.element(),
                    setup.center_hz,
                    Direction::horizon(azimuth),
                );
                (delay, reach.steer * level * cis(phase))
            })
            .collect();
        let wave = Wave::of(emitter, offset, emitter_seed(scene.seed, 0));
        Some(Self {
            illuminator: wave.ray(C64::new(1.0, 0.0), 0.0),
            taps,
            reach,
            history: vec![C32::new(0.0, 0.0); reach + CHUNK],
            next_h: None,
            last_len: 0,
        })
    }

    fn add(&mut self, h: i64, arrival_s: f64, step_s: f64, out: &mut [C32]) {
        let (reach, len) = (self.reach, out.len());
        if self.next_h == Some(h) {
            self.history
                .copy_within(self.last_len..self.last_len + reach, 0);
        } else {
            let head = &mut self.history[..reach];
            head.fill(C32::new(0.0, 0.0));
            self.illuminator
                .add(arrival_s - reach as f64 * step_s, step_s, head);
        }
        let body = &mut self.history[reach..reach + len];
        body.fill(C32::new(0.0, 0.0));
        self.illuminator.add(arrival_s, step_s, body);
        for (n, slot) in out.iter_mut().enumerate() {
            let mut acc = C64::new(0.0, 0.0);
            for (delay, coefficient) in &self.taps {
                acc += coefficient * widen(self.history[reach + n - delay]);
            }
            *slot += narrow(acc);
        }
        self.next_h = Some(h + len as i64);
        self.last_len = len;
    }
}

struct NoiseFeed {
    master: Master,
    level: f64,
}

impl NoiseFeed {
    fn add(&mut self, arrival_s: f64, step_s: f64, out: &mut [C32]) {
        for (n, slot) in out.iter_mut().enumerate() {
            *slot += narrow(self.master.at(arrival_s + n as f64 * step_s) * self.level);
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Tone {
    offset_hz: f64,
    level: f64,
}

pub(crate) struct LaneRenderer {
    rays: Vec<Ray>,
    clutter: Option<ClutterField>,
    noise: Option<NoiseFeed>,
    pilot: Option<Tone>,
    response: C64,
    hardware_delay_s: f64,
    carrier_error_hz: f64,
    thermal_sigma: f32,
    dc: C32,
    rng: Rng,
    antenna: Vec<C32>,
}

impl LaneRenderer {
    pub(crate) fn new(thermal_seed: u64) -> Self {
        Self {
            rays: Vec::new(),
            clutter: None,
            noise: None,
            pilot: None,
            response: C64::new(1.0, 0.0),
            hardware_delay_s: 0.0,
            carrier_error_hz: 0.0,
            thermal_sigma: 0.0,
            dc: C32::new(0.0, 0.0),
            rng: Rng::new(thermal_seed),
            antenna: vec![C32::new(0.0, 0.0); CHUNK],
        }
    }

    pub(crate) fn configure(&mut self, scene: &Scene, setup: &LaneSetup<'_>) {
        let lane = setup.impairments;
        self.rays = rays(scene, setup);
        self.clutter = ClutterField::of(scene, setup);
        self.noise = setup.noise_seed.map(|seed| NoiseFeed {
            master: Master::new(seed, scene.noise_bandwidth, Symbols::Gaussian),
            level: amplitude(scene.noise_source_dbfs),
        });
        self.pilot = setup
            .pilot
            .filter(|pilot| setup.in_band(pilot.offset_hz, 0.0))
            .map(|pilot| Tone {
                offset_hz: pilot.offset_hz,
                level: amplitude(pilot.power_dbfs),
            });
        let turn = (lane.phase_deg + lane.phase_per_db * setup.gain_setting_db).to_radians();
        self.response = amplitude(lane.gain_db) * cis(turn + setup.scramble_rad);
        self.hardware_delay_s = lane.frac_delay / setup.sample_rate;
        self.carrier_error_hz = lane.ppm * 1e-6 * setup.center_hz;
        self.thermal_sigma = (amplitude(scene.thermal_dbfs) as f32) * FRAC_1_SQRT_2;
        self.dc = lane.dc_dbfs.map_or(C32::new(0.0, 0.0), |db| {
            narrow(amplitude(db) * cis(lane.dc_phase_deg.to_radians()))
        });
    }

    pub(crate) fn render(
        &mut self,
        h: i64,
        start_s: f64,
        step_s: f64,
        noise_on: bool,
        out: &mut [C32],
    ) {
        for (index, chunk) in out.chunks_mut(CHUNK).enumerate() {
            let offset = index * CHUNK;
            let start = start_s + offset as f64 * step_s;
            self.render_chunk(h + offset as i64, start, step_s, noise_on, chunk);
        }
    }

    fn render_chunk(&mut self, h: i64, start_s: f64, step_s: f64, noise_on: bool, out: &mut [C32]) {
        let antenna = &mut self.antenna[..out.len()];
        antenna.fill(C32::new(0.0, 0.0));
        let arrival_s = start_s - self.hardware_delay_s;
        for ray in &mut self.rays {
            ray.add(arrival_s, step_s, antenna);
        }
        if let Some(clutter) = &mut self.clutter {
            clutter.add(h, arrival_s, step_s, antenna);
        }
        if noise_on && let Some(noise) = &mut self.noise {
            noise.add(arrival_s, step_s, antenna);
        }
        if let Some(pilot) = self.pilot {
            let mut tone = Rotor::new(pilot.offset_hz, arrival_s, step_s);
            for slot in antenna.iter_mut() {
                *slot += narrow(tone.next() * pilot.level);
            }
        }
        let mut error = Rotor::new(self.carrier_error_hz, start_s, step_s);
        for (slot, value) in out.iter_mut().zip(antenna.iter()) {
            let shaped = narrow(self.response * error.next() * widen(*value));
            *slot = shaped + self.rng.normal_pair(self.thermal_sigma) + self.dc;
        }
    }

    #[cfg(test)]
    pub(crate) fn clutter_taps(&self) -> Vec<(usize, C64)> {
        self.clutter
            .as_ref()
            .map(|clutter| clutter.taps.clone())
            .unwrap_or_default()
    }
}
