mod actions;
mod bands;
pub mod colormap;
pub mod gpu;
mod history;
mod live;
mod menu;
mod overlay;
mod pick;
mod plot;
mod pointer;
mod radio;
mod ruler;
mod settings;
mod sheet;
mod trace;
mod traces;
mod view;

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU32, Ordering},
    },
};

use sdrmm_wire::{
    bandplan::{BandPlan, BandRegionsResponse},
    frame::{FrameKind, SpectrumFrame},
    rest::Bookmark,
    ws::{ClientCommand, ServerEvent, StateScope},
};
use zgui::{prelude::*, reactive::RenderEffect, surface::SurfaceElementExt};

use crate::{
    binding,
    bus::{Frame, Source},
    store::Store,
};

use colormap::Colormap;
use gpu::{WaterfallFeed, WaterfallSurface};
use live::{FrameMeta, Live, Settings};
use pick::ScopePick;
use radio::Lane;
use trace::{Inputs, TraceSurface};
use traces::{DEFAULT_AVERAGE, DbWindow, TraceMode};
use view::SpectrumView;

const SPECTRUM_FPS: u16 = 30;
const SPECTRUM_MAX_BINS: u16 = 4096;
const EMPTY_WINDOW: DbWindow = DbWindow {
    min: -100.0,
    max: -20.0,
};

static CHOSEN_COLORMAP: AtomicU8 = AtomicU8::new(0);
static CHOSEN_AVERAGE: AtomicU32 = AtomicU32::new(DEFAULT_AVERAGE);

