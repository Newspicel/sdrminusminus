use std::{cell::RefCell, rc::Rc, sync::Arc};

use sdrmm_wire::{frame::FrameKind, frame::SymbolPlane, ws::ClientCommand};
use zgui::prelude::*;

use crate::{
    binding,
    bus::Source,
    store::Store,
    ui::{
        kit_raster::{Bounds, number, raster_view},
        faces::scope::colormap::Colormap,
        widgets::{check, row_field, segments},
    },
};

pub mod frames;
pub mod grid;
pub mod measure;
pub mod scope;

use frames::{Block, Burst};
use grid::{Eye, SymbolState, samples_per_symbol, symbol_states};
use measure::{SIGNAL_VIEWS, SYMBOL_VIEWS, View, measurements, waiting};
use scope::{Anchor, Label, Scope, Settings};

const BURST_EVERY: u64 = 10;
const BLOCK_EVERY: u64 = 4;
const STATE_EXTENT: f32 = 1.1;

#[derive(Clone, Copy)]
struct Controls {
    view: RwSignal<View>,
    eye: RwSignal<Eye>,
    rate: RwSignal<f64>,
    decimate: RwSignal<bool>,
}

impl Controls {
    fn new() -> Self {
        let settings = Settings::default();
        Self {
            view: RwSignal::new(settings.view),
            eye: RwSignal::new(settings.eye),
            rate: RwSignal::new(f64::from(settings.symbol_rate)),
            decimate: RwSignal::new(settings.decimate),
        }
    }

    fn settings(self) -> Settings {
        Settings {
            view: self.view.get(),
            eye: self.eye.get(),
            symbol_rate: self.rate.get() as f32,
            decimate: self.decimate.get(),
        }
    }
}

#[derive(Clone, Copy)]
struct Seen {
    burst: RwSignal<Option<Arc<Burst>>>,
    block: RwSignal<Option<Arc<Block>>>,
    labels: RwSignal<Vec<Label>>,
    square: RwSignal<bool>,
}

impl Seen {
    fn refresh(self, scope: &Scope) {
        self.labels.set(scope.labels());
        if self.square.get_untracked() != scope.square() {
            self.square.set(scope.square());
        }
    }
}

pub fn face(store: Store, node: String) -> impl IntoView {
    install_stylesheet("wp11-baseband", SHEET);
    let tap = Memo::new(move |_| {
        binding::sources_of(&store.graph.get(), &node, "baseband")
            .into_iter()
            .find_map(|source| Some((store.device_set_of(&source)?, store.channel_of(&source)?.id)))
    });
    view! {
        column(class = "bb") {
            {move || match tap.get() {
                Some((set, channel)) => AnyView::new(watch(store, set, channel)),
                None => AnyView::new(view! { box(class = "bb__empty") { text(class = "hint") {"Wire a channel's baseband"} } }),
            }}
        }
    }
}

fn listen(store: Store, device_set: u32, channel: u32, scope: Rc<RefCell<Scope>>, seen: Seen) {
    store.hold(ClientCommand::SubscribeIq {
        device_set,
        channel,
    });
    store.hold(ClientCommand::SubscribeSymbols {
        device_set,
        channel,
    });
    let counts = Rc::new(RefCell::new((0u64, 0u64)));
    store.on_frame(move |frame| {
        let source = store.source_of(frame.stream_id);
        let Ok(mut held) = scope.try_borrow_mut() else {
            return;
        };
        let mut counts = counts.borrow_mut();
        match frame.kind {
            FrameKind::IqF32
                if source
                    == Some(Source::Iq {
                        device_set,
                        channel,
                    }) =>
            {
                let Some(burst) = Burst::read(&frame.bytes) else {
                    tracing::warn!(device_set, channel, "an IQ burst did not decode");
                    return;
                };
                counts.0 += 1;
                let publish = counts.0 == 1 || counts.0.is_multiple_of(BURST_EVERY);
                if publish {
                    seen.burst.set(Some(Arc::new(burst.clone())));
                }
                held.take_burst(burst);
                if publish {
                    seen.refresh(&held);
                }
            }
            FrameKind::Symbols
                if source
                    == Some(Source::Symbols {
                        device_set,
                        channel,
                    }) =>
            {
                let Some(block) = Block::read(&frame.bytes) else {
                    tracing::warn!(device_set, channel, "a symbol block did not decode");
                    return;
                };
                counts.1 += 1;
                let publish = counts.1 == 1 || counts.1.is_multiple_of(BLOCK_EVERY);
                if publish {
                    seen.block.set(Some(Arc::new(block.clone())));
                }
                held.take_block(block);
                if publish {
                    seen.refresh(&held);
                }
            }
            _ => {}
        }
    });
}

