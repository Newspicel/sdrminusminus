use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
    sync::Arc,
};

use zgui::reactive::ArcRwSignal;
use zgui::reactive::RenderEffect;
use zgui::{elements::CanvasHandle, prelude::*};

use super::{
    geo::{TILE_PX, TileId, View},
    paint::{EXTENT, LabelClass, Painted},
    source::{Basemap, Cache, Kind, Net, OFFLINE_PATH, Slot},
};
use crate::store::Store;

const CACHED_TILES: usize = 384;
const PARENTS_TRIED: u8 = 5;
const PLACES_SHOWN: usize = 28;
const PLACE_GAP_X: f64 = 90.0;
const PLACE_GAP_Y: f64 = 18.0;

pub struct Shared {
    basemap: ArcRwSignal<Option<Basemap>>,
    arrivals: ArcRwSignal<u64>,
    failed: ArcRwSignal<u32>,
    cache: RefCell<Cache>,
    net: Option<Net>,
    asked: Cell<bool>,
}

thread_local! {
    static SHARED: Rc<Shared> = Rc::new(Shared::new());
}

impl Shared {
    fn new() -> Self {
        let net = match Net::new() {
            Ok(net) => Some(net),
            Err(error) => {
                tracing::warn!(%error, "the map cannot fetch tiles");
                None
            }
        };
        Self {
            basemap: ArcRwSignal::new(None),
            arrivals: ArcRwSignal::new(0),
            failed: ArcRwSignal::new(0),
            cache: RefCell::new(Cache::new(CACHED_TILES)),
            net,
            asked: Cell::new(false),
        }
    }

    pub fn get() -> Rc<Self> {
        SHARED.with(Rc::clone)
    }

    pub fn discover(self: &Rc<Self>, store: Store) {
        if self.asked.replace(true) {
            return;
        }
        let Some(net) = self.net.clone() else {
            self.basemap.set(Some(Basemap::Blank));
            store.say("the map cannot reach the network");
            return;
        };
        let url = store.api().url(OFFLINE_PATH);
        let basemap = self.basemap.clone();
        zgui::task::spawn_detached(async move {
            let found = zgui::task::background(async move { net.discover(url).await }).await;
            basemap.set(Some(found));
        });
    }

    pub fn kind(&self) -> Kind {
        self.basemap
            .get()
            .as_ref()
            .map_or(Kind::Pending, Basemap::kind)
    }

    pub fn failures(&self) -> u32 {
        self.failed.get()
    }

