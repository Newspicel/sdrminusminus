use std::sync::Arc;

use sdrmm_wire::{
    bandplan::{BandPlan, BandRegionsResponse},
    units,
};
use zgui::prelude::*;

use crate::{
    shell::band_plan::{BandMatch, band_tune_hz, search_plan, service_label, service_token},
    store::Store,
    ui::{
        kit_shell::{Entry, Row, entry, hint, list, list_row},
        library::{channel_type_of, load, suggest_mode, target, tune},
        shell::Shell,
        widgets::{check, pick},
    },
};

const LIMIT: usize = 30;

const SHEET: &str = css!(
    r#"
.band__swatch { width: 8px; height: 8px; border-radius: 1px; flex: 0 0 auto; background-color: var(--line-strong); }
.band__swatch[data-service="amateur"] { background-color: oklch(0.64 0.1 300); }
.band__swatch[data-service="broadcast"] { background-color: oklch(0.64 0.1 60); }
.band__swatch[data-service="aeronautical"] { background-color: oklch(0.64 0.1 235); }
.band__swatch[data-service="maritime"] { background-color: oklch(0.64 0.09 200); }
.band__swatch[data-service="mobile"] { background-color: oklch(0.64 0.09 145); }
.band__swatch[data-service="satellite"] { background-color: oklch(0.64 0.1 345); }
.band__swatch[data-service="navigation"] { background-color: oklch(0.64 0.09 265); }
.band__swatch[data-service="science"] { background-color: oklch(0.64 0.08 175); }
.band__swatch[data-service="ism"] { background-color: oklch(0.64 0.1 25); }
.band__swatch[data-service="other"] { background-color: oklch(0.6 0.02 80); }
.band__region { width: 220px; flex: 0 0 auto; }
.band__line { align-items: center; padding-left: 8px; border-top: 1px solid var(--line); background-color: var(--panel); }
.band__line:first-child { border-top-width: 0; }
.band__line > .sk-row { flex: 1 1 auto; min-width: 0; border-top-width: 0; }
"#
);

type Loaded<T> = RwSignal<Option<Result<Arc<T>, String>>>;

pub fn panel(store: Store, shell: Shell) -> impl IntoView {
    install_stylesheet("shell-bands", SHEET);
    let regions = load::<BandRegionsResponse>(store, String::from("/api/bandplan/regions"));
    let plan: Loaded<BandPlan> = RwSignal::new(None);
    let region = Signal::derive(move || {
        store.settings.get().band_region.clone().or_else(|| {
            regions
                .get()
                .and_then(Result::ok)
                .map(|regions| regions.default_region.clone())
        })
    });
    let fetching = zgui::reactive::RenderEffect::new(move |_| {
        if let Some(region) = region.get() {
            super::reload(store, format!("/api/bandplan/regions/{region}"), plan);
        }
    });
    on_cleanup_local(move || drop(fetching));
    let options = move || {
        regions
            .get()
            .and_then(Result::ok)
            .map(|regions| {
                regions
                    .regions
                    .iter()
                    .map(|found| (found.id.clone(), found.name.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let region_pick = move || {
        pick(options(), region, move |picked: String| {
            store.edit_settings(|settings| settings.band_region = Some(picked));
        })
    };
    let ruler = Signal::derive(move || store.settings.get().band_ruler);
    let query = RwSignal::new_local(String::new());
    let aimed = target(store);
    let hits = move || {
        plan.get()
            .and_then(Result::ok)
            .map(|plan| search_plan(&plan, &query.get(), LIMIT))
            .unwrap_or_default()
    };
    let tunable = Signal::derive_local(move || aimed.get().is_some_and(|aimed| !aimed.locked()));
    let hints = move || {
        let Some(Ok(plan)) = plan.get() else {
            return Some(hint("Loading the band plan"));
        };
        let found = hits();
        if !query.get().trim().is_empty() && found.is_empty() {
            return Some(hint(format!(
                "Nothing in {} matches that.",
                plan.region.name
            )));
        }
        (!tunable.get() && !found.is_empty()).then(|| {
            hint(if aimed.get().is_none() {
                "Select a device or decoder to tune."
            } else {
                "Tuning is locked here."
            })
        })
    };
    let rows = move || {
        hits()
            .into_iter()
            .map(|hit| AnyView::new(band_row(store, shell, hit, aimed, tunable)))
            .collect::<Vec<_>>()
    };
    view! {
        column(class = "sk-panel") {
            row(class = "sk-toolbar") {
                box(class = "band__region") {{region_pick}}
                {check(ruler, move |on| store.edit_settings(|settings| settings.band_ruler = on))}
                text(class = "sk-text") {"Ruler"}
            }
            {entry(Entry::new(query, "marine VHF, 70 cm ham, 145.500"), || {}, || {})}
            {hints}
            {move || (!hits().is_empty()).then(|| list(None, rows))}
        }
    }
}

fn band_row(
    store: Store,
    shell: Shell,
    hit: BandMatch,
    aimed: Signal<Option<crate::shell::library_target::TuneTarget>>,
    tunable: Signal<bool, LocalStorage>,
) -> impl IntoView {
    let allocation = hit.allocation;
    let mut facts = vec![
        format!(
            "{}\u{2013}{}",
            units::hertz(allocation.start_hz),
            units::hertz(allocation.stop_hz)
        ),
        service_label(allocation.service).to_owned(),
    ];
    if !hit.lane_name.is_empty() && hit.lane_id != "allocation" {
        facts.push(hit.lane_name);
    }
    let suggested = allocation
        .suggested
        .as_ref()
        .map(|params| params.type_id().to_owned());
    let primary = match &suggested {
        Some(kind) => format!("{}  {kind}", allocation.name),
        None => allocation.name.clone(),
    };
    let hz = band_tune_hz(&allocation);
    let service = service_token(allocation.service);
    let pick_band = move || {
        let Some(aimed) = aimed.get_untracked() else {
            return;
        };
        tune(store, shell, &aimed, hz);
        suggest_mode(
            store,
            suggested.as_deref(),
            channel_type_of(store, &aimed).as_deref(),
            "band",
        );
    };
    view! {
        row(class = "band__line") {
            box(class = "band__swatch", attr:data-service = service) {}
            {list_row(
                Row { primary, secondary: Some(facts.join(" \u{b7} ")) },
                Some(Box::new(pick_band)),
                tunable,
                AnyView::new(()),
                AnyView::new(()),
            )}
        }
    }
}
