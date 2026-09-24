use sdrmm_dsp::stitch::{STITCH_KEEP, auto_offsets};
use sdrmm_wire::{CoherentParams, DeviceSettings, StitchMode, StreamSettings, Tuning};

use crate::{
    DEFAULT_CENTER_HZ, DeviceSetState, EngineError, RebuildEntry, center_of,
    coherent::CoherentCommand, sample_rate_of, tune_together,
};

pub(crate) struct Stitched {
    pub(crate) mode: StitchMode,
    pub(crate) lanes: Vec<u32>,
}

struct Target {
    center_hz: Option<f64>,
    tuning: Option<Tuning>,
}

impl DeviceSetState {
    pub(crate) fn stitched(&self) -> Option<Stitched> {
        let coherent = self.coherent.as_ref()?;
        coherent
            .nodes
            .iter()
            .find_map(|(node, params)| match params {
                CoherentParams::Stitch(stitch) => coherent.lanes.get(node).map(|lanes| Stitched {
                    mode: stitch.mode,
                    lanes: lanes.clone(),
                }),
                _ => None,
            })
    }

    pub(crate) fn is_extra_lane(&self, stream: u32) -> bool {
        stream == self.capabilities.rx_streams && self.cmd_txs.len() as u32 > stream
    }

    pub(crate) fn extra_lane(&self) -> Option<sdrmm_wire::ExtraLane> {
        let stream = self.capabilities.rx_streams;
        self.is_extra_lane(stream).then(|| sdrmm_wire::ExtraLane {
            stream,
            center_hz: self.lane_center(&self.settings, stream),
            sample_rate: self.lane_rate(&self.settings, stream),
        })
    }

    pub(crate) fn lane_rate(&self, tuning: &DeviceSettings, stream: u32) -> f64 {
        let rate = sample_rate_of(tuning);
        match self.stitched() {
            Some(stitched) if self.is_extra_lane(stream) => rate * stitched.lanes.len() as f64,
            _ => rate,
        }
    }

    pub(crate) fn lane_center(&self, tuning: &DeviceSettings, stream: u32) -> f64 {
        if self.is_extra_lane(stream) {
            self.extra_center(tuning)
        } else {
            center_of(tuning, stream, &self.capabilities.per_stream)
        }
    }

    pub(crate) fn extra_center(&self, tuning: &DeviceSettings) -> f64 {
        let scope = &self.capabilities.per_stream;
        match self.stitched() {
            Some(stitched) => stitch_center(tuning, &stitched, scope),
            None => self.coherent_lanes().first().map_or_else(
                || center_of(tuning, 0, scope),
                |lane| center_of(tuning, *lane, scope),
            ),
        }
    }

    pub(crate) fn admits(
        &self,
        node: Option<u32>,
        params: &CoherentParams,
    ) -> Result<(), EngineError> {
        let others = self.coherent.as_ref().map_or(0, |coherent| {
            coherent
                .nodes
                .keys()
                .filter(|existing| Some(**existing) != node)
                .count()
        });
        let held = self.coherent.as_ref().is_some_and(|coherent| {
            coherent.nodes.iter().any(|(existing, params)| {
                Some(*existing) != node && matches!(params, CoherentParams::Stitch(_))
            })
        });
        if matches!(params, CoherentParams::Stitch(_)) {
            if !self.capabilities.per_stream.tuning {
                return Err(EngineError::Coherent(
                    "Stitch needs a radio that tunes each lane on its own".to_owned(),
                ));
            }
            if others > 0 {
                return Err(EngineError::Coherent(
                    "Stitch needs the radio's lanes to itself".to_owned(),
                ));
            }
        } else if held {
            return Err(EngineError::Coherent(
                "a Stitch holds this radio's lanes".to_owned(),
            ));
        }
        Ok(())
    }