fn watch(store: Store, device_set: u32, channel: u32) -> impl IntoView {
    let controls = Controls::new();
    let seen = Seen {
        burst: RwSignal::new(None),
        block: RwSignal::new(None),
        labels: RwSignal::new(Vec::new()),
        square: RwSignal::new(false),
    };
    let scope = Rc::new(RefCell::new(Scope::new(Colormap::Viridis)));
    listen(store, device_set, channel, scope.clone(), seen);
    let configure = {
        let scope = scope.clone();
        zgui::reactive::RenderEffect::new(move |_| {
            let settings = controls.settings();
            if let Ok(mut held) = scope.try_borrow_mut() {
                held.configure(settings);
                seen.refresh(&held);
            }
        })
    };
    on_cleanup_local(move || drop(configure));
    let symbols = Signal::derive(move || seen.block.get().is_some());
    let shown = Signal::derive(move || controls.view.get().shown(symbols.get()));
    let rows = move || {
        let burst = seen.burst.get();
        let period = burst.as_ref().map_or(0.0, |burst| {
            samples_per_symbol(burst.sample_rate, controls.rate.get() as f32)
        });
        measurements(
            shown.get(),
            burst.as_deref(),
            seen.block.get().as_deref(),
            period,
        )
        .into_iter()
        .map(|row| {
            view! {
                row(class = "bb__pair") {
                    text(class = "bb__key") {{row.label}}
                    text(class = "bb__value") {{row.value}}
                }
            }
        })
        .collect::<Vec<_>>()
    };
    let hint = move || waiting(shown.get(), seen.burst.get().is_some(), symbols.get());
    view! {
        column(class = "bb__body") {
            {tabs(controls, shown, symbols)}
            {options(controls, shown, symbols, seen)}
            {plot(scope, seen, shown, hint)}
            row(class = "bb__measures") {{rows}}
        }
    }
}

fn tabs(controls: Controls, shown: Signal<View>, symbols: Signal<bool>) -> impl IntoView {
    let tab = move |view: View| {
        view! {
            control(
                class = "bb__tab",
                tabindex = Focus::Sequential,
                a11y:role = Role::Button,
                class:on = move || shown.get() == view,
                on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation(),
                on:click = move |_| controls.view.set(view)
            ) {
                {view.label()}
            }
        }
    };
    let signal_tabs: Vec<_> = SIGNAL_VIEWS.into_iter().map(tab).collect();
    view! {
        row(class = "bb__tabs", a11y:label = "Baseband view") {
            {signal_tabs}
            {move || symbols.get().then(|| {
                let symbol_tabs: Vec<_> = SYMBOL_VIEWS.into_iter().map(tab).collect();
                view! {
                    row(class = "bb__group") {
                        box(class = "bb__rule") {}
                        {symbol_tabs}
                    }
                }
            })}
        }
    }
}

fn options(
    controls: Controls,
    shown: Signal<View>,
    symbols: Signal<bool>,
    seen: Seen,
) -> impl IntoView {
    let nyquist = Signal::derive(move || {
        seen.burst
            .get()
            .map_or(f64::INFINITY, |burst| f64::from(burst.sample_rate) / 2.0)
    });
    let eye = move || {
        (shown.get() == View::Eye).then(|| {
            AnyView::new(row_field(
                "Trace",
                segments(
                    vec![(Eye::I, "I"), (Eye::Q, "Q"), (Eye::Frequency, "freq")],
                    controls.eye.into(),
                    move |chosen| controls.eye.set(chosen),
                ),
            ))
        })
    };
    let decimate = move || {
        shown.get().decimates(symbols.get()).then(|| {
            AnyView::new(row_field(
                "Symbols only",
                check(controls.decimate.into(), move |on| {
                    controls.decimate.set(on)
                }),
            ))
        })
    };
    let rate = move || {
        shown
            .get()
            .needs_rate(symbols.get(), controls.decimate.get())
            .then(|| {
                let top = nyquist.get();
                AnyView::new(row_field(
                    "Symbol rate Bd",
                    number(
                        "Symbol rate",
                        controls.rate.into(),
                        Bounds::new(1.0, if top.is_finite() { top } else { f64::MAX }),
                        move |value| controls.rate.set(value),
                    ),
                ))
            })
    };
    view! {
        column(class = "bb__options") {
            {eye}
            {decimate}
            {rate}
        }
    }
}

