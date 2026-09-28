use sdrmm_dsp::manifold::{Geometry, Winding};
use sdrmm_wire::{Coherence, DcArtifact, NoiseSource, StreamScope};

pub const MAX_BENCH_LANES: usize = 16;
pub const MAX_CLUTTER_ECHOES: u32 = 64;
pub const MAX_CLUTTER_DELAY_SAMPLES: u32 = 65_536;
pub const MAX_PPM: f64 = 1_000.0;

const DEFAULT_SEED: u64 = 0x5EED_BE7C_0000_0137;
const IMPAIRMENT_SEED: u64 = 0xB1A5_ED00_0000_0005;
const KRAKEN_RADIUS_M: f64 = 0.35;
const DONGLE_SPACING_M: f64 = 0.5;
const DONGLE2_START_OFFSET: i64 = 480_000;
const KRAKEN_OFFSET_SPAN: f64 = 20_000.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub positions: Vec<[f64; 3]>,
    pub emitters: Vec<Emitter>,
    pub echoes: Vec<Echo>,
    pub clutter: Clutter,
    pub thermal_dbfs: f64,
    pub noise_source_dbfs: f64,
    pub noise_bandwidth: f64,
    pub seed: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Emitter {
    pub azimuth_deg: f64,
    pub elevation_deg: f64,
    pub offset_hz: f64,
    pub power_dbfs: f64,
    pub waveform: Waveform,
    pub paths: Vec<Path>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Waveform {
    Tone,
    Fm { deviation_hz: f64, rate_hz: f64 },
    Noise { bandwidth_hz: f64 },
    Bpsk { symbol_rate: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Path {
    pub azimuth_deg: f64,
    pub elevation_deg: f64,
    pub delay_s: f64,
    pub gain_db: f64,
    pub phase_deg: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Echo {
    pub emitter: usize,
    pub azimuth_deg: f64,
    pub delay_s: f64,
    pub doppler_hz: f64,
    pub gain_db: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Clutter {
    pub echoes: u32,
    pub max_delay_samples: u32,
    pub gain_db: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaneImpairments {
    pub start_offset: i64,
    pub ppm: f64,
    pub gain_db: f64,
    pub phase_deg: f64,
    pub phase_per_db: f64,
    pub frac_delay: f64,
    pub dc_dbfs: Option<f64>,
    pub dc_phase_deg: f64,
    pub scramble_on_retune: bool,
    pub slips: Vec<Slip>,
    pub gaps: Vec<ReportedGap>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slip {
    pub at: u64,
    pub samples: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReportedGap {
    pub at: u64,
    pub missing: u64,
    pub reported: u64,
    pub error: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pilot {
    pub offset_hz: f64,
    pub power_dbfs: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BenchDeviceSpec {
    pub key: String,
    pub label: String,
    pub first_element: usize,
    pub lanes: Vec<LaneImpairments>,
    pub coherence: Coherence,
    pub noise_source: NoiseSource,
    pub noise_from: Option<String>,
    pub pilot: Option<Pilot>,
    pub dc_artifact: DcArtifact,
    pub retune_keeps_phase: bool,
    pub per_stream: StreamScope,
}

impl Waveform {
    #[must_use]
    pub fn half_bandwidth_hz(self) -> f64 {
        match self {
            Self::Tone => 0.0,
            Self::Fm {
                deviation_hz,
                rate_hz,
            } => deviation_hz + rate_hz,
            Self::Noise { bandwidth_hz } => bandwidth_hz / 2.0,
            Self::Bpsk { symbol_rate } => symbol_rate / 2.0,
        }
    }

    fn problem(self) -> Option<&'static str> {
        let sound = match self {
            Self::Tone => true,
            Self::Fm {
                deviation_hz,
                rate_hz,
            } => deviation_hz.is_finite() && deviation_hz >= 0.0 && positive(rate_hz),
            Self::Noise { bandwidth_hz } => positive(bandwidth_hz),
            Self::Bpsk { symbol_rate } => positive(symbol_rate),
        };
        (!sound).then_some("an emitter waveform needs finite, positive rates")
    }
}

fn positive(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

fn all_finite(values: &[f64]) -> bool {
    values.iter().all(|value| value.is_finite())
}

impl Scene {
    #[must_use]
    pub fn problem(&self) -> Option<String> {
        if !self.positions.iter().all(|p| all_finite(p)) {
            return Some("element positions must be finite".to_owned());
        }
        if !all_finite(&[self.thermal_dbfs, self.noise_source_dbfs])
            || !positive(self.noise_bandwidth)
        {
            return Some("noise levels and the noise bandwidth must be finite".to_owned());
        }
        for (index, emitter) in self.emitters.iter().enumerate() {
            if let Some(problem) = emitter.problem() {
                return Some(format!("emitter {index}: {problem}"));
            }
        }
        for (index, echo) in self.echoes.iter().enumerate() {
            if echo.emitter >= self.emitters.len() {
                return Some(format!("echo {index} names a missing emitter"));
            }
            if !all_finite(&[
                echo.azimuth_deg,
                echo.delay_s,
                echo.doppler_hz,
                echo.gain_db,
            ]) {
                return Some(format!("echo {index} must be finite"));
            }
        }
        self.clutter_problem()
    }

    fn clutter_problem(&self) -> Option<String> {
        let clutter = self.clutter;
        if clutter.echoes == 0 {
            return None;
        }
        if self.emitters.is_empty() {
            return Some("clutter needs an emitter to reflect".to_owned());
        }
        let sound = clutter.echoes <= MAX_CLUTTER_ECHOES
            && (1..=MAX_CLUTTER_DELAY_SAMPLES).contains(&clutter.max_delay_samples)
            && clutter.gain_db.is_finite();
        (!sound).then(|| {
            format!(
                "clutter takes at most {MAX_CLUTTER_ECHOES} echoes within 1 to \
                 {MAX_CLUTTER_DELAY_SAMPLES} samples"
            )
        })
    }
}

impl Emitter {
    fn problem(&self) -> Option<&'static str> {
        if !all_finite(&[
            self.azimuth_deg,
            self.elevation_deg,
            self.offset_hz,
            self.power_dbfs,
        ]) {
            return Some("direction, offset and power must be finite");
        }
        let paths_sound = self.paths.iter().all(|path| {
            all_finite(&[
                path.azimuth_deg,
                path.elevation_deg,
                path.delay_s,
                path.gain_db,
                path.phase_deg,
            ])
        });
        if !paths_sound {
            return Some("every path must be finite");
        }
        self.waveform.problem()
    }
}

impl LaneImpairments {
    #[must_use]
    pub fn problem(&self) -> Option<&'static str> {
        let sound = all_finite(&[
            self.ppm,
            self.gain_db,
            self.phase_deg,
            self.phase_per_db,
            self.frac_delay,
            self.dc_phase_deg,
        ]) && self.dc_dbfs.is_none_or(f64::is_finite);
        if !sound {
            return Some("lane impairments must be finite");
        }
        (self.ppm.abs() > MAX_PPM).then_some("a lane clock is off by at most 1000 ppm")
    }
}

impl BenchDeviceSpec {
    pub(crate) fn problem(&self, scene: &Scene) -> Option<String> {
        if self.lanes.is_empty() || self.lanes.len() > MAX_BENCH_LANES {
            return Some(format!("{} needs 1 to {MAX_BENCH_LANES} lanes", self.key));
        }
        let pilot_sound = self
            .pilot
            .is_none_or(|pilot| all_finite(&[pilot.offset_hz, pilot.power_dbfs]));
        if !pilot_sound {
            return Some(format!("{}: the pilot must be finite", self.key));
        }
        let lane_problem = self
            .lanes
            .iter()
            .enumerate()
            .find_map(|(lane, impairments)| {
                impairments
                    .problem()
                    .map(|problem| format!("{} lane {lane}: {problem}", self.key))
            });
        lane_problem.or_else(|| self.coverage(scene))
    }

    pub(crate) fn coverage(&self, scene: &Scene) -> Option<String> {
        let end = self.first_element + self.lanes.len();
        (end > scene.positions.len()).then(|| {
            format!(
                "{} needs elements {} to {}, the scene has {}",
                self.key,
                self.first_element,
                end.saturating_sub(1),
                scene.positions.len()
            )
        })
    }
}

struct Draw(u64);

impl Draw {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        mix(self.0)
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn within(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit()
    }
}

#[must_use]
pub(crate) fn mix(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

#[must_use]
pub(crate) fn mix_key(seed: u64, key: &str) -> u64 {
    key.bytes()
        .fold(mix(seed), |acc, byte| mix(acc ^ u64::from(byte)))
}

fn points(geometry: &Geometry) -> Vec<[f64; 3]> {
    geometry
        .positions()
        .iter()
        .map(|p| [p.x, p.y, p.z])
        .collect()
}

pub(crate) fn uca_positions(radius_m: f64, count: usize) -> Result<Vec<[f64; 3]>, String> {
    Geometry::uca(radius_m, count, 0.0, Winding::Clockwise)
        .map(|geometry| points(&geometry))
        .map_err(|err| err.to_string())
}

fn circle(radius_m: f64, count: usize) -> Vec<[f64; 3]> {
    uca_positions(radius_m, count).unwrap_or_default()
}

fn line(spacing_m: f64, count: usize) -> Vec<[f64; 3]> {
    Geometry::ula(spacing_m, count, 90.0)
        .map(|geometry| points(&geometry))
        .unwrap_or_default()
}

#[must_use]
pub fn default_scene() -> Scene {
    let mut positions = circle(KRAKEN_RADIUS_M, 5);
    positions.extend(circle(KRAKEN_RADIUS_M, 4));
    positions.extend(line(DONGLE_SPACING_M, 2));
    Scene {
        positions,
        emitters: vec![Emitter {
            azimuth_deg: 137.0,
            elevation_deg: 0.0,
            offset_hz: 0.0,
            power_dbfs: -30.0,
            waveform: Waveform::Fm {
                deviation_hz: 75_000.0,
                rate_hz: 1_000.0,
            },
            paths: Vec::new(),
        }],
        echoes: vec![Echo {
            emitter: 0,
            azimuth_deg: 250.0,
            delay_s: 40e-6,
            doppler_hz: 60.0,
            gain_db: -25.0,
        }],
        clutter: Clutter::default(),
        thermal_dbfs: -60.0,
        noise_source_dbfs: -20.0,
        noise_bandwidth: 2_000_000.0,
        seed: DEFAULT_SEED,
    }
}

fn kraken_lanes(draw: &mut Draw) -> Vec<LaneImpairments> {
    (0..5)
        .map(|_| LaneImpairments {
            start_offset: draw.within(-KRAKEN_OFFSET_SPAN, KRAKEN_OFFSET_SPAN).round() as i64,
            frac_delay: draw.unit(),
            phase_deg: draw.within(0.0, 360.0),
            gain_db: draw.within(-1.5, 1.5),
            phase_per_db: 0.3,
            scramble_on_retune: true,
            ..LaneImpairments::default()
        })
        .collect()
}

fn array_lanes(draw: &mut Draw) -> Vec<LaneImpairments> {
    (0..4)
        .map(|_| LaneImpairments {
            frac_delay: draw.within(0.0, 0.1),
            phase_deg: draw.within(0.0, 360.0),
            gain_db: draw.within(-1.0, 1.0),
            ..LaneImpairments::default()
        })
        .collect()
}

fn dongle_lane(draw: &mut Draw, start_offset: i64) -> Vec<LaneImpairments> {
    vec![LaneImpairments {
        start_offset,
        frac_delay: draw.unit(),
        phase_deg: draw.within(0.0, 360.0),
        phase_per_db: 0.3,
        scramble_on_retune: true,
        ..LaneImpairments::default()
    }]
}

const BANK_SCOPE: StreamScope = StreamScope {
    tuning: true,
    gain: true,
    antenna: false,
    agc: false,
};

const SHARED_TUNING_SCOPE: StreamScope = StreamScope {
    tuning: false,
    gain: true,
    antenna: false,
    agc: false,
};

#[must_use]
pub fn default_devices() -> Vec<BenchDeviceSpec> {
    let mut draw = Draw(IMPAIRMENT_SEED);
    vec![
        BenchDeviceSpec {
            key: "kraken5".to_owned(),
            label: "Kraken bench ×5 (virtual)".to_owned(),
            first_element: 0,
            lanes: kraken_lanes(&mut draw),
            coherence: Coherence::TimeSync,
            noise_source: NoiseSource::Isolated,
            noise_from: None,
            pilot: None,
            dc_artifact: DcArtifact::Managed,
            retune_keeps_phase: false,
            per_stream: BANK_SCOPE,
        },
        BenchDeviceSpec {
            key: "array4".to_owned(),
            label: "Coherent Array ×4 (virtual)".to_owned(),
            first_element: 5,
            lanes: array_lanes(&mut draw),
            coherence: Coherence::PhaseCoherent,
            noise_source: NoiseSource::None,
            noise_from: None,
            pilot: Some(Pilot {
                offset_hz: 100_000.0,
                power_dbfs: -25.0,
            }),
            dc_artifact: DcArtifact::Operator,
            retune_keeps_phase: true,
            per_stream: SHARED_TUNING_SCOPE,
        },
        dongle(
            "dongle1",
            "Dongle 1 (virtual)",
            9,
            dongle_lane(&mut draw, 0),
        ),
        BenchDeviceSpec {
            noise_source: NoiseSource::None,
            noise_from: Some("dongle1".to_owned()),
            ..dongle(
                "dongle2",
                "Dongle 2 (virtual)",
                10,
                dongle_lane(&mut draw, DONGLE2_START_OFFSET),
            )
        },
    ]
}

fn dongle(key: &str, label: &str, element: usize, lanes: Vec<LaneImpairments>) -> BenchDeviceSpec {
    BenchDeviceSpec {
        key: key.to_owned(),
        label: label.to_owned(),
        first_element: element,
        lanes,
        coherence: Coherence::TimeSync,
        noise_source: NoiseSource::Isolated,
        noise_from: None,
        pilot: None,
        dc_artifact: DcArtifact::Managed,
        retune_keeps_phase: false,
        per_stream: StreamScope::default(),
    }
}
