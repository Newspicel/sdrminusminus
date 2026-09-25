use std::sync::Arc;

use sdrmm_wire::{
    channel::{
        ChannelInfo, ChannelSettings, MAX_SQUELCH_AUTO_MARGIN_DB, MIN_SQUELCH_AUTO_MARGIN_DB,
        Squelch,
    },
    device::{DeviceInfo, DeviceSettings},
    patch::{DeviceRef, NodeBody, PatchNode},
    state::{DeviceSet, DeviceSetStatus},
};
use zgui::prelude::*;

use crate::{
    format,
    socket::Spectrum,
    store::Store,
    ui::{
        gpu,
        plot::{self, Palette},
        widgets::{check, dial, level_bar, pick, row_field, segments, slide},
    },
};

pub mod array;
pub mod audio_fx;
pub mod audio_recorder;
pub mod baseband_recorder;
pub mod baseband_scope;
pub mod channel;
pub mod combiner;
pub mod decoder_log;
pub mod device;
pub mod df;
pub mod dmr_trunk;
pub mod event_filter;
pub mod event_output;
pub mod export;
pub mod gps;
pub mod hunt;
pub mod map;
pub mod network_export;
pub mod passive_radar;
pub mod propagation;
pub mod readout;
pub mod recorder;
pub mod recording;
pub mod satellite;
pub mod scanner;
pub mod scope;
pub mod signal_gen;
pub mod signal_map;
pub mod speaker;
pub mod spectrum_monitor;
pub mod stitch;
pub mod time_machine;
pub mod triangulation;
pub mod video;

pub fn face(store: Store, node: &PatchNode) -> AnyView {
    match &node.body {
        NodeBody::Device(_) => AnyView::new(device::face(store, node.id.clone())),
        NodeBody::Channel(_) => AnyView::new(channel::face(store, node.id.clone())),
        NodeBody::Scope => AnyView::new(scope::face(store, node.id.clone())),
        NodeBody::Speaker => AnyView::new(speaker::face(store, node.id.clone())),
        NodeBody::DecoderLog => AnyView::new(decoder_log::face(store, node.id.clone())),
        NodeBody::Array(_) => AnyView::new(array::face(store, node.id.clone())),
        NodeBody::AudioFx(_) => AnyView::new(audio_fx::face(store, node.id.clone())),
        NodeBody::AudioRecorder(_) => AnyView::new(audio_recorder::face(store, node.id.clone())),
        NodeBody::BasebandRecorder(_) => {
            AnyView::new(baseband_recorder::face(store, node.id.clone()))
        }
        NodeBody::BasebandScope => AnyView::new(baseband_scope::face(store, node.id.clone())),
        NodeBody::Combiner(_) => AnyView::new(combiner::face(store, node.id.clone())),
        NodeBody::Df(_) => AnyView::new(df::face(store, node.id.clone())),
        NodeBody::DmrTrunk(_) => AnyView::new(dmr_trunk::face(store, node.id.clone())),
        NodeBody::EventFilter(_) => AnyView::new(event_filter::face(store, node.id.clone())),
        NodeBody::EventOutput(_) => AnyView::new(event_output::face(store, node.id.clone())),
        NodeBody::Export => AnyView::new(export::face(store, node.id.clone())),
        NodeBody::Gps(_) => AnyView::new(gps::face(store, node.id.clone())),
        NodeBody::Hunt(_) => AnyView::new(hunt::face(store, node.id.clone())),
        NodeBody::Map => AnyView::new(map::face(store, node.id.clone())),
        NodeBody::NetworkExport(_) => AnyView::new(network_export::face(store, node.id.clone())),
        NodeBody::PassiveRadar(_) => AnyView::new(passive_radar::face(store, node.id.clone())),
        NodeBody::Propagation(_) => AnyView::new(propagation::face(store, node.id.clone())),
        NodeBody::Readout => AnyView::new(readout::face(store, node.id.clone())),
        NodeBody::Recorder(_) => AnyView::new(recorder::face(store, node.id.clone())),
        NodeBody::Recording(_) => AnyView::new(recording::face(store, node.id.clone())),
        NodeBody::Satellite(_) => AnyView::new(satellite::face(store, node.id.clone())),
        NodeBody::Scanner => AnyView::new(scanner::face(store, node.id.clone())),
        NodeBody::SignalGen(_) => AnyView::new(signal_gen::face(store, node.id.clone())),
        NodeBody::SignalMap(_) => AnyView::new(signal_map::face(store, node.id.clone())),
        NodeBody::SpectrumMonitor(_) => {
            AnyView::new(spectrum_monitor::face(store, node.id.clone()))
        }
        NodeBody::Stitch(_) => AnyView::new(stitch::face(store, node.id.clone())),
        NodeBody::TimeMachine(_) => AnyView::new(time_machine::face(store, node.id.clone())),
        NodeBody::Triangulation => AnyView::new(triangulation::face(store, node.id.clone())),
        NodeBody::Video => AnyView::new(video::face(store, node.id.clone())),
    }
}

pub fn status_of(store: Store, node: &str) -> (&'static str, &'static str) {
    if !carries_status(store, node) {
        return ("idle", "");
    }
    let Some(set) = store.device_set_of(node) else {
        return ("idle", "UNBOUND");
    };
    match store.set_of(set).map(|set| set.status) {
        Some(DeviceSetStatus::Running) => ("run", "RUNNING"),
        Some(DeviceSetStatus::Error) => ("err", "ERROR"),
        _ => ("idle", "IDLE"),
    }
}

fn carries_status(store: Store, node: &str) -> bool {
    store.graph.get().nodes.iter().any(|found| {
        found.id == node && matches!(found.body, NodeBody::Device(_) | NodeBody::Channel(_))
    })
}

pub(crate) fn plain(kind: &str) -> impl IntoView {
    let kind = kind.replace('_', " ");
    view! {
        column(class = "face") {
            text(class = "hint") {{kind}}
        }
    }
}

pub(crate) fn set_signal(store: Store, node: String) -> Signal<Option<DeviceSet>> {
    Signal::derive(move || store.device_set_of(&node).and_then(|id| store.set_of(id)))
}

pub(crate) fn channel_signal(store: Store, node: String) -> Signal<Option<ChannelInfo>> {
    Signal::derive(move || store.channel_of(&node))
}

type ChannelEdit = Box<dyn FnOnce(&mut ChannelSettings)>;

pub(crate) fn channel_writer(
    store: Store,
    node: String,
    channel: Signal<Option<ChannelInfo>>,
) -> impl Fn(ChannelEdit) + Clone {
    move |change| {
        if let Some(mut settings) = channel.get_untracked().map(|channel| channel.settings) {
            change(&mut settings);
            store.set_channel(node.clone(), settings);
        }
    }
}

pub(crate) fn spectrum_signal(store: Store, node: String) -> Signal<Option<Arc<Spectrum>>> {
    Signal::derive(move || {
        let set = store.device_set_of(&node)?;
        store.spectra.get().get(&set).cloned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_radio_is_named_by_its_driver_and_key() {
        let info = DeviceInfo {
            driver: "virtual".to_owned(),
            key: "siggen".to_owned(),
            label: "Signal Generator (virtual)".to_owned(),
            serial: None,
            profile: None,
        };
        assert_eq!(device::device_key(&info), "virtual:siggen");
    }
}
