use sdrmm_wire::{
    channel::ChannelInfo,
    network::{
        ChannelNetworkExportRequest, NetworkExportAction, NetworkExportRequest,
        NetworkExportSettings, NetworkExportStatus, NetworkSampleFormat, NetworkTransport,
    },
    patch::NodeBody,
    state::{DeviceSet, DeviceSetStatus},
};
use zgui::prelude::*;

use super::device::actions::edit_body;
use crate::{
    binding,
    store::Store,
    ui::{
        kit_sources::{Tone, button, footer, install, readout, text_field, units},
        widgets::{pick, row_field},
    },
};

const UNWIRED: &str = "Wire a running device's IQ or a channel's baseband into this sink first.";

#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Unavailable,
    Ready,
    Active(NetworkExportStatus),
    Busy(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Device { set: u32, stream: u32 },
    Channel { set: u32, channel: u32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub running: bool,
    pub active: Option<NetworkExportStatus>,
}

#[must_use]
pub fn device_source(set: Option<&DeviceSet>) -> Option<Source> {
    set.map(|set| Source {
        running: set.status == DeviceSetStatus::Running,
        active: set.network_export.clone(),
    })
}

#[must_use]
pub fn channel_source(set: Option<&DeviceSet>, channel: Option<&ChannelInfo>) -> Option<Source> {
    Some(Source {
        running: set?.status == DeviceSetStatus::Running,
        active: channel?.network_export.clone(),
    })
}

#[must_use]
pub fn control_of(source: Option<Source>, node: &str) -> Control {
    let Some(source) = source.filter(|source| source.running) else {
        return Control::Unavailable;
    };
    match source.active {
        None => Control::Ready,
        Some(active) if active.node == node => Control::Active(active),
        Some(active) => Control::Busy(active.node),
    }
}

#[must_use]
pub fn controls_locked(control: &Control, pending: bool) -> bool {
    matches!(control, Control::Active(_)) || pending
}

#[derive(Debug, PartialEq)]
pub enum Request {
    Device(String, NetworkExportRequest),
    Channel(String, ChannelNetworkExportRequest),
}

pub fn request_for(
    target: Option<Target>,
    action: NetworkExportAction,
    node: &str,
    settings: NetworkExportSettings,
) -> Result<Request, &'static str> {
    let node = node.to_owned();
    match target.ok_or(UNWIRED)? {
        Target::Device { set, stream } => Ok(Request::Device(
            format!("/api/devicesets/{set}/network-export"),
            NetworkExportRequest {
                action,
                node,
                stream,
                settings,
            },
        )),
        Target::Channel { set, channel } => Ok(Request::Channel(
            format!("/api/devicesets/{set}/channels/{channel}/network-export"),
            ChannelNetworkExportRequest {
                action,
                node,
                settings,
            },
        )),
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Wiring {
    target: Option<Target>,
    control: Control,
    baseband: bool,
}

fn wiring(store: Store, node: &str) -> Wiring {
    let graph = store.graph.get();
    let baseband = binding::sources_of(&graph, node, "baseband")
        .into_iter()
        .find_map(|source| {
            let channel = store.channel_of(&source)?;
            let set = store.set_of(store.device_set_of(&source)?)?;
            Some((set, channel))
        });
    if let Some((set, channel)) = baseband {
        return Wiring {
            target: Some(Target::Channel {
                set: set.id,
                channel: channel.id,
            }),
            control: control_of(channel_source(Some(&set), Some(&channel)), node),
            baseband: true,
        };
    }
    let set = store.device_set_of(node).and_then(|id| store.set_of(id));
    let stream = binding::iq_source_of(&graph, node).map(|(_, stream)| stream);
    Wiring {
        target: set
            .as_ref()
            .zip(stream)
            .map(|(set, stream)| Target::Device {
                set: set.id,
                stream,
            }),
        control: control_of(device_source(set.as_ref()), node),
        baseband: false,
    }
}

fn settings_of(store: Store, node: &str) -> NetworkExportSettings {
    store.graph.with(|graph| {
        graph
            .node(node)
            .and_then(|found| match &found.body {
                NodeBody::NetworkExport(export) => Some(export.settings.clone()),
                _ => None,
            })
            .unwrap_or_default()
    })
}

fn edit(store: Store, node: String, change: impl FnOnce(&mut NetworkExportSettings) + 'static) {
    edit_body(store, node, move |body| {
        if let NodeBody::NetworkExport(export) = body {
            change(&mut export.settings);
        }
    });
}

fn send(store: Store, request: Request, pending: RwSignal<bool>) {
    pending.set(true);
    zgui::task::spawn_local(async move {
        let sent: anyhow::Result<NetworkExportStatus> = match &request {
            Request::Device(path, body) => store.api().post(path, body).await,
            Request::Channel(path, body) => store.api().post(path, body).await,
        };
        pending.try_set(false);
        if let Err(error) = sent {
            store.say(error.to_string());
        }
        store.refresh_state();
    });
}

pub fn face(store: Store, node: String) -> impl IntoView {
    install();
    let wired = {
        let node = node.clone();
        Memo::new(move |_| wiring(store, &node))
    };
    let settings = {
        let node = node.clone();
        Memo::new(move |_| settings_of(store, &node))
    };
    let pending = RwSignal::new(false);
    let locked = Signal::derive(move || controls_locked(&wired.get().control, pending.get()));
    let act = {
        let node = node.clone();
        move |action: NetworkExportAction| match request_for(
            wired.get_untracked().target,
            action,
            &node,
            settings.get_untracked(),
        ) {
            Ok(request) => send(store, request, pending),
            Err(said) => store.say(said),
        }
    };
    let title = move || {
        if wired.get().baseband {
            "Network baseband"
        } else {
            "Network IQ"
        }
    };
    view! {
        column(class = "face") {
            row(class = "kit-head") {
                text(class = "kit-head__title") {{title}}
            }
            {transport_rows(store, node, settings, locked)}
            {move || status_view(&wired.get(), &settings.get())}
            {move || {
                let act = act.clone();
                let busy: Signal<bool> = pending.into();
                if matches!(wired.get().control, Control::Active(_)) {
                    AnyView::new(footer(button(|| "Stop".to_owned(), Tone::Danger, busy, move || act(NetworkExportAction::Stop))))
                } else {
                    let blocked = Signal::derive(move || wired.get().control != Control::Ready || pending.get());
                    AnyView::new(footer(button(|| "Start export".to_owned(), Tone::Plain, blocked, move || act(NetworkExportAction::Start))))
                }
            }}
        }
    }
}

fn transport_rows(
    store: Store,
    node: String,
    settings: Memo<NetworkExportSettings>,
    locked: Signal<bool>,
) -> impl IntoView {
    let transports = vec![
        (NetworkTransport::Udp, "UDP datagrams".to_owned()),
        (NetworkTransport::Tcp, "TCP stream".to_owned()),
        (
            NetworkTransport::RtlTcp,
            "rtl_tcp server (rtl_433)".to_owned(),
        ),
    ];
    let formats = vec![
        (
            NetworkSampleFormat::Cf32Le,
            "Complex float 32 LE".to_owned(),
        ),
        (NetworkSampleFormat::Ci16Le, "Complex int 16 LE".to_owned()),
        (NetworkSampleFormat::Cu8, "Complex unsigned 8".to_owned()),
    ];
    let rtl = Signal::derive(move || settings.get().transport == NetworkTransport::RtlTcp);
    let format_locked = Signal::derive(move || locked.get() || rtl.get());
    let address = Signal::derive(move || settings.get().address);
    let (on_transport, on_format, on_address) = (node.clone(), node.clone(), node);
    view! {
        column(class = "kit-radio") {
            box(class = "kit-slot", class:kit-muted = locked) {
                {row_field("Transport", pick(transports, Signal::derive(move || Some(settings.get().transport)), move |transport| {
                    edit(store, on_transport.clone(), move |settings| {
                        settings.transport = transport;
                        if transport == NetworkTransport::RtlTcp {
                            settings.format = NetworkSampleFormat::Cu8;
                            "127.0.0.1:1234".clone_into(&mut settings.address);
                        }
                    });
                }))}
            }
            box(class = "kit-slot", class:kit-muted = format_locked) {
                {row_field("Samples", pick(formats, Signal::derive(move || Some(settings.get().format)), move |format| {
                    edit(store, on_format.clone(), move |settings| settings.format = format);
                }))}
            }
            {move || row_field(
                if rtl.get() { "Listen on" } else { "Destination" },
                text_field(
                    if rtl.get() { "rtl_tcp listen address" } else { "Network IQ destination" },
                    address,
                    "",
                    locked,
                    |text| !text.trim().is_empty(),
                    {
                        let node = on_address.clone();
                        move |text: String| {
                            let next = text.trim().to_owned();
                            edit(store, node.clone(), move |settings| settings.address = next);
                        }
                    },
                ),
            )}
        }
    }
}

fn status_view(wired: &Wiring, settings: &NetworkExportSettings) -> AnyView {
    if wired.target.is_none() {
        return AnyView::new(
            view! { text(class = "kit-note") {"Wire a device's IQ or a channel's baseband in"} },
        );
    }
    match &wired.control {
        Control::Active(status) => AnyView::new(active(status)),
        Control::Busy(_) => AnyView::new(
            view! { text(class = "kit-note") {"Another network sink already uses this input"} },
        ),
        Control::Ready => {
            let said = if settings.transport == NetworkTransport::RtlTcp {
                "rtl_433 input · CU8"
            } else {
                "Raw interleaved I/Q"
            };
            AnyView::new(view! { text(class = "kit-note") {{said}} })
        }
        Control::Unavailable => AnyView::new(()),
    }
}

fn active(status: &NetworkExportStatus) -> impl IntoView {
    let mut rows = Vec::new();
    if status.settings.transport == NetworkTransport::RtlTcp {
        rows.push((
            "Clients".to_owned(),
            AnyView::new(status.clients.to_string()),
        ));
    }
    rows.push((
        "Rate".to_owned(),
        AnyView::new(units::sample_rate(status.sample_rate as f64)),
    ));
    rows.push((
        "Center".to_owned(),
        AnyView::new(units::hz(status.center_hz as f64)),
    ));
    rows.push((
        "Sent".to_owned(),
        AnyView::new(units::bytes(status.bytes as f64)),
    ));
    let packets = if status.settings.transport == NetworkTransport::Udp {
        "Datagrams"
    } else {
        "Writes"
    };
    rows.push((packets.to_owned(), AnyView::new(status.packets.to_string())));
    if cfg!(debug_assertions) {
        rows.push((
            "Capture loss".to_owned(),
            AnyView::new(format!("{} samples", status.overruns)),
        ));
    }
    let error = status
        .error
        .clone()
        .map(|error| AnyView::new(view! { text(class = "kit-alert") {{error}} }));
    view! { column { {readout(rows)} {error} } }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn settings() -> NetworkExportSettings {
        NetworkExportSettings {
            transport: NetworkTransport::Tcp,
            format: NetworkSampleFormat::Ci16Le,
            address: "analysis.local:7355".to_owned(),
        }
    }

    fn status(node: &str) -> NetworkExportStatus {
        NetworkExportStatus {
            node: node.to_owned(),
            stream: 0,
            settings: settings(),
            sample_rate: 2_400_000,
            center_hz: 100_000_000,
            samples: 10,
            bytes: 80,
            packets: 1,
            clients: 0,
            overruns: 0,
            error: None,
        }
    }

    fn set(status: &str, export: Option<NetworkExportStatus>) -> DeviceSet {
        serde_json::from_value(json!({
            "id": 7, "status": status, "network_export": export,
            "device": { "driver": "virtual", "key": "siggen", "label": "Signal Generator" },
            "capabilities": { "freq_ranges": [], "sample_rates": [], "gains": [], "antennas": [], "bandwidths": [] },
            "settings": {}, "channels": []
        }))
        .expect("device set")
    }

    fn channel(export: Option<NetworkExportStatus>) -> ChannelInfo {
        serde_json::from_value(json!({
            "id": 3, "stream": 0, "out_of_band": false, "network_export": export,
            "settings": { "frequency_hz": 100e6, "params": { "type": "nfm", "settings": {} } }
        }))
        .expect("channel")
    }

    #[test]
    fn the_owner_is_told_apart_from_another_sink_on_the_same_radio() {
        let running = set("running", Some(status("net-a")));
        assert_eq!(
            control_of(device_source(Some(&running)), "net-a"),
            Control::Active(status("net-a"))
        );
        assert_eq!(
            control_of(device_source(Some(&running)), "net-b"),
            Control::Busy("net-a".to_owned())
        );
    }

    #[test]
    fn start_is_offered_only_on_a_running_unclaimed_radio() {
        assert_eq!(
            control_of(device_source(Some(&set("running", None))), "net-a"),
            Control::Ready
        );
        assert_eq!(
            control_of(device_source(Some(&set("error", None))), "net-a"),
            Control::Unavailable
        );
        assert_eq!(
            control_of(device_source(None), "net-a"),
            Control::Unavailable
        );
    }

    #[test]
    fn a_channel_reads_its_own_export_not_the_radios() {
        let radio = set("running", Some(status("net-a")));
        let exported = channel(Some(status("bb-a")));
        assert_eq!(
            control_of(channel_source(Some(&radio), Some(&exported)), "bb-a"),
            Control::Active(status("bb-a"))
        );
        assert_eq!(
            control_of(channel_source(Some(&radio), Some(&channel(None))), "bb-a"),
            Control::Ready
        );
        assert_eq!(
            control_of(channel_source(Some(&radio), None), "bb-a"),
            Control::Unavailable
        );
    }

    #[test]
    fn start_and_stop_carry_the_bound_stream_and_the_current_settings() {
        for action in [NetworkExportAction::Start, NetworkExportAction::Stop] {
            let request = request_for(
                Some(Target::Device { set: 7, stream: 2 }),
                action,
                "net-a",
                settings(),
            );
            assert_eq!(
                request,
                Ok(Request::Device(
                    "/api/devicesets/7/network-export".to_owned(),
                    NetworkExportRequest {
                        action,
                        node: "net-a".to_owned(),
                        stream: 2,
                        settings: settings()
                    }
                ))
            );
        }
    }

    #[test]
    fn a_channels_baseband_goes_to_the_channel_route() {
        let request = request_for(
            Some(Target::Channel { set: 7, channel: 3 }),
            NetworkExportAction::Start,
            "net-a",
            settings(),
        );
        assert_eq!(
            request,
            Ok(Request::Channel(
                "/api/devicesets/7/channels/3/network-export".to_owned(),
                ChannelNetworkExportRequest {
                    action: NetworkExportAction::Start,
                    node: "net-a".to_owned(),
                    settings: settings()
                }
            ))
        );
    }

    #[test]
    fn settings_lock_during_a_request_and_throughout_an_export() {
        assert!(!controls_locked(&Control::Ready, false));
        assert!(controls_locked(&Control::Ready, true));
        assert!(controls_locked(&Control::Active(status("net-a")), false));
    }

    #[test]
    fn an_action_without_a_live_source_is_refused() {
        let refused = request_for(None, NetworkExportAction::Start, "net-a", settings());
        assert!(refused.is_err_and(|said| said.starts_with("Wire a running device's IQ")));
    }
}
