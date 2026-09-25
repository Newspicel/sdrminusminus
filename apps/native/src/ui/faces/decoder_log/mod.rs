mod columns;
mod detail;
mod state;

use zgui::prelude::*;
use zgui_ui::prelude::*;

use crate::{
    decoders::log::{LIMIT_OPTIONS, LogRow, dropped_notice, export_path, kind_label},
    decoders::views::format_clock,
    store::Store,
    ui::{kit_decoders, widgets::pick},
};

use self::state::Log;

const ROW_HEIGHT: f32 = 20.0;

const SHEET: &str = css!(
    r#"
.dlog { flex-direction: column; gap: 6px; min-width: 0; }
.dlog__bar { flex-direction: row; align-items: center; gap: 8px; }
.dlog__search { flex: 1 1 auto; min-width: 0; }
.dlog__search .native-input { height: 26px; width: 100%; padding: 3px 8px; font-family: var(--mono); font-size: 11px; }
.dlog__limit { flex: 0 0 110px; }
.dlog__stats { flex-direction: row; gap: 10px; font-family: var(--mono); font-size: 10px; color: var(--ink-dim); }
.dlog__table { flex-direction: column; border: 1px solid var(--line); border-radius: 5px; overflow: hidden; }
.dlog__head { flex-direction: row; background-color: var(--panel); border-bottom: 1px solid var(--line); }
.dlog__th {
    position: relative;
    flex: 0 1 auto;
    min-width: 36px;
    padding: 2px 6px;
    font-family: var(--mono);
    font-size: 9px;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--ink-faint);
    border-right: 1px solid var(--line);
    overflow: hidden;
}
.dlog__th.flex { flex: 1 1 0; border-right: 0; }
.dlog__grip { position: absolute; top: 0; bottom: 0; right: 0; width: 7px; cursor: col-resize; }
.dlog__grip:hover, .dlog__grip:focus-visible, .dlog__grip.on { background-color: var(--accent); }
.dlog__list { height: 260px; }
.dlog__row { flex-direction: row; height: 20px; align-items: center; font-family: var(--mono); font-size: 11px; cursor: pointer; }
.dlog__row:hover { background-color: var(--panel-2); }
.dlog__row.on { background-color: var(--panel-3); }
.dlog__cell { flex: 0 1 auto; min-width: 36px; padding: 0 6px; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; color: var(--ink); }
.dlog__cell.dim { color: var(--ink-dim); }
.dlog__cell.num { text-align: right; }
.dlog__cell.flex { flex: 1 1 0; min-width: 0; }
.dlog__empty { padding: 6px 8px; color: var(--ink-dim); font-size: 12px; }
.dlog__detail { padding: 6px 8px; border: 1px solid var(--line); border-radius: 5px; background-color: var(--panel-2); max-height: 320px; overflow: auto; flex-direction: column; gap: 6px; }
.dlog__armed { color: var(--danger); border-color: var(--danger); }
"#
);

pub fn face(store: Store, node: String) -> impl IntoView {
    panel(store, node)
}

pub fn panel(store: Store, sink: String) -> impl IntoView {
    kit_decoders::install();
    install_stylesheet("decoder-log", SHEET);
    let log = Log::new(store, sink);
    view! {
        column(class = "face dlog", {..kit_decoders::no_pan()}) {
            {toolbar(log)}
            {notices(log)}
            {stats(log)}
            column(class = "dlog__table") {
                {columns::header(log)}
                {list(log)}
            }
            {move || detail::opened(log)}
            {footer(log)}
        }
    }
}

fn toolbar(log: Log) -> impl IntoView {
    let options: Vec<(u32, String)> = LIMIT_OPTIONS
        .iter()
        .map(|limit| (*limit, format!("{limit} rows")))
        .collect();
    let chosen = Signal::derive(move || Some(log.filter.get().limit));
    view! {
        row(class = "dlog__bar") {
            box(class = "dlog__search") {
                Input(
                    value = log.search,
                    class = "native-input",
                    label = "Search decoder log",
                    placeholder = "Search station or summary",
                )
            }
            box(class = "dlog__limit") {
                {pick(options, chosen, move |limit| log.set_limit(limit))}
            }
        }
    }
}

