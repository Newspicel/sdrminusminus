pub mod analysis;
pub mod calibration;
pub mod chart;
pub mod export;
pub mod readout;
pub mod rf;
#[cfg(test)]
mod testdata;
pub mod traces;

use sdrmm_wire::tools::{
    NanoVnaCalibration, NanoVnaDeviceReport, NanoVnaMatch, NanoVnaSweep, NanoVnaSweepRequest,
    NanoVnaSweepState, ToolResponse,
};
use zgui::prelude::*;
use zgui_ui::prelude::*;

use self::{
    analysis::{analyse, readouts},
    calibration::Calibrate,
    chart::{SweepInputs, smith_chart, sweep_chart},
    export::{FORMATS, Format, export_filename, sweep_csv, touchstone_s1p, touchstone_s2p},
    readout::{device_report, marker_readout, sweep_summary},
    rf::{
        describe_request, devices_of, devices_request, ignored_ports_of, lowest_vswr_index,
        report_of, sweep_of, sweep_request,
    },
    traces::{CHART_VIEWS, ChartId},
};
use crate::{
    store::Store,
    ui::{
        tools::kit::{Query, alert, button, chip, format_hz, labelled, number, run_tool, save_for},
        widgets::{pick, segments, slide},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Measure,
    Calibrate,
    Device,
}

const RANGE_PRESETS: [(&str, f64, f64); 5] = [
    ("HF", 0.05, 30.0),
    ("6 m", 50.0, 54.0),
    ("2 m", 144.0, 148.0),
    ("70 cm", 430.0, 440.0),
    ("Full", 0.05, 900.0),
];

#[must_use]
pub fn sweep_state(start_mhz: f64, stop_mhz: f64, points: f64) -> NanoVnaSweepState {
    NanoVnaSweepState {
        start_hz: (start_mhz * 1e6).round() as u64,
        stop_hz: (stop_mhz * 1e6).round() as u64,
        points: points.round() as u32,
    }
}

#[derive(Clone, Copy)]
struct Vna {
    store: Store,
    port: RwSignal<String, LocalStorage>,
    start: RwSignal<f64>,
    stop: RwSignal<f64>,
    points: RwSignal<f64>,
    averages: RwSignal<f64>,
    devices: Query<ToolResponse>,
    sweep: Query<ToolResponse>,
    describe: Query<ToolResponse>,
    effective: Memo<String>,
    range: Memo<NanoVnaSweepState>,
}

impl Vna {
    fn scan(self) {
        let store = self.store;
        self.devices.run(run_tool(store, devices_request()));
    }

    fn acquire(self) {
        let range = self.range.get_untracked();
        let request = NanoVnaSweepRequest {
            port: self.effective.get_untracked(),
            start_hz: range.start_hz,
            stop_hz: range.stop_hz,
            points: range.points,
            averages: self.averages.get_untracked().round() as u16,
        };
        self.sweep.run(run_tool(self.store, sweep_request(request)));
    }

    fn read_device(self) {
        let port = self.effective.get_untracked();
        if port.is_empty() {
            return;
        }
        self.describe
            .run(run_tool(self.store, describe_request(&port)));
    }
}

pub fn panel(store: Store) -> impl IntoView {
    let port = RwSignal::new_local(String::new());
    let start = RwSignal::new(1.0);
    let stop = RwSignal::new(30.0);
    let points = RwSignal::new(101.0);
    let devices: Query<ToolResponse> = Query::new();
    let effective = Memo::new(move |_| {
        let typed = port.get();
        if typed.is_empty() {
            devices.data.with(|data| {
                devices_of(data.as_ref())
                    .first()
                    .map(|device| device.port.clone())
                    .unwrap_or_default()
            })
        } else {
            typed
        }
    });
    let vna = Vna {
        store,
        port,
        start,
        stop,
        points,
        averages: RwSignal::new(1.0),
        devices,
        sweep: Query::new(),
        describe: Query::new(),
        effective,
        range: Memo::new(move |_| sweep_state(start.get(), stop.get(), points.get())),
    };
    vna.scan();
    let tab = RwSignal::new(Tab::Measure);
    let described = zgui::reactive::RenderEffect::new(move |_| {
        if tab.get() != Tab::Measure && !effective.get().is_empty() {
            vna.read_device();
        }
    });
    on_cleanup_local(move || drop(described));
    let sweep = Memo::new(move |_| vna.sweep.data.with(|data| sweep_of(data.as_ref()).cloned()));
    let report = Memo::new(move |_| {
        vna.describe
            .data
            .with(|data| report_of(data.as_ref()).cloned())
            .or_else(|| sweep.with(|sweep| sweep.as_ref().map(|sweep| sweep.device.clone())))
    });
    let calibration = RwSignal::new(None::<NanoVnaCalibration>);
    let seed = zgui::reactive::RenderEffect::new(move |_| {
        let known = report.with(|report| report.as_ref().map(|report| report.calibration.clone()));
        if calibration.get_untracked().is_none() && known.is_some() {
            calibration.set(known);
        }
    });
    on_cleanup_local(move || drop(seed));
    let chart = RwSignal::new(ChartId::Magnitude);
    let format = RwSignal::new(Format::Ri);

    view! {
        column(class = "tool-stack") {
            {controls(vna, sweep)}
            {presets(vna)}
            {device_bar(vna)}
            {segments(vec![(Tab::Measure, "Measure"), (Tab::Calibrate, "Calibrate"), (Tab::Device, "Device")], tab.into(), move |picked| tab.set(picked))}
            {alert(move || vna.sweep.error.get())}
            {alert(move || if tab.get() == Tab::Measure { None } else { vna.describe.error.get() })}
            {move || match tab.get() {
                Tab::Measure => measure(vna, sweep, chart, format),
                Tab::Calibrate => AnyView::new(calibration::panel(Calibrate { store, port: effective, range: vna.range, state: calibration })),
                Tab::Device => device_tab(vna, report),
            }}
        }
    }
}

fn controls(vna: Vna, sweep: Memo<Option<NanoVnaSweep>>) -> impl IntoView {
    let placeholder = move || {
        vna.devices.data.with(|data| {
            devices_of(data.as_ref()).first().map_or_else(
                || "no NanoVNA found".to_owned(),
                |device| device.port.clone(),
            )
        })
    };
    let label = move || {
        if vna.sweep.busy.get() {
            "Sweeping\u{2026}".to_owned()
        } else if sweep.with(Option::is_none) {
            "Sweep".to_owned()
        } else {
            "Sweep again".to_owned()
        }
    };
    let blocked = move || {
        vna.effective.with(String::is_empty)
            || vna.sweep.busy.get()
            || vna.stop.get() <= vna.start.get()
    };
    view! {
        row(class = "tool-row") {
            {labelled("Instrument", view! {
                row(class = "tool-bar") {
                    {move || {
                        let hint = placeholder();
                        view! { box(class = "tool-text wide") { Input(class = "native-input", value = vna.port, label = "NanoVNA serial port", placeholder = hint) } }
                    }}
                    {button("btn", move || if vna.devices.busy.get() { "Scanning\u{2026}".to_owned() } else { "Rescan".to_owned() }, move || vna.devices.busy.get(), move || vna.scan())}
                }
            })}
            {labelled("Start", number("Sweep start", vna.start, 0.01, 6300.0, "MHz"))}
            {labelled("Stop", number("Sweep stop", vna.stop, 0.01, 6300.0, "MHz"))}
            {labelled("Points", number("Sweep points", vna.points, 11.0, 10_001.0, ""))}
            {labelled("Averages", number("Sweep averages", vna.averages, 1.0, 16.0, ""))}
            {button("btn primary", label, blocked, move || vna.acquire())}
        }
    }
}

fn presets(vna: Vna) -> impl IntoView {
    let chips: Vec<AnyView> = RANGE_PRESETS
        .iter()
        .map(|(label, start, stop)| {
            let (start, stop) = (*start, *stop);
            AnyView::new(button(
                "tool-chip",
                move || (*label).to_owned(),
                || false,
                move || {
                    vna.start.set(start);
                    vna.stop.set(stop);
                },
            ))
        })
        .collect();
    view! {
        row(class = "tool-bar") {
            text(class = "legend") {"Range"}
            {chips}
        }
    }
}

fn device_bar(vna: Vna) -> impl IntoView {
    move || {
        if let Some(error) = vna.devices.error.get() {
            return AnyView::new(view! { text(class = "tool-alert") {{error}} });
        }
        if vna.devices.busy.get() && vna.devices.data.with(Option::is_none) {
            return AnyView::new(
                view! { text(class = "tool-dim") {"Looking for a NanoVNA\u{2026}"} },
            );
        }
        let (found, ignored) = vna.devices.data.with(|data| {
            (
                devices_of(data.as_ref()).to_vec(),
                ignored_ports_of(data.as_ref()).len(),
            )
        });
        let selected = vna.effective.get();
        let mut items: Vec<AnyView> = if found.is_empty() {
            vec![AnyView::new(
                view! { text(class = "tool-dim") {"No NanoVNA found. Connect one and rescan, or type its port."} },
            )]
        } else {
            found
                .into_iter()
                .map(|device| {
                    let port = device.port.clone();
                    let on = device.port == selected;
                    let unconfirmed = (device.match_kind == NanoVnaMatch::Probable)
                        .then(|| view! { text(class = "tool-faint") {"unconfirmed"} });
                    AnyView::new(view! {
                        control(
                            class = "tool-chip",
                            class:on = on,
                            a11y:role = Role::Button,
                            tabindex = Focus::Sequential,
                            on:click:stop = move |_| vna.port.set(port.clone())
                        ) {
                            text {{device.label.clone()}}
                            {unconfirmed}
                        }
                    })
                })
                .collect()
        };
        if ignored > 0 {
            let noun = if ignored == 1 { "port" } else { "ports" };
            items.push(AnyView::new(view! { text(class = "tool-faint") {{format!("{ignored} other serial {noun} ignored")}} }));
        }
        AnyView::new(view! { row(class = "tool-bar") {{items}} })
    }
}

fn device_tab(vna: Vna, report: Memo<Option<NanoVnaDeviceReport>>) -> AnyView {
    AnyView::new(move || {
        let Some(report) = report.get() else {
            let hint = if vna.describe.busy.get() {
                "Reading the instrument\u{2026}"
            } else {
                "No instrument selected."
            };
            return AnyView::new(view! { text(class = "tool-dim") {{hint}} });
        };
        AnyView::new(view! {
            column(class = "tool-stack") {
                row(class = "tool-bar") {
                    {button("btn", move || if vna.describe.busy.get() { "Reading\u{2026}".to_owned() } else { "Re-read the instrument".to_owned() }, move || vna.describe.busy.get(), move || vna.read_device())}
                }
                {device_report(&report)}
            }
        })
    })
}

fn measure(
    vna: Vna,
    sweep: Memo<Option<NanoVnaSweep>>,
    chart: RwSignal<ChartId>,
    format: RwSignal<Format>,
) -> AnyView {
    AnyView::new(move || {
        let Some(current) = sweep.get() else {
            let hint = if vna.effective.with(String::is_empty) {
                "Connect a NanoVNA and rescan."
            } else {
                "Sweep to measure S11 and S21 across the range above."
            };
            return AnyView::new(view! { text(class = "tool-dim") {{hint}} });
        };
        if current.points.is_empty() {
            return AnyView::new(
                view! { text(class = "tool-alert") {"The NanoVNA returned an empty sweep."} },
            );
        }
        AnyView::new(sweep_view(vna.store, current, chart, format))
    })
}

#[must_use]
pub fn visible_range(zoom: Option<(usize, usize)>, count: usize) -> (usize, usize) {
    let last = count.saturating_sub(1);
    zoom.map_or((0, last), |(from, to)| {
        (from.min(last), to.min(last).max(from.min(last)))
    })
}

fn sweep_view(
    store: Store,
    sweep: NanoVnaSweep,
    chart: RwSignal<ChartId>,
    format: RwSignal<Format>,
) -> impl IntoView {
    let all = StoredValue::new(readouts(&sweep.points));
    let analysis = analyse(&sweep.points);
    let resonance = lowest_vswr_index(&sweep.points);
    let zoom = RwSignal::new(None::<(usize, usize)>);
    let marker = RwSignal::new(None::<usize>);
    let visible = Memo::new(move |_| {
        let (from, to) = visible_range(zoom.get(), all.with_value(Vec::len));
        all.with_value(|rows| rows.get(from..=to).map(<[_]>::to_vec).unwrap_or_default())
    });
    let active = Memo::new(move |_| {
        let offset = zoom.get().map_or(0, |(from, _)| from);
        let fallback = resonance.saturating_sub(offset);
        let last = visible.with(Vec::len).saturating_sub(1);
        marker.get().unwrap_or(fallback).min(last)
    });
    let on_marker = UnsyncCallback::new(move |index| marker.set(Some(index)));
    let on_zoom = UnsyncCallback::new(move |(from, to): (usize, usize)| {
        zoom.update(|zoom| {
            let base = zoom.map_or(0, |(start, _)| start);
            *zoom = Some((base + from, base + to));
        });
    });
    let chart_options: Vec<(ChartId, &'static str)> = CHART_VIEWS
        .iter()
        .map(|view| (view.id, view.label))
        .collect();
    let formats: Vec<(Format, String)> = FORMATS
        .iter()
        .map(|(format, label)| (*format, (*label).to_owned()))
        .collect();
    let figure = move || {
        if chart.get() == ChartId::Smith {
            AnyView::new(smith_chart(visible, active, on_marker))
        } else {
            AnyView::new(sweep_chart(SweepInputs {
                rows: visible,
                chart,
                marker: active,
                on_marker,
                on_zoom,
            }))
        }
    };
    let slider = move || {
        let last = visible.with(Vec::len).saturating_sub(1).max(1) as f64;
        let read = move |value: f64| {
            visible
                .with(|rows| {
                    rows.get(value.round() as usize)
                        .map(|row| format_hz(row.frequency_hz))
                })
                .unwrap_or_default()
        };
        slide(
            Signal::derive(move || active.get() as f64),
            0.0,
            last,
            read,
            move |value| marker.set(Some(value.round() as usize)),
        )
    };
    let readout = move || {
        visible.with(|rows| {
            rows.get(active.get())
                .map(|row| AnyView::new(marker_readout(row)))
        })
    };
    let chips = sweep_chips(&sweep);
    let exported = StoredValue::new(sweep);
    let save = move |extension: &'static str| {
        exported.with_value(|sweep| {
            let at = jiff::Timestamp::now().to_string();
            let text = match extension {
                "s2p" => touchstone_s2p(sweep, format.get_untracked(), Some(&at)),
                "s1p" => touchstone_s1p(sweep, format.get_untracked(), Some(&at)),
                _ => sweep_csv(sweep),
            };
            save_for(store, &export_filename(sweep, extension), &text);
        });
    };
    view! {
        column(class = "tool-stack") {
            row(class = "tool-chips") {{chips}}
            row(class = "tool-bar") {
                {segments(chart_options, chart.into(), move |picked| chart.set(picked))}
                spacer() {}
                {move || zoom.get().is_some().then(|| AnyView::new(button("btn", || "Reset zoom".to_owned(), || false, move || zoom.set(None))))}
                text(class = "tool-faint tool-mono") {"drag: marker \u{b7} shift-drag: zoom"}
            }
            {figure}
            row(class = "vna-marker") {
                text(class = "legend") {"Marker"}
                {slider}
            }
            {readout}
            {sweep_summary(&analysis)}
            row(class = "vna-export") {
                {labelled("Touchstone format", pick(formats, Signal::derive(move || Some(format.get())), move |picked| format.set(picked)))}
                {button("btn", || "Export .s2p".to_owned(), || false, move || save("s2p"))}
                {button("btn", || "Export .s1p".to_owned(), || false, move || save("s1p"))}
                {button("btn", || "Export CSV".to_owned(), || false, move || save("csv"))}
            }
        }
    }
}

fn sweep_chips(sweep: &NanoVnaSweep) -> Vec<AnyView> {
    let calibration = &sweep.device.calibration;
    let mut chips = vec![
        chip("points", sweep.points.len().to_string()),
        chip("averages", sweep.averages.to_string()),
        chip("took", format!("{:.1} s", sweep.elapsed_ms as f64 / 1000.0)),
        chip("correction", if calibration.applied { "on" } else { "off" }),
    ];
    if let Some(bandwidth) = sweep.device.bandwidth_hz {
        chips.push(chip("IF", format_hz(f64::from(bandwidth))));
    }
    chips
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_in_megahertz_becomes_whole_hertz_and_points() {
        assert_eq!(
            sweep_state(0.05, 30.0, 101.4),
            NanoVnaSweepState {
                start_hz: 50_000,
                stop_hz: 30_000_000,
                points: 101
            }
        );
    }

    #[test]
    fn a_zoom_stays_inside_the_sweep() {
        assert_eq!(visible_range(None, 101), (0, 100));
        assert_eq!(visible_range(Some((20, 60)), 101), (20, 60));
        assert_eq!(visible_range(Some((90, 400)), 101), (90, 100));
        assert_eq!(visible_range(Some((200, 400)), 101), (100, 100));
        assert_eq!(visible_range(None, 0), (0, 0));
    }
}
