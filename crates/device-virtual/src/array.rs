use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_device::DeviceError;
use sdrmm_wire::{DeviceSettings, ExtraSetting, ExtraValue, Range};

pub const BEARING_SETTING: &str = "wavefront_bearing_deg";
pub const RADIUS_SETTING: &str = "array_radius_m";
pub const SCRAMBLE_SETTING: &str = "lane_phase_scramble";
pub const ECHO_DELAY_SETTING: &str = "echo_delay_samples";
pub const ECHO_DOPPLER_SETTING: &str = "echo_doppler_hz";
pub const ECHO_GAIN_SETTING: &str = "echo_gain_db";

/// The one signal every lane shares fills the whole span, because that is what a real
/// illuminator does and what gives an echo a range worth measuring. What is fixed is where a
/// reader can look at it undisturbed: a window clear of the per-lane markers, which start at
/// 50 kHz and go up in steps of the same.
pub const WAVEFRONT_OFFSET_HZ: f64 = 25_000.0;
pub const WAVEFRONT_WINDOW_HZ: f64 = 6_000.0;
const WAVEFRONT_AMP: f64 = 0.5;
const WAVEFRONT_SEED: u64 = 0x5164_A15E_0000_0001;

pub const MAX_ECHO_DELAY_SAMPLES: usize = 4_096;
pub const MAX_ECHO_DOPPLER_HZ: f64 = 2_000.0;
const LIGHT_SPEED_M_S: f64 = 299_792_458.0;

/// The lane the echo lands on, so a surveillance/reference pair is a fixed property of the
/// instrument rather than something a test has to arrange.
pub const ECHO_LANE: usize = 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArrayParams {
    pub bearing_deg: f64,
    pub radius_m: f64,
    pub scramble: bool,
    pub echo_delay: usize,
    pub echo_doppler_hz: f64,
    pub echo_gain_db: f64,
}

impl ArrayParams {
    /// Whether the shared wavefront is rendered at all. An array with no extent and no echo to
    /// build is a bank of independent receivers, and that is what the instrument stays until the
    /// operator says otherwise.
    #[must_use]
    pub const fn carries_wavefront(&self) -> bool {
        self.radius_m > 0.0 || self.echo_delay > 0
    }
}

impl Default for ArrayParams {
    fn default() -> Self {
        Self {
            bearing_deg: 0.0,
            radius_m: 0.0,
            scramble: false,
            echo_delay: 0,
            echo_doppler_hz: 0.0,
            echo_gain_db: -20.0,
        }
    }
}

#[must_use]
pub fn extra_settings() -> Vec<ExtraSetting> {
    vec![
        ExtraSetting::Range {
            name: BEARING_SETTING.to_string(),
            range: Range {
                min: 0.0,
                max: 360.0,
                step: None,
            },
            unit: "°".to_string(),
        },
        ExtraSetting::Range {
            name: RADIUS_SETTING.to_string(),
            range: Range {
                min: 0.0,
                max: 10.0,
                step: None,
            },
            unit: "m".to_string(),
        },
        ExtraSetting::Bool {
            name: SCRAMBLE_SETTING.to_string(),
            default: false,
        },
        ExtraSetting::Range {
            name: ECHO_DELAY_SETTING.to_string(),
            range: Range {
                min: 0.0,
                max: MAX_ECHO_DELAY_SAMPLES as f64,
                step: Some(1.0),
            },
            unit: "samples".to_string(),
        },
        ExtraSetting::Range {
            name: ECHO_DOPPLER_SETTING.to_string(),
            range: Range {
                min: -MAX_ECHO_DOPPLER_HZ,
                max: MAX_ECHO_DOPPLER_HZ,
                step: None,
            },
            unit: "Hz".to_string(),
        },
        ExtraSetting::Range {
            name: ECHO_GAIN_SETTING.to_string(),
            range: Range {
                min: -60.0,
                max: 0.0,
                step: None,
            },
            unit: "dB".to_string(),
        },
    ]
}

#[must_use]
pub fn default_extra() -> Vec<ExtraValue> {
    let defaults = ArrayParams::default();
    vec![
        number(BEARING_SETTING, defaults.bearing_deg),
        number(RADIUS_SETTING, defaults.radius_m),
        ExtraValue {
            name: SCRAMBLE_SETTING.to_string(),
            value: serde_json::Value::Bool(defaults.scramble),
        },
        number(ECHO_DELAY_SETTING, defaults.echo_delay as f64),
        number(ECHO_DOPPLER_SETTING, defaults.echo_doppler_hz),
        number(ECHO_GAIN_SETTING, defaults.echo_gain_db),
    ]
}

fn number(name: &str, value: f64) -> ExtraValue {
    ExtraValue {
        name: name.to_string(),
        value: serde_json::Number::from_f64(value).map_or(serde_json::Value::Null, Into::into),
    }
}