fn notices(log: Log) -> impl IntoView {
    let rejected = move || {
        log.rejected.get().map(|error| {
            AnyView::new(kit_decoders::alert(
                format!("Rejected: {error}"),
                move || log.rejected.set(None),
            ))
        })
    };
    let unavailable = move || {
        log.unavailable.get().map(|error| {
            AnyView::new(
                view! { row(class = "dk-alert") { text {{format!("Log unavailable: {error}")}} } },
            )
        })
    };
    let dropped = move || {
        let lost = log.store.decoded.with(|decoded| decoded.lost);
        let dropped = log
            .page
            .with(|page| page.as_ref().map_or(0, |page| page.dropped));
        dropped_notice(lost, dropped)
            .map(|notice| AnyView::new(view! { text(class = "dk-num dk-danger") {{notice}} }))
    };
    view! { column(class = "dk-pane") { {rejected} {unavailable} {dropped} } }
}

fn stats(log: Log) -> impl IntoView {
    let counts = move || {
        let shown = log.rows.with(|rows| rows.len());
        let total = log
            .page
            .with(|page| page.as_ref().map_or(0, |page| page.total));
        format!("{shown} shown · {total} stored")
    };
    let cleared = move || {
        log.cleared
            .get()
            .map(|count| format!("{count} rows cleared"))
    };
    let armed = move || {
        log.armed
            .get()
            .then_some("Clear removes every stored row this node sees")
    };
    view! {
        row(class = "dlog__stats") {
            text {{counts}}
            text(class = "dk-danger") {{armed}}
            text {{cleared}}
        }
    }
}

fn list(log: Log) -> impl IntoView {
    let count = Signal::derive_local(move || log.rows.with(|rows| rows.len()));
    let empty = move || {
        (count.get() == 0).then(|| {
            let said = if log.page.with(Option::is_none) {
                "Loading…"
            } else if log.filter.with(|filter| filter.filtered()) {
                "No rows match this filter."
            } else {
                "Nothing logged yet."
            };
            AnyView::new(view! { text(class = "dlog__empty") {{said}} })
        })
    };
    view! {
        {empty}
        VirtualList(
            count = count,
            row_size = ROW_HEIGHT,
            class = "dlog__list",
            label = "Decoder log",
            row = move |index: usize| move || {
                log.rows
                    .with(|rows| rows.get(index).cloned())
                    .map(|row| AnyView::new(row_view(log, row)))
            }
        )
    }
}

fn row_view(log: Log, row: LogRow) -> impl IntoView {
    let key = row.key.clone();
    let marked = row.key.clone();
    let widths = log.widths;
    let width = move |index: usize| move || Some(format!("{}px", widths.get().0[index]));
    let station = row.station.clone().unwrap_or_else(|| "-".to_owned());
    view! {
        row(
            class = "dlog__row",
            class:on = move || log.opened.get().as_deref() == Some(marked.as_str()),
            on:click:stop = move |_| log.toggle(&key)
        ) {
            text(class = "dlog__cell dim", style:width = width(0)) {{format_clock(&row.at)}}
            text(class = "dlog__cell dim", style:width = width(1)) {{kind_label(&row.kind)}}
            text(class = "dlog__cell num", style:width = width(2)) {{format!("{:.4} MHz", row.freq_hz / 1e6)}}
            text(class = "dlog__cell", style:width = width(3)) {{station}}
            text(class = "dlog__cell flex") {{row.summary.clone()}}
        }
    }
}

fn footer(log: Log) -> impl IntoView {
    let store = log.store;
    let export = move |format: &'static str| {
        move |_: &mut EventCx<'_, events::Click>| {
            let query = log.filter.get_untracked().query(&log.sink.get_value());
            kit_decoders::save_download(
                store,
                export_path(format, &query),
                format!("decoder-log.{format}"),
            );
        }
    };
    view! {
        row(class = "face__foot") {
            control(class = "btn", on:click:stop = export("csv")) {"CSV"}
            control(class = "btn", on:click:stop = export("json")) {"JSON"}
            spacer()
            control(
                class = "btn",
                class:dlog__armed = move || log.armed.get(),
                on:click:stop = move |_| log.press_clear()
            ) {
                {move || if log.armed.get() { "Confirm clear" } else { "Clear" }}
            }
        }
    }
}
