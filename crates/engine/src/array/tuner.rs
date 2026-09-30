use std::collections::BTreeMap;

use sdrmm_wire::{
    AgcSetting, ArrayFailure, ArrayGain, ArrayTune, ArrayTuningMode, Capabilities, DeviceSettings,
    GainKind, GainStage, GainValue, Range, StreamSettings, Tuning,
};

use super::LaneRef;
use crate::DEFAULT_SAMPLE_RATE;

const SAME_STEP_DB: f64 = 0.05;
const MAX_MENU_STEPS: usize = 4_096;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TunePlan {
    pub(crate) deltas: Vec<(u32, DeviceSettings)>,
    pub(crate) lane_centers_hz: Vec<f64>,
    pub(crate) gain_db: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GainMenu {
    Steps(Vec<f64>),
    Span(Range),
}

impl GainMenu {
    fn of(stage: &GainStage) -> Self {
        if !stage.values.is_empty() {
            let mut steps = stage.values.clone();
            steps.sort_by(f64::total_cmp);
            return Self::Steps(steps);
        }
        match stage.range.step.filter(|step| *step > 0.0) {
            Some(step) if stage.setting_count() <= MAX_MENU_STEPS => Self::Steps(
                (0..stage.setting_count())
                    .map(|index| (stage.range.min + index as f64 * step).min(stage.range.max))
                    .collect(),
            ),
            _ => Self::Span(Range {
                min: stage.range.min,
                max: stage.range.max,
                step: None,
            }),
        }
    }

    fn holds(&self, value: f64) -> bool {
        match self {
            Self::Steps(steps) => steps
                .iter()
                .any(|step| (step - value).abs() <= SAME_STEP_DB),
            Self::Span(range) => range.holds(value),
        }
    }

    fn intersect(self, other: &Self) -> Self {
        match (self, other) {
            (Self::Steps(steps), other) => Self::Steps(
                steps
                    .into_iter()
                    .filter(|step| other.holds(*step))
                    .collect(),
            ),
            (Self::Span(range), Self::Steps(steps)) => Self::Steps(
                steps
                    .iter()
                    .copied()
                    .filter(|step| range.holds(*step))
                    .collect(),
            ),
            (Self::Span(range), Self::Span(other)) => {
                let min = range.min.max(other.min);
                let max = range.max.min(other.max);
                if min <= max {
                    Self::Span(Range {
                        min,
                        max,
                        step: None,
                    })
                } else {
                    Self::Steps(Vec::new())
                }
            }
        }
    }

    pub(crate) fn snap(&self, value_db: f64) -> Option<f64> {
        match self {
            Self::Steps(steps) => steps.iter().copied().min_by(|a, b| {
                (a - value_db)
                    .abs()
                    .total_cmp(&(b - value_db).abs())
                    .then(a.total_cmp(b))
            }),
            Self::Span(range) => Some(value_db.clamp(range.min, range.max)),
        }
    }

    pub(crate) fn steps(&self) -> Vec<f64> {
        match self {
            Self::Steps(steps) => steps.clone(),
            Self::Span(range) => {
                let low = range.min.ceil();
                let count = (range.max.floor() - low).max(0.0) as usize + 1;
                (0..count.min(MAX_MENU_STEPS))
                    .map(|step| low + step as f64)
                    .collect()
            }
        }
    }

    pub(crate) fn range(&self) -> Option<Range> {
        match self {
            Self::Steps(steps) => Some(Range {
                min: *steps.first()?,
                max: *steps.last()?,
                step: None,
            }),
            Self::Span(range) => Some(*range),
        }
    }
}

pub(crate) fn main_stage(caps: &Capabilities) -> Option<&GainStage> {
    caps.gains
        .iter()
        .find(|stage| stage.kind == GainKind::Tuner)
        .or_else(|| {
            caps.gains.iter().find(|stage| {
                !stage.is_switch() && !matches!(stage.kind, GainKind::Attenuator | GainKind::Tx)
            })
        })
}

pub(crate) fn gain_menu<'a>(caps: impl IntoIterator<Item = &'a Capabilities>) -> Option<GainMenu> {
    let mut menu: Option<GainMenu> = None;
    for caps in caps {
        let stage = GainMenu::of(main_stage(caps)?);
        menu = Some(match menu {
            Some(menu) => menu.intersect(&stage),
            None => stage,
        });
    }
    menu.filter(|menu| menu.range().is_some())
}

struct Device<'a> {
    ds: u32,
    caps: &'a Capabilities,
    lanes: Vec<(usize, u32)>,
}

