use std::{sync::Arc, time::Duration};

use sdrmm_wire::{
    rest::{DecoderLogEntry, DecoderLogResponse, DeletedCount},
    ws::{ServerEvent, StateScope},
};
use zgui::{prelude::*, reactive::RenderEffect, view::TimeoutHandle};

use crate::{
    decoders::{
        log::{
            ColumnWidths, LIVE_ROW_CAP, LogFilter, LogRow, build_rows, collect_live, query_string,
        },
        wiring::event_sources_of,
    },
    store::Store,
};

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(250);
const CLEAR_ARM: Duration = Duration::from_secs(3);
const WIDTHS_FILE: &str = "decoder-log-columns.json";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub entries: Arc<Vec<Arc<DecoderLogEntry>>>,
    pub total: u64,
    pub dropped: u64,
}

#[derive(Clone, Copy)]
pub struct Log {
    pub store: Store,
    pub sink: StoredValue<String>,
    pub filter: RwSignal<LogFilter>,
    pub search: RwSignal<String, LocalStorage>,
    pub page: RwSignal<Option<Page>>,
    pub unavailable: RwSignal<Option<String>>,
    pub rejected: RwSignal<Option<String>>,
    pub armed: RwSignal<bool>,
    pub cleared: RwSignal<Option<u64>>,
    pub opened: RwSignal<Option<String>>,
    pub widths: RwSignal<ColumnWidths>,
    pub rows: RwSignal<Arc<Vec<LogRow>>>,
    revision: RwSignal<u64>,
    asked: StoredValue<u64>,
    clock: StoredValue<Option<Timers>, LocalStorage>,
    pending: StoredValue<Option<TimeoutHandle>, LocalStorage>,
    disarm: StoredValue<Option<TimeoutHandle>, LocalStorage>,
}

impl Log {
    pub fn new(store: Store, sink: String) -> Self {
        let log = Self {
            store,
            sink: StoredValue::new(sink),
            filter: RwSignal::new(LogFilter::default()),
            search: RwSignal::new_local(String::new()),
            page: RwSignal::new(None),
            unavailable: RwSignal::new(None),
            rejected: RwSignal::new(None),
            armed: RwSignal::new(false),
            cleared: RwSignal::new(None),
            opened: RwSignal::new(None),
            widths: RwSignal::new(load_widths()),
            rows: RwSignal::new(Arc::new(Vec::new())),
            revision: RwSignal::new(0),
            asked: StoredValue::new(0),
            clock: StoredValue::new_local(Timers::current()),
            pending: StoredValue::new_local(None),
            disarm: StoredValue::new_local(None),
        };
        log.watch();
        log
    }

    fn watch(self) {
        let fetch = RenderEffect::new(move |_| {
            let query = self.filter.get().query(&self.sink.get_value());
            self.revision.track();
            self.fetch(query_string(&query));
        });
        let debounce = RenderEffect::new(move |_| {
            let typed = self.search.get();
            if typed != self.filter.get_untracked().q {
                self.debounce(typed);
            }
        });
        let rows = RenderEffect::new(move |_| self.rows.set(Arc::new(self.build())));
        on_cleanup_local(move || drop((fetch, debounce, rows)));
        self.store.on_event(move |event: &ServerEvent| {
            if matches!(
                event,
                ServerEvent::StateChanged {
                    scope: StateScope::DecoderLog
                } | ServerEvent::Hello { .. }
            ) {
                self.revision.update(|n| *n += 1);
            }
        });
    }

    fn build(self) -> Vec<LogRow> {
        let sink = self.sink.get_value();
        let filter = self.filter.get();
        let wired = !event_sources_of(&self.store.graph.get(), &sink).is_empty();
        let live = if wired {
            self.store
                .decoded
                .with(|decoded| collect_live(decoded, &filter, &sink, LIVE_ROW_CAP))
        } else {
            Vec::new()
        };
        let entries = self
            .page
            .with(|page| page.as_ref().map(|page| Arc::clone(&page.entries)))
            .unwrap_or_default();
        build_rows(&entries, live)
    }

    fn debounce(self, typed: String) {
        let Some(clock) = self.clock.get_value() else {
            self.set_search(typed);
            return;
        };
        let handle = clock.set_timeout(SEARCH_DEBOUNCE, move || self.set_search(typed));
        self.pending.set_value(Some(handle));
    }

    fn set_search(self, q: String) {
        self.cleared.set(None);
        self.filter.update(|filter| filter.q = q);
    }

    pub fn set_limit(self, limit: u32) {
        self.cleared.set(None);
        self.filter.update(|filter| filter.limit = limit);
    }

    fn fetch(self, query: String) {
        let asked = self.asked.get_value() + 1;
        self.asked.set_value(asked);
        let store = self.store;
        zgui::task::spawn_local(async move {
            let answer = store
                .api()
                .get::<DecoderLogResponse>(&format!("/api/decoderlog?{query}"))
                .await;
            if self.asked.get_value() != asked {
                return;
            }
            match answer {
                Ok(response) => {
                    self.unavailable.set(None);
                    self.page.set(Some(Page {
                        entries: Arc::new(response.entries.into_iter().map(Arc::new).collect()),
                        total: response.total,
                        dropped: response.dropped,
                    }));
                }
                Err(error) => self.unavailable.set(Some(error.to_string())),
            }
        });
    }

    pub fn press_clear(self) {
        if self.armed.get_untracked() {
            self.clear();
            return;
        }
        self.armed.set(true);
        if let Some(clock) = self.clock.get_value() {
            let handle = clock.set_timeout(CLEAR_ARM, move || self.armed.set(false));
            self.disarm.set_value(Some(handle));
        }
    }

    fn clear(self) {
        self.armed.set(false);
        let sink = self.sink.get_value();
        let filter = self.filter.get_untracked();
        let path = format!("/api/decoderlog?{}", query_string(&filter.query(&sink)));
        let live = self
            .rows
            .with_untracked(|rows| rows.iter().filter(|row| row.live).count());
        let store = self.store;
        zgui::task::spawn_local(async move {
            let answer = store
                .api()
                .send::<(), DeletedCount>(reqwest::Method::DELETE, &path, None)
                .await;
            match answer {
                Ok(count) => {
                    store.drop_decoded(|record| filter.matches(record, &sink));
                    self.rejected.set(None);
                    self.cleared.set(Some(count.deleted + live as u64));
                }
                Err(error) => self.rejected.set(Some(error.to_string())),
            }
            self.revision.update(|n| *n += 1);
        });
    }

    pub fn toggle(self, key: &str) {
        let next = (self.opened.get_untracked().as_deref() != Some(key)).then(|| key.to_owned());
        self.opened.set(next);
    }

    pub fn save_widths(self) {
        let raw = self.widths.get_untracked().write();
        let Some(path) = widths_path() else {
            return;
        };
        zgui::task::spawn_local(async move {
            let written = zgui::task::blocking(move || {
                if let Some(folder) = path.parent() {
                    std::fs::create_dir_all(folder)?;
                }
                std::fs::write(path, raw)
            })
            .await;
            if let Err(error) = written {
                tracing::debug!(%error, "cannot keep the decoder log columns");
            }
        });
    }
}

fn widths_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|folder| folder.join("sdrmm-native").join(WIDTHS_FILE))
}

fn load_widths() -> ColumnWidths {
    let raw = widths_path().and_then(|path| std::fs::read_to_string(path).ok());
    ColumnWidths::read(raw.as_deref())
}
