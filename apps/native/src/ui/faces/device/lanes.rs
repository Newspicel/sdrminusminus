use std::collections::BTreeSet;

use sdrmm_wire::{
    device::{AgcSetting, Capabilities, Coherence, DeviceSettings, StreamSettings, Tuning},
    patch::{DeviceRef, MAX_STREAMS, NodeBody, PatchGraph, port_stream, stream_port},
    state::{DeviceFault, DeviceSet, DeviceSetStatus},
};

use super::caps::{agc_state, format_gain};
use crate::ui::kit_sources::dial::reachable_hz;

#[must_use]
pub fn rx_stream_count(caps: &Capabilities) -> u32 {
    caps.rx_streams.clamp(1, MAX_STREAMS)
}

#[must_use]
pub fn stream_label(base: &str, index: u32, streams: u32) -> String {
    if streams > 1 && index == 0 {
        format!("{base}1")
    } else {
        stream_port(base, index)
    }
}

#[must_use]
pub fn clipping_said(set: &DeviceSet) -> Option<String> {
    if set.clipping.is_empty() {
        return None;
    }
    let streams = rx_stream_count(&set.capabilities);
    if streams <= 1 {
        return Some("yes".to_owned());
    }
    Some(
        set.clipping
            .iter()
            .map(|lane| stream_label("iq", *lane, streams))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

#[derive(Clone, Debug, PartialEq)]
pub struct TunerDial {
    pub stream: u32,
    pub port: Option<String>,
    pub hz: f64,
}

#[must_use]
pub fn lanes_merged(set: &DeviceSet) -> bool {
    set.capabilities.per_stream.tuning && rx_stream_count(&set.capabilities) > 1
}

#[must_use]
pub fn tuner_dials(set: &DeviceSet) -> Vec<TunerDial> {
    let caps = &set.capabilities;
    let streams = rx_stream_count(caps);
    if !lanes_merged(set) {
        return vec![TunerDial {
            stream: 0,
            port: None,
            hz: set.settings.center_hz.unwrap_or(0.0),
        }];
    }
    (0..streams)
        .map(|stream| TunerDial {
            stream,
            port: Some(stream_label("iq", stream, streams)),
            hz: set
                .settings
                .for_stream(stream, &caps.per_stream)
                .center_hz
                .unwrap_or(0.0),
        })
        .collect()
}

#[must_use]
pub fn has_lane_controls(caps: &Capabilities) -> bool {
    (caps.per_stream.gain && !caps.gains.is_empty())
        || (caps.per_stream.antenna && caps.antennas.len() > 1)
}

#[must_use]
pub fn bond_said(coherence: Coherence) -> Option<&'static str> {
    match coherence {
        Coherence::None => None,
        Coherence::TimeSync => Some("Shared clock"),
        Coherence::PhaseCoherent => Some("Phase coherent"),
    }
}

#[must_use]
pub fn auto_tuning(set: &DeviceSet, stream: u32) -> bool {
    set.settings
        .for_stream(stream, &set.capabilities.per_stream)
        .tuning
        .unwrap_or(Tuning::Auto)
        == Tuning::Auto
}

#[must_use]
pub fn tune_delta(caps: &Capabilities, stream: u32, hz: f64) -> DeviceSettings {
    let center_hz = Some(reachable_hz(&caps.freq_ranges, hz));
    if caps.per_stream.tuning {
        DeviceSettings {
            streams: vec![StreamSettings {
                stream,
                center_hz,
                tuning: Some(Tuning::Manual),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        }
    } else {
        DeviceSettings {
            center_hz,
            tuning: Some(Tuning::Manual),
            ..DeviceSettings::default()
        }
    }
}

#[must_use]
pub fn tuning_delta(caps: &Capabilities, stream: u32, tuning: Tuning) -> DeviceSettings {
    if caps.per_stream.tuning {
        DeviceSettings {
            streams: vec![StreamSettings {
                stream,
                tuning: Some(tuning),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        }
    } else {
        DeviceSettings {
            tuning: Some(tuning),
            ..DeviceSettings::default()
        }
    }
}

#[must_use]
pub fn lane_agc(set: &DeviceSet, stream: u32) -> AgcSetting {
    agc_state(
        &set.capabilities,
        &set.settings
            .for_stream(stream, &set.capabilities.per_stream),
    )
}

#[must_use]
pub fn agc_delta(caps: &Capabilities, stream: u32, agc: AgcSetting) -> DeviceSettings {
    if caps.per_stream.agc {
        DeviceSettings {
            streams: vec![StreamSettings {
                stream,
                agc: Some(agc),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        }
    } else {
        DeviceSettings {
            agc: Some(agc),
            ..DeviceSettings::default()
        }
    }
}

#[must_use]
pub fn agc_gain_db(set: &DeviceSet, stream: u32) -> Option<f64> {
    if set.capabilities.gains.len() != 1 || !lane_agc(set, stream).on {
        return None;
    }
    set.agc_gains
        .iter()
        .find(|reading| reading.stream == stream)
        .map(|reading| reading.value_db)
}

#[must_use]
pub fn agc_tip(set: &DeviceSet, stream: u32, advised: bool) -> String {
    if !lane_agc(set, stream).on {
        return if advised {
            "AGC off, as coherent lanes want".to_owned()
        } else {
            "Let the radio set its own gain".to_owned()
        };
    }
    let reading = match (agc_gain_db(set, stream), set.capabilities.gains.first()) {
        (Some(db), Some(stage)) => format!(" at {} dB", format_gain(stage.unit, db)),
        _ => String::new(),
    };
    let advice = if advised {
        ". Fixed gain keeps coherent lanes calibrated"
    } else {
        ""
    };
    format!("AGC on{reading}{advice}")
}

fn coherent_user(body: &NodeBody) -> bool {
    matches!(
        body,
        NodeBody::Df(_)
            | NodeBody::Combiner(_)
            | NodeBody::Stitch(_)
            | NodeBody::PassiveRadar(_)
            | NodeBody::Array(_)
    )
}

#[must_use]
pub fn coherent_lanes(graph: &PatchGraph, device_node: &str) -> BTreeSet<u32> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.from.node == device_node)
        .filter(|edge| {
            graph
                .node(&edge.to.node)
                .is_some_and(|node| coherent_user(&node.body))
        })
        .filter_map(|edge| port_stream("iq", &edge.from.port))
        .collect()
}

#[must_use]
pub fn lock_stream(locked: &[u32], stream: u32, held: bool) -> Vec<u32> {
    let mut next: Vec<u32> = locked.iter().copied().filter(|s| *s != stream).collect();
    if held {
        next.push(stream);
        next.sort_unstable();
    }
    next
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Danger,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hearing {
    pub heard: usize,
    pub total: usize,
    pub tone: Tone,
}

#[must_use]
pub fn hearing(set: &DeviceSet) -> Hearing {
    let total = set.channels.len();
    let heard = set.channels.iter().filter(|c| !c.out_of_band).count();
    let missing = set.status != DeviceSetStatus::Running || (total > 0 && heard == 0);
    let tone = if missing {
        Tone::Danger
    } else if heard == total {
        Tone::Ok
    } else {
        Tone::Warn
    };
    Hearing { heard, total, tone }
}

#[must_use]
pub fn ref_label(reference: &DeviceRef) -> String {
    match reference.key.as_ref().or(reference.serial.as_ref()) {
        Some(identity) => format!("{} · {identity}", reference.backend),
        None => reference.backend.clone(),
    }
}

#[must_use]
pub fn refusal_said(set: &DeviceSet) -> Option<String> {
    if set.error.is_some() {
        return None;
    }
    let names = &set.refused.as_ref()?.settings;
    let listed = match names.as_slice() {
        [] => return Some("Radio refused the change".to_owned()),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    Some(format!("Radio refused the new {listed}"))
}

#[must_use]
pub fn fault_said(set: &DeviceSet) -> Option<String> {
    let said = match set.fault? {
        DeviceFault::Unplugged => {
            "is no longer attached. Plug it back in and it picks up where it left off."
        }
        DeviceFault::InUse => {
            "is open in another program. Close that one, and this radio comes back."
        }
        DeviceFault::Permissions => {
            "may not be opened by this user. Open Check hardware for the USB permission line, which names the device node and the group that owns it."
        }
        DeviceFault::Other => return None,
    };
    Some(format!("{} {said}", set.device.label))
}

#[cfg(test)]
#[path = "lanes_tests.rs"]
mod tests;