fn devices<'a>(
    lanes: &[Option<LaneRef>],
    caps: &'a BTreeMap<u32, Capabilities>,
) -> Result<Vec<Device<'a>>, ArrayFailure> {
    let mut devices: Vec<Device<'a>> = Vec::new();
    for (slot, lane) in lanes.iter().enumerate() {
        let Some(lane) = lane else { continue };
        let Some(device_caps) = caps.get(&lane.device_set) else {
            return Err(ArrayFailure::DeviceDown { lane: slot as u32 });
        };
        match devices
            .iter_mut()
            .find(|device| device.ds == lane.device_set)
        {
            Some(device) => device.lanes.push((slot, lane.stream)),
            None => devices.push(Device {
                ds: lane.device_set,
                caps: device_caps,
                lanes: vec![(slot, lane.stream)],
            }),
        }
    }
    Ok(devices)
}

fn lane_centers(
    lanes: &[Option<LaneRef>],
    current: &BTreeMap<u32, DeviceSettings>,
    mode: ArrayTuningMode,
    center_hz: f64,
) -> Vec<f64> {
    match mode {
        ArrayTuningMode::Together => vec![center_hz; lanes.len()],
        ArrayTuningMode::Spread => {
            let rate = lanes
                .iter()
                .flatten()
                .find_map(|lane| current.get(&lane.device_set)?.sample_rate)
                .unwrap_or(DEFAULT_SAMPLE_RATE);
            sdrmm_dsp::stitch::auto_offsets(lanes.len(), rate)
                .into_iter()
                .map(|offset| center_hz + offset)
                .collect()
        }
    }
}

fn gain_values(stage: Option<&GainStage>, gain_db: Option<f64>) -> Vec<GainValue> {
    match (stage, gain_db) {
        (Some(stage), Some(db)) => vec![GainValue {
            stage: stage.name.clone(),
            value_db: stage.snap(db),
        }],
        _ => Vec::new(),
    }
}

fn holds_every_stream(device: &Device<'_>) -> bool {
    (0..device.caps.rx_streams).all(|stream| device.lanes.iter().any(|(_, held)| *held == stream))
}

fn device_delta(
    device: &Device<'_>,
    mode: ArrayTuningMode,
    centers: &[f64],
    tune: &ArrayTune,
    gain_db: Option<f64>,
) -> Result<DeviceSettings, ArrayFailure> {
    let scope = device.caps.per_stream;
    let gains = gain_values(main_stage(device.caps), gain_db);
    let off = device.caps.agc.offered().then(AgcSetting::off);
    let mut delta = DeviceSettings::default();
    if !scope.tuning {
        if mode == ArrayTuningMode::Spread && device.lanes.len() > 1 {
            return Err(ArrayFailure::SpreadUnsupported);
        }
        delta.center_hz = device.lanes.first().map(|(slot, _)| centers[*slot]);
        delta.tuning = Some(Tuning::Manual);
    } else if holds_every_stream(device) {
        delta.center_hz = Some(tune.center_hz);
        delta.tuning = Some(Tuning::Manual);
    }
    if !scope.gain {
        delta.gains.clone_from(&gains);
    }
    if !scope.agc {
        delta.agc.clone_from(&off);
    }
    if scope.tuning || scope.gain || scope.agc {
        delta.streams = device
            .lanes
            .iter()
            .map(|(slot, stream)| StreamSettings {
                stream: *stream,
                center_hz: scope.tuning.then_some(centers[*slot]),
                tuning: scope.tuning.then_some(Tuning::Manual),
                gains: if scope.gain {
                    gains.clone()
                } else {
                    Vec::new()
                },
                antenna: None,
                agc: if scope.agc { off.clone() } else { None },
            })
            .collect();
    }
    Ok(delta)
}

pub(crate) fn plan(
    lanes: &[Option<LaneRef>],
    caps: &BTreeMap<u32, Capabilities>,
    current: &BTreeMap<u32, DeviceSettings>,
    mode: ArrayTuningMode,
    tune: &ArrayTune,
) -> Result<TunePlan, ArrayFailure> {
    let devices = devices(lanes, caps)?;
    let lane_centers_hz = lane_centers(lanes, current, mode, tune.center_hz);
    let gain_db = match tune.gain {
        ArrayGain::Manual { db } => gain_menu(devices.iter().map(|device| device.caps))
            .map_or(Some(db), |menu| menu.snap(db)),
        ArrayGain::Auto => None,
    };
    let deltas = devices
        .iter()
        .map(|device| {
            device_delta(device, mode, &lane_centers_hz, tune, gain_db)
                .map(|delta| (device.ds, delta))
        })
        .collect::<Result<Vec<_>, ArrayFailure>>()?;
    Ok(TunePlan {
        deltas,
        lane_centers_hz,
        gain_db,
    })
}

#[cfg(test)]
mod tests;