fn plot(
    scope: Rc<RefCell<Scope>>,
    seen: Seen,
    shown: Signal<View>,
    hint: impl Fn() -> Option<&'static str> + Copy + 'static,
) -> impl IntoView {
    let frame = NodeRef::new();
    let size = frame.observe_content_size();
    let square = move || {
        let size = size.get();
        let scale = frame.scale().max(0.1);
        let (width, height) = (size.width.0 / scale, size.height.0 / scale);
        if seen.square.get() {
            let side = width.min(height);
            ((width - side) / 2.0, (height - side) / 2.0, side, side)
        } else {
            (0.0, 0.0, width, height)
        }
    };
    let states = Memo::new(move |_| {
        seen.block
            .get()
            .map(|block| states_of(&block))
            .unwrap_or_default()
    });
    view! {
        box(class = "bb__plot", node_ref = frame) {
            {raster_view("bb__surface", scope)}
            box(
                class = "bb__labels",
                style:left = move || Some(format!("{:.1}px", square().0)),
                style:top = move || Some(format!("{:.1}px", square().1)),
                style:width = move || Some(format!("{:.1}px", square().2)),
                style:height = move || Some(format!("{:.1}px", square().3))
            ) {
                {move || seen.labels.get().into_iter().map(label_view).collect::<Vec<_>>()}
            }
            if move || shown.get() == View::States {
                column(class = "bb__states") {{states_view(states)}}
            }
            if move || hint().is_some() {
                text(class = "bb__hint") {{move || hint().unwrap_or_default()}}
            }
        }
    }
}

fn label_view(label: Label) -> AnyView {
    let top = format!("{:.2}%", label.y * 100.0);
    match label.anchor {
        Anchor::Left => AnyView::new(view! {
            text(class = "bb__label", style:left = Some(format!("{:.2}%", label.x * 100.0)), style:top = Some(top)) {{label.text}}
        }),
        Anchor::Right => AnyView::new(view! {
            text(class = "bb__label bb__label--right", style:right = Some(format!("{:.2}%", (1.0 - label.x) * 100.0)), style:top = Some(top)) {{label.text}}
        }),
        Anchor::Centre => AnyView::new(view! {
            text(class = "bb__label bb__label--centre", style:left = Some(format!("{:.2}%", label.x * 100.0)), style:top = Some(top)) {{label.text}}
        }),
    }
}

#[derive(Clone, Debug, PartialEq)]
struct StateRow {
    state: SymbolState,
    signed: bool,
}

fn states_of(block: &Block) -> Vec<StateRow> {
    let signed = block.plane != SymbolPlane::Complex;
    symbol_states(block)
        .into_iter()
        .map(|state| StateRow { state, signed })
        .collect()
}

#[must_use]
pub fn state_x(error: f32, signed: bool) -> f32 {
    let unit = if signed {
        (error / STATE_EXTENT + 1.0) / 2.0
    } else {
        error / STATE_EXTENT
    };
    unit.clamp(0.0, 1.0)
}

