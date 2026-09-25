use std::time::Duration;

use sdrmm_wire::rest::{OccupancyBucket, OccupancyReport};
use zgui::prelude::*;

use crate::{
    shell::occupancy::{
        HOURS, MAX_ROWS, MIN_SAMPLES, Sort, bucket_hz, duty_alpha, duty_text, has_occupancy,
        hour_text, row_hint, rows,
    },
    store::Store,
    ui::{
        kit_shell::{Entry, entry, hint},
        library::{active_set, load, reload, tune_radio},
        shell::Shell,
        widgets::segments,
    },
};

const REFRESH: Duration = Duration::from_secs(15);

const SHEET: &str = css!(
    r#"
.occ { flex-direction: column; border: 1px solid var(--line); border-radius: 4px; overflow: hidden; }
.occ__axis { justify-content: space-between; padding: 0 52px 2px 104px; }
.occ__row { align-items: center; gap: 8px; padding: 4px 8px; border-top: 1px solid var(--line); background-color: var(--panel); }
.occ__row:first-child { border-top-width: 0; }
.occ__row:hover { background-color: var(--panel-2); }
.occ__row:disabled { opacity: 0.5; }
.occ__hz { width: 96px; flex: 0 0 auto; font-family: var(--mono); font-size: 12px; color: var(--ink); }
.occ__row:hover .occ__hz { color: var(--accent); }
.occ__hours { flex: 1 1 auto; min-width: 0; gap: 1px; }
.occ__cell { flex: 1 1 0; height: 12px; border-radius: 1px; background-color: var(--accent); }
.occ__duty { width: 40px; flex: 0 0 auto; text-align: right; font-family: var(--mono); font-size: 11px; color: var(--ink-dim); }
"#
);

pub fn panel(store: Store, shell: Shell) -> impl IntoView {
    install_stylesheet("shell-occupancy", SHEET);
    let path = format!("/api/occupancy?min_samples={MIN_SAMPLES}");
    let report = load::<OccupancyReport>(store, path.clone());
    let refreshing = set_interval(REFRESH, move || reload(store, path.clone(), report));
    on_cleanup_local(move || drop(refreshing));
    let sort = RwSignal::new(Sort::Busiest);
    let query = RwSignal::new_local(String::new());
    let active = active_set(store);
    let enabled = Signal::derive_local(move || active.get().is_some());
    let shown = move || {
        let report = report.get().and_then(Result::ok);
        rows(report.as_deref(), sort.get(), &query.get(), MAX_ROWS)
    };
    let hints = move || match report.get() {
        None => Some(hint("Reading the statistics")),
        Some(Err(_)) => Some(hint("Could not read the statistics.")),
        Some(Ok(found)) if !has_occupancy(Some(&found)) => Some(hint("Nothing measured yet.")),
        Some(Ok(_)) => None,
    };
    let footer = move || {
        let total = report
            .get()
            .and_then(Result::ok)
            .map_or(0, |found| found.buckets.len());
        let count = shown().len();
        (count > 0 && total > count).then(|| {
            let which = if count == MAX_ROWS {
                "that fit"
            } else {
                "that match"
            };
            hint(format!(
                "{count} of {total} frequencies, the busiest {which}."
            ))
        })
    };
    let listing = move || {
        let buckets = shown();
        (!buckets.is_empty()).then(move || {
            let axis: Vec<AnyView> = [0, 6, 12, 18]
                .into_iter()
                .map(|hour| AnyView::new(view! { text(class = "legend") {{hour_text(hour)}} }))
                .collect();
            let lines: Vec<AnyView> = buckets
                .into_iter()
                .map(|bucket| AnyView::new(bucket_row(store, shell, bucket, active, enabled)))
                .collect();
            AnyView::new(view! {
                row(class = "occ__axis") {{axis}}
                column(class = "occ") {{lines}}
            })
        })
    };
    view! {
        column(class = "sk-panel") {
            {move || active.get().is_none().then(|| hint("Select a device node first."))}
            row(class = "sk-toolbar") {
                {segments(vec![(Sort::Busiest, "Busiest"), (Sort::Frequency, "Frequency")], sort.into(), move |picked| sort.set(picked))}
                {entry(Entry::new(query, "145.5, 433"), || {}, || {})}
            }
            {hints}
            {listing}
            {footer}
        }
    }
}

fn bucket_row(
    store: Store,
    shell: Shell,
    bucket: OccupancyBucket,
    active: Signal<Option<sdrmm_wire::state::DeviceSet>>,
    enabled: Signal<bool, LocalStorage>,
) -> impl IntoView {
    let cells: Vec<AnyView> = (0..HOURS)
        .map(|hour| {
            let alpha = duty_alpha(bucket.by_hour.get(hour).copied().unwrap_or_default());
            AnyView::new(view! {
                box(class = "occ__cell", style:opacity = Some(format!("{alpha:.3}")))
            })
        })
        .collect();
    let freq_hz = bucket.freq_hz as f64;
    let label = row_hint(&bucket);
    view! {
        control(
            class = "occ__row",
            tabindex = Focus::Sequential,
            a11y:role = Role::Button,
            a11y:label = label,
            state:disabled = move || !enabled.get(),
            on:click:stop = move |_| {
                if let Some(set) = active.get_untracked() {
                    tune_radio(store, shell, &set, freq_hz);
                }
            }
        ) {
            text(class = "occ__hz") {{bucket_hz(bucket.freq_hz)}}
            row(class = "occ__hours") {{cells}}
            text(class = "occ__duty") {{duty_text(bucket.duty)}}
        }
    }
}
