use std::time::Duration;

use zgui::prelude::*;

use crate::{
    decoded::now_ms,
    decoders::views::{
        Age, DecoderScope, TARGET_MAX_AGE_MS, TargetRow, TargetSort, age_class, aircraft_row,
        format_age, ship_row, sort_targets,
    },
    store::Store,
};

use super::stations_of;

const SHOWN_TARGETS: usize = 200;

struct Columns {
    title: &'static str,
    id: &'static str,
    label: &'static str,
    primary: &'static str,
    secondary: &'static str,
}

const AIRCRAFT: Columns = Columns {
    title: "Aircraft",
    id: "ICAO",
    label: "Callsign",
    primary: "Altitude",
    secondary: "Speed / track",
};

const SHIPS: Columns = Columns {
    title: "Ships",
    id: "MMSI",
    label: "Name",
    primary: "Speed",
    secondary: "Course / destination",
};

pub fn view(store: Store, aircraft: bool, scope: DecoderScope) -> impl IntoView {
    let stations = stations_of(store, if aircraft { "adsb" } else { "ais" });
    let now = RwSignal::new(now_ms());
    let tick = set_interval(Duration::from_secs(1), move || {
        now.set(now_ms());
        store.age_out_stations(TARGET_MAX_AGE_MS);
    });
    on_cleanup_local(move || drop(tick));
    let sort = RwSignal::new(TargetSort::Age);
    let descending = RwSignal::new(false);
    let toggle = move |key: TargetSort| {
        if sort.get_untracked() == key {
            descending.update(|down| *down = !*down);
        } else {
            sort.set(key);
            descending.set(false);
        }
    };
    let columns = if aircraft { &AIRCRAFT } else { &SHIPS };
    let rows = move || {
        let at = now.get();
        let rows: Vec<TargetRow> = stations.with(|stations| {
            stations
                .iter()
                .flat_map(|stations| stations.values())
                .filter(|station| scope.holds(station.device_set, station.channel))
                .filter_map(|station| {
                    if aircraft {
                        aircraft_row(station, at)
                    } else {
                        ship_row(station, at)
                    }
                })
                .collect()
        });
        sort_targets(&rows, sort.get(), descending.get())
    };
    let table = move || {
        let rows = rows();
        let count = rows.len();
        if count == 0 {
            let said = format!("No {} heard.", columns.title.to_lowercase());
            return AnyView::new(view! { text(class = "hint") {{said}} });
        }
        let hidden = count.saturating_sub(SHOWN_TARGETS);
        let body: Vec<AnyView> = rows
            .into_iter()
            .take(SHOWN_TARGETS)
            .map(|row| AnyView::new(target_row(row)))
            .collect();
        let more = (hidden > 0)
            .then(|| AnyView::new(view! { text(class = "dk-more") {{format!("{hidden} more")}} }));
        AnyView::new(view! {
            column(class = "dk-table") {
                row(class = "dk-tr") {
                    {sort_head(columns.id, TargetSort::Id, sort, descending, toggle)}
                    text(class = "dk-td dk-th") {{columns.label}}
                    text(class = "dk-td dk-th") {{columns.primary}}
                    text(class = "dk-td dk-th wide") {{columns.secondary}}
                    text(class = "dk-td dk-th wide") {"Position"}
                    {sort_head("Age", TargetSort::Age, sort, descending, toggle)}
                }
                {body}
                {more}
            }
        })
    };
    let count = move || {
        stations
            .with(|s| s.as_ref().map_or(0, |s| s.len()))
            .to_string()
    };
    view! {
        column(class = "dk-pane") {
            row(class = "dk-line") {
                text(class = "legend") {{columns.title}}
                text(class = "dk-num") {{count}}
            }
            {table}
        }
    }
}

fn sort_head(
    label: &'static str,
    key: TargetSort,
    sort: RwSignal<TargetSort>,
    descending: RwSignal<bool>,
    toggle: impl Fn(TargetSort) + 'static,
) -> impl IntoView {
    let arrow = move || (sort.get() == key).then(|| if descending.get() { " v" } else { " ^" });
    view! {
        control(class = "dk-td dk-th dk-sort", on:click:stop = move |_| toggle(key)) {
            {label}
            {arrow}
        }
    }
}

fn target_row(row: TargetRow) -> impl IntoView {
    let tone = match age_class(row.age_ms) {
        Age::Fresh => "",
        Age::Stale => "dim",
        Age::Fading => "fade",
    };
    let secondary = if row.secondary.is_empty() {
        "-".to_owned()
    } else {
        row.secondary
    };
    view! {
        row(class = "dk-tr") {
            text(class = format!("dk-td {tone}")) {{row.id}}
            text(class = format!("dk-td {tone}")) {{row.label}}
            text(class = format!("dk-td {tone}")) {{row.primary}}
            text(class = format!("dk-td wide {tone}")) {{secondary}}
            text(class = format!("dk-td wide {tone}")) {{row.position}}
            text(class = format!("dk-td {tone}")) {{format_age(row.age_ms)}}
        }
    }
}