#[must_use]
pub fn chosen_colormap() -> Colormap {
    Colormap::from_index(CHOSEN_COLORMAP.load(Ordering::Relaxed))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuAt {
    pub pick: ScopePick,
    pub x: f64,
    pub y: f64,
    pub stamp: (u64, u64, u64, u64),
}

#[derive(Clone, Copy)]
pub struct ScopeCx {
    pub store: Store,
    pub node: StoredValue<String>,
    pub lane: StoredValue<Lane>,
    pub meta: RwSignal<Option<FrameMeta>>,
    pub view: RwSignal<SpectrumView>,
    pub modes: RwSignal<Vec<TraceMode>>,
    pub phosphor: RwSignal<bool>,
    pub average: RwSignal<u32>,
    pub range: RwSignal<Option<DbWindow>>,
    pub colormap: RwSignal<Colormap>,
    pub fraction: RwSignal<f64>,
    pub preview: RwSignal<Option<(u32, f64)>>,
    pub panning: RwSignal<bool>,
    pub picked: RwSignal<Option<u32>>,
    pub menu: RwSignal<Option<MenuAt>>,
    pub picker: RwSignal<Option<MenuAt>>,
    pub settings_open: RwSignal<bool>,
    pub hover: RwSignal<Option<f64>>,
    pub readout: RwSignal<Option<String>>,
    pub plan: RwSignal<Option<Arc<BandPlan>>>,
    pub bookmarks: RwSignal<Arc<Vec<Bookmark>>>,
    pub plot: NodeRef,
    pub trace_box: NodeRef,
    pub live: StoredValue<Rc<RefCell<Live>>, LocalStorage>,
    pub feed: StoredValue<Rc<RefCell<WaterfallFeed>>, LocalStorage>,
    pub gesture: StoredValue<Option<pointer::Gesture>, LocalStorage>,
    pub grabbed: StoredValue<Option<u32>, LocalStorage>,
    pub last_click: StoredValue<Option<(f64, f64)>, LocalStorage>,
}

pub fn face(store: Store, node: String) -> impl IntoView {
    install_stylesheet("scope", sheet::SHEET);
    let lane = {
        let node = node.clone();
        Memo::new(move |_| lane_of(store, &node))
    };
    view! {
        column(class = "scope") {
            {move || match lane.get() {
                Some(lane) => AnyView::new(spectrum(store, node.clone(), lane)),
                None => AnyView::new(view! {
                    column(class = "scope__empty") { text(class = "hint") {"Wire an IQ output"} }
                }),
            }}
        }
    }
}

fn lane_of(store: Store, node: &str) -> Option<Lane> {
    let graph = store.graph.get();
    let (device, stream) = binding::iq_source_of(&graph, node)?;
    let set = store.device_set_of(&device)?;
    Some(Lane {
        set,
        stream,
        device,
    })
}

fn spectrum(store: Store, node: String, lane: Lane) -> impl IntoView {
    let key = (lane.set, lane.stream);
    let latest = live::lane_latest(key);
    let feed = Rc::new(RefCell::new(WaterfallFeed::default()));
    let cx = ScopeCx {
        store,
        node: StoredValue::new(node),
        lane: StoredValue::new(lane),
        meta: RwSignal::new(latest.map(|(meta, _)| meta)),
        view: RwSignal::new(SpectrumView::default()),
        modes: RwSignal::new(Vec::new()),
        phosphor: RwSignal::new(false),
        average: RwSignal::new(CHOSEN_AVERAGE.load(Ordering::Relaxed)),
        range: RwSignal::new(None),
        colormap: RwSignal::new(chosen_colormap()),
        fraction: RwSignal::new(0.32),
        preview: RwSignal::new(None),
        panning: RwSignal::new(false),
        picked: RwSignal::new(None),
        menu: RwSignal::new(None),
        picker: RwSignal::new(None),
        settings_open: RwSignal::new(false),
        hover: RwSignal::new(None),
        readout: RwSignal::new(None),
        plan: RwSignal::new(None),
        bookmarks: RwSignal::new(Arc::new(Vec::new())),
        plot: NodeRef::new(),
        trace_box: NodeRef::new(),
        live: StoredValue::new_local(Rc::new(RefCell::new(Live::seeded(latest)))),
        feed: StoredValue::new_local(Rc::clone(&feed)),
        gesture: StoredValue::new_local(None),
        grabbed: StoredValue::new_local(None),
        last_click: StoredValue::new_local(None),
    };
    live::reseed(&mut feed.borrow_mut(), key, cx.meta.get_untracked(), None);
    store.hold(ClientCommand::SubscribeSpectrum {
        device_set: key.0,
        fps: SPECTRUM_FPS,
        bins: SPECTRUM_MAX_BINS,
        stream: key.1,
    });
    listen(cx);
    follow_settings(cx);
    load_bookmarks(cx);
    load_band_plan(cx);
    actions::apply_creation_tunes(cx);
    layout(cx)
}

fn listen(cx: ScopeCx) {
    let (set, stream) = cx.lane.with_value(|lane| (lane.set, lane.stream));
    cx.store.on_frame(move |frame: &Frame| {
        if frame.kind != FrameKind::Spectrum
            || cx.store.source_of(frame.stream_id)
                != Some(Source::Spectrum {
                    device_set: set,
                    stream,
                })
        {
            return;
        }
        let Some(decoded) = SpectrumFrame::decode(&frame.bytes) else {
            return;
        };
        receive(cx, (set, stream), &decoded);
    });
}

fn receive(cx: ScopeCx, lane: (u32, u32), frame: &SpectrumFrame<'_>) {
    live::record(lane, frame);
    let settings = Settings {
        range: cx.range.get_untracked(),
        average: cx.average.get_untracked(),
        view: cx.view.get_untracked(),
        phosphor: cx.phosphor.get_untracked(),
        colormap: cx.colormap.get_untracked(),
    };
    let live = cx.live.get_value();
    let feed = cx.feed.get_value();
    let due = live
        .borrow_mut()
        .receive(frame, settings, &mut feed.borrow_mut(), || {
            live::lane_history(lane)
        });
    if due {
        cx.meta.set(Some(FrameMeta::of(frame)));
    }
    actions::update_readout(cx);
}

fn follow_settings(cx: ScopeCx) {
    let feed = cx.feed.get_value();
    let live = cx.live.get_value();
    let window = {
        let feed = Rc::clone(&feed);
        RenderEffect::new(move |_| {
            let view = cx.view.get();
            feed.borrow_mut().set_window(view.start, view.width());
        })
    };
    let colours = RenderEffect::new(move |_| {
        let colormap = cx.colormap.get();
        let phosphor = cx.phosphor.get();
        feed.borrow_mut().set_colormap(colormap);
        live.borrow_mut().sync_density(phosphor, colormap);
    });
    on_cleanup_local(move || {
        drop(window);
        drop(colours);
    });
}

fn load_bookmarks(cx: ScopeCx) {
    let fetch = move || {
        zgui::task::spawn_local(async move {
            match cx.store.api().get::<Vec<Bookmark>>("/api/bookmarks").await {
                Ok(listed) => cx.bookmarks.set(Arc::new(listed)),
                Err(error) => tracing::debug!(%error, "no bookmarks"),
            }
        });
    };
    fetch();
    cx.store.on_event(move |event: &ServerEvent| {
        if matches!(
            event,
            ServerEvent::StateChanged {
                scope: StateScope::Bookmarks | StateScope::All
            }
        ) {
            fetch();
        }
    });
}

thread_local! {
    static PLANS: RefCell<Vec<(String, Arc<BandPlan>)>> = const { RefCell::new(Vec::new()) };
    static DEFAULT_REGION: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn load_band_plan(cx: ScopeCx) {
    let chosen = Memo::new(move |_| {
        cx.store
            .settings
            .with(|settings| settings.band_region.clone())
    });
    let loading = RenderEffect::new(move |_| {
        let chosen = chosen.get();
        zgui::task::spawn_local(async move {
            let Some(region) = region_of(cx, chosen).await else {
                return;
            };
            let cached = PLANS.with(|plans| {
                plans
                    .borrow()
                    .iter()
                    .find(|(id, _)| *id == region)
                    .map(|(_, plan)| Arc::clone(plan))
            });
            if let Some(plan) = cached {
                cx.plan.set(Some(plan));
                return;
            }
            let path = format!("/api/bandplan/regions/{region}");
            match cx.store.api().get::<BandPlan>(&path).await {
                Ok(plan) => {
                    let plan = Arc::new(plan);
                    PLANS.with(|plans| plans.borrow_mut().push((region, Arc::clone(&plan))));
                    cx.plan.set(Some(plan));
                }
                Err(error) => tracing::debug!(%error, "no band plan"),
            }
        });
    });
    on_cleanup_local(move || drop(loading));
}

async fn region_of(cx: ScopeCx, chosen: Option<String>) -> Option<String> {
    if chosen.is_some() {
        return chosen;
    }
    if let Some(known) = DEFAULT_REGION.with(|region| region.borrow().clone()) {
        return Some(known);
    }
    match cx
        .store
        .api()
        .get::<BandRegionsResponse>("/api/bandplan/regions")
        .await
    {
        Ok(listed) => {
            DEFAULT_REGION
                .with(|region| *region.borrow_mut() = Some(listed.default_region.clone()));
            Some(listed.default_region)
        }
        Err(error) => {
            tracing::debug!(%error, "no band regions");
            None
        }
    }
}

impl ScopeCx {
    #[must_use]
    pub fn shown_window(self) -> DbWindow {
        self.range
            .get()
            .unwrap_or_else(|| self.meta.get().map_or(EMPTY_WINDOW, |meta| meta.window()))
    }

    #[must_use]
    pub fn active(self) -> bool {
        self.store.pane.get() == crate::store::Pane::Rack
            || self.store.selected.get().as_deref() == Some(self.node.get_value().as_str())
    }

    #[must_use]
    pub fn frame_stamp(self) -> (u64, u64, u64, u64) {
        let meta = self.meta.get();
        let view = self.view.get();
        (
            meta.map_or(0, |meta| meta.centre_hz.to_bits()),
            meta.map_or(0, |meta| meta.span_hz.to_bits()),
            view.start.to_bits(),
            view.end.to_bits(),
        )
    }

    pub fn choose_colormap(self, colormap: Colormap) {
        CHOSEN_COLORMAP.store(colormap.index() as u8, Ordering::Relaxed);
        self.colormap.set(colormap);
    }

    pub fn choose_average(self, frames: u32) {
        CHOSEN_AVERAGE.store(frames, Ordering::Relaxed);
        self.average.set(frames);
    }
}

fn layout(cx: ScopeCx) -> impl IntoView {
    let live = cx.live.get_value();
    let trace = TraceSurface::new(
        Rc::clone(&live),
        Inputs {
            view: cx.view.into(),
            window: Signal::derive(move || {
                cx.range
                    .get()
                    .unwrap_or_else(|| cx.meta.get().map_or(EMPTY_WINDOW, |meta| meta.window()))
            }),
            overlays: cx.modes.into(),
            hover: cx.hover,
        },
    );
    let waterfall = WaterfallSurface::new(cx.feed.get_value());
    let trace_height = move || Some(format!("{:.3}%", cx.fraction.get() * 100.0));
    view! {
        column(
            node_ref = cx.plot,
            class = "scope__plot",
            class:active = move || cx.active(),
            class:ruled = move || ruler::shown(cx),
            class:panning = move || cx.panning.get(),
            {..pointer::handlers(cx)}
        ) {
            {ruler::ruler(cx)}
            box(node_ref = cx.trace_box, class = "scope__trace", style:height = trace_height) {
                {zgui::elements::surface().class("scope__gpu").renderer(trace).into_view()}
                {overlay::trace_labels(cx)}
            }
            {pointer::divider(cx)}
            box(class = "scope__fall") {
                {zgui::elements::surface().class("scope__gpu").renderer(waterfall).into_view()}
                {overlay::bookmark_labels(cx)}
            }
            {overlay::bookmark_lines(cx)}
            {overlay::markers(cx)}
            {overlay::chrome(cx)}
            {menu::menu(cx)}
            {menu::picker(cx)}
        }
    }
}