    pub fn credit(&self) -> &'static str {
        self.basemap.get().as_ref().map_or("", Basemap::attribution)
    }

    fn request(self: &Rc<Self>, basemap: &Basemap, id: TileId) {
        let Some(net) = self.net.clone() else {
            return;
        };
        if !self.cache.borrow_mut().claim(basemap.key(), id) {
            return;
        }
        let shared = self.clone();
        let basemap = basemap.clone();
        zgui::task::spawn_detached(async move {
            let source = basemap.key().to_owned();
            let fetched = zgui::task::background(async move { net.tile(&basemap, id).await }).await;
            let slot = match fetched {
                Ok(painted) => Slot::Ready(Arc::new(painted)),
                Err(error) => {
                    tracing::debug!(%error, ?id, "a map tile failed");
                    shared.failed.update(|count| *count += 1);
                    Slot::Failed
                }
            };
            shared.cache.borrow_mut().settle(&source, id, slot);
            shared.arrivals.update(|count| *count += 1);
        });
    }

    fn ready(&self, source: &str, id: TileId) -> Option<Arc<Painted>> {
        self.cache.borrow().ready(source, id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Drawn {
    key: String,
    id: TileId,
    column: i64,
}

fn place(view: &View, id: TileId, column: i64) -> (f64, f64, f64) {
    let world = view.world();
    let size = world / f64::from(1u32 << id.z);
    let left = view.width / 2.0 - view.x * world + column as f64 * size;
    let top = view.height / 2.0 - view.y * world + f64::from(id.y) * size;
    (left, top, size)
}

fn drawn(shared: &Shared, basemap: &Basemap, view: &View) -> Vec<Drawn> {
    let source = basemap.key();
    let mut children = Vec::new();
    let mut parents = Vec::new();
    let mut seen = HashSet::new();
    for spot in view.tiles(basemap.max_zoom()) {
        if shared.ready(source, spot.id).is_some() {
            children.push(Drawn {
                key: format!("{}/{}/{}", spot.id.z, spot.column, spot.id.y),
                id: spot.id,
                column: spot.column,
            });
            continue;
        }
        let (mut id, mut column) = (spot.id, spot.column);
        for _ in 0..PARENTS_TRIED {
            let Some(parent) = id.parent() else {
                break;
            };
            id = parent;
            column = column.div_euclid(2);
            if shared.ready(source, id).is_some() {
                let key = format!("{}/{}/{}", id.z, column, id.y);
                if seen.insert(key.clone()) {
                    parents.push(Drawn { key, id, column });
                }
                break;
            }
        }
    }
    parents.sort_by_key(|tile| tile.id.z);
    parents.extend(children);
    parents
}

fn current<T>(
    shared: &Shared,
    view: RwSignal<View>,
    read: fn(&Shared, &Basemap, &View) -> Vec<T>,
) -> Vec<T> {
    shared.arrivals.with(|_| ());
    shared
        .basemap
        .get()
        .map(|basemap| read(shared, &basemap, &view.get()))
        .unwrap_or_default()
}

pub fn layer(shared: Rc<Shared>, view: RwSignal<View>) -> impl IntoView {
    let fetching = {
        let shared = shared.clone();
        RenderEffect::new(move |_| {
            let Some(basemap) = shared.basemap.get() else {
                return;
            };
            let current = view.get();
            for spot in current.tiles(basemap.max_zoom()) {
                shared.request(&basemap, spot.id);
            }
        })
    };
    on_cleanup_local(move || drop(fetching));

    let tiles = shared.clone();
    let places = shared.clone();

    let row = move |tile: Drawn| {
        let source = shared
            .basemap
            .get_untracked()
            .map(|basemap| basemap.key().to_owned())
            .unwrap_or_default();
        let handle = CanvasHandle::new();
        if let Some(painted) = shared.ready(&source, tile.id) {
            handle.draw(|scene| scene.replace(painted.shapes.clone()));
        }
        let (id, column) = (tile.id, tile.column);
        zgui::elements::canvas()
            .class("map__tile")
            .scene(&handle)
            .view_box(0.0, 0.0, EXTENT as f32, EXTENT as f32)
            .style_property("transform", move || {
                let (left, top, size) = place(&view.get(), id, column);
                Some(format!(
                    "translate({left:.2}px, {top:.2}px) scale({:.5})",
                    size / TILE_PX
                ))
            })
    };

    view! {
        box(class = "map__layer") {
            for tile in move || current(&tiles, view, drawn), key = |tile: &Drawn| tile.key.clone() {
                {row(tile)}
            }
        }
        box(class = "map__layer") {
            for spot in move || current(&places, view, places_of), key = |spot: &Place| spot.key.clone() {
                {place_label(spot, view)}
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Place {
    key: String,
    text: String,
    class: LabelClass,
    x: f64,
    y: f64,
}

fn places_of(shared: &Shared, basemap: &Basemap, view: &View) -> Vec<Place> {
    let source = basemap.key();
    let mut candidates: Vec<Place> = Vec::new();
    let mut names = HashSet::new();
    let mut ranked: Vec<(i64, Place)> = Vec::new();
    for spot in view.tiles(basemap.max_zoom()) {
        let Some(painted) = shared.ready(source, spot.id) else {
            continue;
        };
        let shift = (spot.column - i64::from(spot.id.x)) as f64 / f64::from(1u32 << spot.id.z);
        for label in &painted.labels {
            ranked.push((
                label.score,
                Place {
                    key: format!("{}:{:.5}:{:.5}", label.text, label.x, label.y),
                    text: label.text.clone(),
                    class: label.class,
                    x: label.x + shift,
                    y: label.y,
                },
            ));
        }
    }
    ranked.sort_by_key(|(score, _)| *score);
    for (_, place) in ranked {
        let (sx, sy) = view.screen_of_unit(place.x, place.y);
        let inside = (0.0..view.width).contains(&sx) && (0.0..view.height).contains(&sy);
        let crowded = candidates.iter().any(|kept| {
            let (kx, ky) = view.screen_of_unit(kept.x, kept.y);
            (kx - sx).abs() < PLACE_GAP_X && (ky - sy).abs() < PLACE_GAP_Y
        });
        if inside && !crowded && names.insert(place.text.clone()) {
            candidates.push(place);
            if candidates.len() >= PLACES_SHOWN {
                break;
            }
        }
    }
    candidates
}

fn place_label(spot: Place, view: RwSignal<View>) -> impl IntoView {
    let (x, y) = (spot.x, spot.y);
    let at = move || view.with(|view| view.screen_of_unit(x, y));
    view! {
        text(
            class = "map__place",
            attr:data-class = spot.class.css(),
            style:left = move || Some(format!("{:.1}px", at().0)),
            style:top = move || Some(format!("{:.1}px", at().1))
        ) {{spot.text}}
    }
}