pub fn validate(settings: &DeviceSettings) -> Result<(), DeviceError> {
    for extra in &settings.extra {
        let name = extra.name.as_str();
        let bounded = |low: f64, high: f64| -> Result<(), DeviceError> {
            match extra.value.as_f64() {
                Some(value) if (low..=high).contains(&value) => Ok(()),
                _ => Err(DeviceError::Unsupported(format!(
                    "`{name}` must be a number in {low}..={high}, got {}",
                    extra.value
                ))),
            }
        };
        match name {
            BEARING_SETTING => bounded(0.0, 360.0)?,
            RADIUS_SETTING => bounded(0.0, 10.0)?,
            ECHO_DELAY_SETTING => bounded(0.0, MAX_ECHO_DELAY_SAMPLES as f64)?,
            ECHO_DOPPLER_SETTING => bounded(-MAX_ECHO_DOPPLER_HZ, MAX_ECHO_DOPPLER_HZ)?,
            ECHO_GAIN_SETTING => bounded(-60.0, 0.0)?,
            SCRAMBLE_SETTING if extra.value.is_boolean() => {}
            SCRAMBLE_SETTING => {
                return Err(DeviceError::Unsupported(format!(
                    "`{SCRAMBLE_SETTING}` must be a boolean, got {}",
                    extra.value
                )));
            }
            other => {
                return Err(DeviceError::Unsupported(format!("extra `{other}`")));
            }
        }
    }
    Ok(())
}

#[must_use]
pub fn read(settings: &DeviceSettings) -> ArrayParams {
    let defaults = ArrayParams::default();
    let number = |name: &str, fallback: f64| {
        settings
            .extra
            .iter()
            .find(|extra| extra.name == name)
            .and_then(|extra| extra.value.as_f64())
            .unwrap_or(fallback)
    };
    ArrayParams {
        bearing_deg: number(BEARING_SETTING, defaults.bearing_deg),
        radius_m: number(RADIUS_SETTING, defaults.radius_m),
        scramble: settings
            .extra
            .iter()
            .find(|extra| extra.name == SCRAMBLE_SETTING)
            .and_then(|extra| extra.value.as_bool())
            .unwrap_or(defaults.scramble),
        echo_delay: number(ECHO_DELAY_SETTING, defaults.echo_delay as f64) as usize,
        echo_doppler_hz: number(ECHO_DOPPLER_SETTING, defaults.echo_doppler_hz),
        echo_gain_db: number(ECHO_GAIN_SETTING, defaults.echo_gain_db),
    }
}

/// The phase a plane wave from `bearing_deg` carries at element `lane` of a uniform circular
/// array of `lanes` elements, relative to the array centre. Bearings are compass bearings, and
/// element zero sits due north, so a reading and a placement can be compared without a convention
/// to remember.
#[must_use]
pub fn steering_phase(lane: usize, lanes: usize, params: &ArrayParams, freq_hz: f64) -> f64 {
    if params.radius_m <= 0.0 || lanes == 0 || freq_hz <= 0.0 {
        return 0.0;
    }
    let wavelength_m = LIGHT_SPEED_M_S / freq_hz;
    let element = TAU * lane as f64 / lanes as f64;
    let arrival = params.bearing_deg.to_radians();
    TAU * params.radius_m * (element - arrival).cos() / wavelength_m
}

/// The per-lane phase a receiver with its own synthesizer comes up at after a retune. Derived
/// from the tuning rather than drawn at random, so a test can retune twice and get the same
/// answer twice while still seeing phase move.
#[must_use]
pub fn scramble_phase(lane: usize, center_hz: f64) -> f64 {
    let mut state =
        (center_hz.to_bits() ^ ((lane as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15))) | 1;
    state ^= state >> 33;
    state = state.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    state ^= state >> 33;
    TAU * (state >> 11) as f64 / (1u64 << 53) as f64
}

/// The one waveform every lane of the array sees, plus the delayed copy of it a passive-radar
/// test needs on the surveillance lane.
pub struct ArrayField {
    state: u64,
    doppler_phase: f64,
    tail: Vec<Complex<f64>>,
}

impl Default for ArrayField {
    fn default() -> Self {
        Self::new()
    }
}