fn level(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

fn percent(value: f32, sign: bool) -> String {
    if !value.is_finite() {
        return "-".to_owned();
    }
    let shown = (value * 100.0).round();
    format!("{}{shown:.0}%", if sign && shown >= 0.0 { "+" } else { "" })
}

#[must_use]
pub fn numerics(state: &SymbolState) -> String {
    format!(
        "{:.0}%  {}  {}",
        state.share * 100.0,
        percent(state.mean, true),
        percent(state.sigma, false)
    )
}

fn states_view(states: Memo<Vec<StateRow>>) -> impl IntoView {
    move || {
        states
            .get()
            .into_iter()
            .map(|row| {
                let StateRow { state, signed } = row;
                let ideal = if signed {
                    level(state.i)
                } else {
                    format!("{},{}", level(state.i), level(state.q))
                };
                let track = if state.count == 0 {
                    AnyView::new(view! { text(class = "bb__never") {"never decided"} })
                } else {
                    let spread = state.sigma.is_finite().then(|| {
                        let low = state_x(state.mean - state.sigma, signed);
                        let high = state_x(state.mean + state.sigma, signed);
                        view! {
                            box(
                                class = "bb__spread",
                                style:left = Some(format!("{:.1}%", low * 100.0)),
                                style:width = Some(format!("{:.1}%", ((high - low) * 100.0).max(0.5)))
                            )
                        }
                    });
                    let mean = state_x(state.mean, signed);
                    let peak = state_x(state.peak, signed);
                    AnyView::new(view! {
                        box(class = "bb__track") {
                            box(class = "bb__ideal", style:left = Some(format!("{:.1}%", state_x(0.0, signed) * 100.0))) {}
                            {spread}
                            box(class = "bb__peak", style:left = Some(format!("{:.1}%", peak * 100.0)))
                            box(class = "bb__mean", style:left = Some(format!("{:.1}%", mean * 100.0)))
                        }
                    })
                };
                let figures = numerics(&state);
                view! {
                    row(class = "bb__state") {
                        text(class = "bb__bits") {{state.bits}}
                        text(class = "bb__level") {{ideal}}
                        {track}
                        text(class = "bb__figures") {{figures}}
                    }
                }
            })
            .collect::<Vec<_>>()
    }
}

const SHEET: &str = css!(
    r#"
.bb { flex-direction: column; background-color: #08090b; border-bottom-left-radius: 6px; border-bottom-right-radius: 6px; overflow: hidden; }
.bb__empty { height: 160px; align-items: center; justify-content: center; display: flex; }
.bb__body { flex-direction: column; }
.bb__tabs { flex-wrap: wrap; align-items: stretch; padding: 0 4px; border-bottom: 1px solid var(--line); background-color: var(--panel); }
.bb__group { align-items: stretch; }
.bb__rule { width: 1px; margin: 6px 4px; background-color: var(--line); }
.bb__tab { padding: 4px 7px; font-family: var(--mono); font-size: 10px; color: var(--ink-faint); border-bottom: 2px solid transparent; }
.bb__tab:hover { color: var(--ink); }
.bb__tab.on { color: var(--accent); border-bottom-color: var(--accent); }
.bb__options { gap: 4px; padding: 0 8px; background-color: var(--panel); }
.bb__options > * { margin: 4px 0; }
.bb__plot { position: relative; height: 240px; }
.bb__surface { position: absolute; left: 0; top: 0; width: 100%; height: 100%; }
.bb__labels { position: absolute; pointer-events: none; }
.bb__label {
    position: absolute;
    height: 12px;
    margin-top: -6px;
    padding: 0 2px;
    font-family: var(--mono);
    font-size: 9px;
    line-height: 12px;
    white-space: nowrap;
    color: var(--plot-ink-dim);
    background-color: rgba(8, 9, 11, 0.75);
}
.bb__label--centre { width: 60px; margin-left: -30px; text-align: center; background-color: transparent; }
.bb__hint {
    position: absolute;
    left: 0;
    right: 0;
    top: 50%;
    margin-top: -7px;
    text-align: center;
    font-family: var(--mono);
    font-size: 10px;
    color: var(--plot-ink-dim);
    pointer-events: none;
}
.bb__measures { flex-wrap: wrap; gap: 2px 14px; padding: 6px 10px; border-top: 1px solid var(--line); background-color: var(--panel); }
.bb__pair { gap: 6px; align-items: baseline; }
.bb__key { font-family: var(--mono); font-size: 9px; letter-spacing: 0.09em; text-transform: uppercase; color: var(--ink-faint); }
.bb__value { font-family: var(--mono); font-size: 11px; color: var(--ink); }
.bb__states { position: absolute; left: 0; top: 0; right: 0; bottom: 0; padding: 8px 0; overflow: auto; background-color: #08090b; }
.bb__state { position: relative; gap: 8px; align-items: center; padding: 2px 8px; background-color: #08090b; font-family: var(--mono); font-size: 10px; color: var(--plot-ink-dim); }
.bb__bits { width: 36px; flex: 0 0 auto; }
.bb__level { width: 56px; flex: 0 0 auto; text-align: right; }
.bb__figures { width: 110px; flex: 0 0 auto; white-space: nowrap; }
.bb__never { flex: 1 1 auto; }
.bb__track { position: relative; flex: 1 1 auto; height: 14px; border-top: 1px solid transparent; }
.bb__ideal { position: absolute; top: 0; bottom: 0; width: 1px; background-color: var(--plot-ink-dim); }
.bb__spread { position: absolute; top: 3px; height: 8px; background-color: rgba(102, 229, 255, 0.25); }
.bb__peak { position: absolute; top: 4px; width: 1px; height: 6px; background-color: rgba(240, 180, 80, 0.65); }
.bb__mean { position: absolute; top: 0; width: 2px; height: 14px; background-color: rgb(102, 229, 255); }
"#
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_error_lands_between_the_slices() {
        assert!((state_x(0.0, true) - 0.5).abs() < 1e-6);
        assert!((state_x(STATE_EXTENT, true) - 1.0).abs() < 1e-6);
        assert_eq!(state_x(-5.0, true), 0.0);
        assert_eq!(state_x(0.0, false), 0.0);
        assert_eq!(state_x(5.0, false), 1.0);
    }

    #[test]
    fn a_state_reads_its_share_offset_and_spread() {
        let state = SymbolState {
            bits: "10".into(),
            i: 1.0,
            q: 0.0,
            count: 2,
            share: 0.25,
            mean: 0.12,
            sigma: f32::NAN,
            peak: 0.2,
        };
        assert_eq!(numerics(&state), "25%  +12%  -");
    }
}