    pub(crate) fn misaligned(&self) -> Option<DeviceSettings> {
        let scope = &self.capabilities.per_stream;
        let lead = *self.coherent_lanes().first()?;
        if !scope.tuning {
            return None;
        }
        let current = self.settings.for_stream(lead, scope);
        let mut delta = DeviceSettings {
            streams: vec![StreamSettings {
                stream: lead,
                center_hz: current.center_hz,
                tuning: Some(current.tuning.unwrap_or_default()),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        };
        self.lay_out(&mut delta);
        let aligned = delta.streams.iter().all(|entry| {
            let now = self.settings.for_stream(entry.stream, scope);
            entry.center_hz.is_none_or(|hz| now.center_hz == Some(hz))
                && entry
                    .tuning
                    .is_none_or(|tuning| now.tuning.unwrap_or_default() == tuning)
        });
        (!aligned).then_some(delta)
    }

    pub(crate) fn plan_stitch(&self) -> Option<DeviceSettings> {
        let stitched = self.stitched()?;
        if stitched.mode != StitchMode::Auto {
            return None;
        }
        let scope = &self.capabilities.per_stream;
        let lead = *stitched.lanes.first()?;
        if !self.settings.for_stream(lead, scope).tunes_itself() {
            return None;
        }
        let extra = self.capabilities.rx_streams;
        let heard: Vec<_> = self
            .channels
            .iter()
            .filter(|channel| channel.stream == extra || stitched.lanes.contains(&channel.stream))
            .cloned()
            .collect();
        let current_hz = self.extra_center(&self.settings);
        let center_hz = crate::planning::best_center_in(
            &self.capabilities,
            &self.settings,
            extra,
            &heard,
            (
                current_hz,
                usable_span(stitched.lanes.len(), sample_rate_of(&self.settings)),
            ),
        )?;
        if center_hz == current_hz {
            return None;
        }
        let mut delta = DeviceSettings {
            streams: vec![StreamSettings {
                stream: extra,
                center_hz: Some(center_hz),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        };
        self.lay_out(&mut delta);
        Some(delta)
    }

    pub(crate) fn send_meta(&self, retuned: bool) {
        let Some(coherent) = self.coherent.as_ref() else {
            return;
        };
        let lanes_hz = (0..self.capabilities.rx_streams)
            .map(|lane| center_of(&self.settings, lane, &self.capabilities.per_stream))
            .collect();
        coherent.runtime.send(CoherentCommand::Meta {
            center_hz: self.extra_center(&self.settings),
            lanes_hz,
            retuned,
        });
    }

    pub(crate) fn extra_rebuilds(&self) -> Vec<RebuildEntry> {
        let extra = self.capabilities.rx_streams;
        self.channels
            .iter()
            .filter(|channel| channel.stream == extra)
            .filter_map(|channel| {
                self.media.get(&channel.id).map(|media| RebuildEntry {
                    id: channel.id,
                    stream: channel.stream,
                    settings: channel.settings.clone(),
                    sinks: media.sinks.clone(),
                })
            })
            .collect()
    }

    pub(crate) fn lay_out(&self, delta: &mut DeviceSettings) {
        let extra = self.capabilities.rx_streams;
        let target = if self.is_extra_lane(extra) {
            take_extra(delta, extra)
        } else {
            Target {
                center_hz: None,
                tuning: None,
            }
        };
        if !self.capabilities.per_stream.tuning {
            if let Some(center_hz) = target.center_hz {
                delta.center_hz = Some(center_hz);
            }
            return;
        }
        let members = self.coherent_lanes();
        match self.stitched() {
            Some(stitched) if stitched.mode == StitchMode::Auto => {
                self.lay_out_auto(delta, &stitched.lanes, &target);
            }
            Some(stitched) => self.shift_manual(delta, &stitched.lanes, &target),
            None => {
                if let Some(lead) = members.first() {
                    lead_with(delta, *lead, &target);
                }
                tune_together(delta, &members);
            }
        }
    }

    fn lay_out_auto(&self, delta: &mut DeviceSettings, lanes: &[u32], target: &Target) {
        let rate = delta.sample_rate.unwrap_or(sample_rate_of(&self.settings));
        let offsets = auto_offsets(lanes.len(), rate);
        let placed = lanes.iter().zip(&offsets).find_map(|(lane, offset)| {
            delta
                .streams
                .iter()
                .find(|entry| entry.stream == *lane)
                .and_then(|entry| entry.center_hz)
                .map(|hz| hz - offset)
        });
        let tuning = target.tuning.or_else(|| {
            delta
                .streams
                .iter()
                .filter(|entry| lanes.contains(&entry.stream))
                .find_map(|entry| entry.tuning)
        });
        let center = target.center_hz.or(placed).or_else(|| {
            (delta.sample_rate.is_some() || tuning.is_some())
                .then(|| self.extra_center(&self.settings))
        });
        for (lane, offset) in lanes.iter().zip(&offsets) {
            let entry = entry_for(delta, *lane);
            if let Some(center) = center {
                entry.center_hz = Some(center + offset);
            }
            if tuning.is_some() {
                entry.tuning = tuning;
            }
        }
    }

    fn shift_manual(&self, delta: &mut DeviceSettings, lanes: &[u32], target: &Target) {
        let shift = target
            .center_hz
            .map(|hz| hz - self.extra_center(&self.settings));
        for lane in lanes {
            let current = center_of(&self.settings, *lane, &self.capabilities.per_stream);
            let entry = entry_for(delta, *lane);
            if let Some(shift) = shift
                && entry.center_hz.is_none()
            {
                entry.center_hz = Some(current + shift);
            }
            if target.tuning.is_some() {
                entry.tuning = target.tuning;
            }
        }
    }
}

fn usable_span(lanes: usize, rate: f64) -> f64 {
    let offsets = auto_offsets(lanes, rate);
    let reach = offsets
        .iter()
        .map(|offset| offset.abs())
        .fold(0.0, f64::max);
    2.0 * reach + STITCH_KEEP * rate
}

pub(crate) fn stitch_center(
    tuning: &DeviceSettings,
    stitched: &Stitched,
    scope: &sdrmm_wire::StreamScope,
) -> f64 {
    let centers: Vec<f64> = stitched
        .lanes
        .iter()
        .map(|lane| center_of(tuning, *lane, scope))
        .collect();
    match stitched.mode {
        StitchMode::Auto => {
            let offsets = auto_offsets(stitched.lanes.len(), sample_rate_of(tuning));
            centers
                .first()
                .zip(offsets.first())
                .map_or(DEFAULT_CENTER_HZ, |(hz, offset)| hz - offset)
        }
        StitchMode::Manual => {
            let low = centers.iter().copied().fold(f64::INFINITY, f64::min);
            let high = centers.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            if low.is_finite() && high.is_finite() {
                f64::midpoint(low, high)
            } else {
                DEFAULT_CENTER_HZ
            }
        }
    }
}

fn take_extra(delta: &mut DeviceSettings, extra: u32) -> Target {
    let mut target = Target {
        center_hz: None,
        tuning: None,
    };
    delta.streams.retain(|entry| {
        if entry.stream != extra {
            return true;
        }
        target.center_hz = entry.center_hz.or(target.center_hz);
        target.tuning = entry.tuning.or(target.tuning);
        false
    });
    target
}

fn lead_with(delta: &mut DeviceSettings, lead: u32, target: &Target) {
    if target.center_hz.is_none() && target.tuning.is_none() {
        return;
    }
    let entry = entry_for(delta, lead);
    if target.center_hz.is_some() {
        entry.center_hz = target.center_hz;
    }
    if target.tuning.is_some() {
        entry.tuning = target.tuning;
    }
}

fn entry_for(delta: &mut DeviceSettings, lane: u32) -> &mut StreamSettings {
    let at = match delta.streams.iter().position(|entry| entry.stream == lane) {
        Some(at) => at,
        None => {
            delta.streams.push(StreamSettings {
                stream: lane,
                ..StreamSettings::default()
            });
            delta.streams.len() - 1
        }
    };
    &mut delta.streams[at]
}