impl ArrayField {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: WAVEFRONT_SEED,
            doppler_phase: 0.0,
            tail: vec![Complex::new(0.0, 0.0); MAX_ECHO_DELAY_SAMPLES],
        }
    }

    /// Renders the next `len` samples of the shared wavefront.
    pub fn fill(&mut self, out: &mut Vec<Complex<f64>>, len: usize, _sample_rate: f64) {
        out.clear();
        out.reserve(len);
        for _ in 0..len {
            out.push(Complex::new(
                self.next() * WAVEFRONT_AMP,
                self.next() * WAVEFRONT_AMP,
            ));
        }
    }

    fn next(&mut self) -> f64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        let bits = (self.state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f64;
        bits / (1u64 << 23) as f64 - 1.0
    }

    /// Keeps the end of the block the echo of the next one reaches back into.
    pub fn commit(&mut self, common: &[Complex<f64>]) {
        let keep = common.len().min(MAX_ECHO_DELAY_SAMPLES);
        self.tail.rotate_left(keep);
        let start = self.tail.len() - keep;
        self.tail[start..].copy_from_slice(&common[common.len() - keep..]);
    }

    /// Adds this lane's share of the field to a block that already carries its own marker.
    pub fn add_lane(
        &mut self,
        lane: usize,
        block: &mut [Complex<f32>],
        common: &[Complex<f64>],
        phase: f64,
        params: &ArrayParams,
        sample_rate: f64,
    ) {
        let steer = Complex::from_polar(1.0, phase);
        for (slot, sample) in block.iter_mut().zip(common) {
            let field = sample * steer;
            *slot += Complex::new(field.re as f32, field.im as f32);
        }
        if lane != ECHO_LANE || params.echo_delay == 0 || params.echo_delay > MAX_ECHO_DELAY_SAMPLES
        {
            return;
        }
        let gain = 10f64.powf(params.echo_gain_db / 20.0);
        let doppler_w = params.echo_doppler_hz * TAU / sample_rate;
        let mut doppler = self.doppler_phase;
        for (index, slot) in block.iter_mut().enumerate().take(common.len()) {
            let echo = if index >= params.echo_delay {
                common[index - params.echo_delay]
            } else {
                self.tail[self.tail.len() + index - params.echo_delay]
            };
            let shifted = echo * Complex::from_polar(gain, doppler);
            doppler += doppler_w;
            *slot += Complex::new(shifted.re as f32, shifted.im as f32);
        }
        self.doppler_phase = doppler.rem_euclid(TAU);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_radius_of_zero_leaves_every_lane_in_phase() {
        let params = ArrayParams {
            bearing_deg: 137.0,
            ..ArrayParams::default()
        };
        for lane in 0..4 {
            assert_eq!(steering_phase(lane, 4, &params, 100e6), 0.0);
        }
    }

    #[test]
    fn the_element_facing_the_source_leads_the_one_behind_it() {
        let params = ArrayParams {
            bearing_deg: 0.0,
            radius_m: 0.5,
            ..ArrayParams::default()
        };
        let front = steering_phase(0, 4, &params, 300e6);
        let back = steering_phase(2, 4, &params, 300e6);
        assert!(front > back, "front {front} back {back}");
        assert!((front + back).abs() < 1e-9, "the pair must be symmetric");
    }

    #[test]
    fn scramble_is_repeatable_per_tuning_but_moves_between_them() {
        let a = scramble_phase(1, 100e6);
        assert!((a - scramble_phase(1, 100e6)).abs() < 1e-12);
        assert!((a - scramble_phase(1, 100.1e6)).abs() > 1e-6);
        assert!((a - scramble_phase(2, 100e6)).abs() > 1e-6);
        for lane in 0..8 {
            let phase = scramble_phase(lane, 433e6);
            assert!((0.0..TAU).contains(&phase), "{phase}");
        }
    }

    #[test]
    fn defaults_round_trip_through_the_settings_table() {
        let settings = DeviceSettings {
            extra: default_extra(),
            ..DeviceSettings::default()
        };
        validate(&settings).expect("defaults are valid");
        assert_eq!(read(&settings), ArrayParams::default());
    }

    #[test]
    fn out_of_range_settings_are_refused_by_name() {
        for (name, value) in [
            (BEARING_SETTING, 400.0),
            (RADIUS_SETTING, 50.0),
            (ECHO_DELAY_SETTING, 99_999.0),
            (ECHO_DOPPLER_SETTING, 1e6),
            (ECHO_GAIN_SETTING, 10.0),
        ] {
            let settings = DeviceSettings {
                extra: vec![number(name, value)],
                ..DeviceSettings::default()
            };
            let Err(DeviceError::Unsupported(message)) = validate(&settings) else {
                panic!("{name} = {value} must be refused");
            };
            assert!(message.contains(name), "{message}");
        }
    }

    #[test]
    fn the_echo_lands_at_the_configured_delay() {
        let params = ArrayParams {
            echo_delay: 64,
            echo_gain_db: 0.0,
            ..ArrayParams::default()
        };
        let mut field = ArrayField::new();
        let mut common = Vec::new();
        let rate = 1_000_000.0;
        field.fill(&mut common, 512, rate);
        let mut reference = vec![Complex::new(0.0f32, 0.0); 512];
        field.add_lane(0, &mut reference, &common, 0.0, &params, rate);
        let mut surveillance = vec![Complex::new(0.0f32, 0.0); 512];
        field.add_lane(ECHO_LANE, &mut surveillance, &common, 0.0, &params, rate);
        for index in 128..512 {
            let echo = surveillance[index] - reference[index];
            let want = reference[index - 64];
            assert!(
                (echo - want).norm() < 1e-3,
                "sample {index}: echo {echo} want {want}"
            );
        }
    }
}
