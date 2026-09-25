use sdrmm_wire::bandplan::{BandAllocation, BandPlan};
use zgui::{
    geom::{Css, CssPx, Point},
    prelude::*,
};

use super::{
    ScopeCx,
    actions::tune_to_band,
    bands::{
        BandIdentity, covered_by_layer, flatten_lanes, identify, provision_text, service_label,
        service_name, spans_in, suggested_at,
    },
    pick::format_hz,
    view::span_to_offset,
};

const LABEL_MIN: f64 = 0.07;
const TIP_WIDTH: f64 = 288.0;

fn window_of(cx: ScopeCx) -> Option<(f64, f64)> {
    let meta = cx.meta.get().filter(|meta| meta.span_hz > 0.0)?;
    let view = cx.view.get();
    Some((
        meta.centre_hz + span_to_offset(view.start, meta.span_hz),
        meta.span_hz * view.width(),
    ))
}

fn fraction(node: NodeRef, position: Point<CssPx, Css>) -> Option<(f64, f64)> {
    let bounds = node.window_bounds()?;
    let scale = f64::from(node.scale().max(0.01));
    let left = f64::from(bounds.origin.x.0) / scale;
    let width = f64::from(bounds.size.width.0) / scale;
    (width > 0.0).then(|| {
        (
            ((f64::from(position.x.0) - left) / width).clamp(0.0, 1.0),
            width,
        )
    })
}

pub fn shown(cx: ScopeCx) -> bool {
    cx.store.settings.with(|settings| settings.band_ruler)
        && cx.plan.with(Option::is_some)
        && window_of(cx).is_some()
}

pub fn ruler(cx: ScopeCx) -> impl IntoView {
    move || shown(cx).then(|| AnyView::new(lane(cx)))
}

fn lane(cx: ScopeCx) -> impl IntoView {
    let own = NodeRef::new();
    let hover = RwSignal::new(None::<(f64, f64)>);
    let spans = move || {
        let plan = cx.plan.get()?;
        let (low, visible) = window_of(cx)?;
        let lanes: Vec<_> = plan
            .lanes
            .iter()
            .map(|lane| spans_in(&plan, lane, low, visible))
            .collect();
        let pieces = flatten_lanes(&plan, &lanes)
            .into_iter()
            .filter_map(|span| {
                let allocation = plan.allocations.get(span.of)?;
                let service = service_name(allocation.service);
                let label = (span.width >= LABEL_MIN).then(|| {
                    view! { text(class = "scope__band-name") {{allocation.name.clone()}} }
                });
                let edge = span
                    .starts_inside
                    .then(|| view! { box(class = format!("scope__band-edge band--{service}")) });
                Some(view! {
                    box(
                        class = format!("scope__band band-fill--{service}"),
                        style:left = Some(format!("{:.4}%", span.left * 100.0)),
                        style:width = Some(format!("{:.4}%", span.width * 100.0))
                    ) {
                        {edge}
                        {label}
                    }
                })
            })
            .collect::<Vec<_>>();
        Some(pieces)
    };
    let tune = move |ev: &mut EventCx<'_, events::Click>| {
        ev.stop_propagation();
        let (Some(plan), Some((low, visible)), Some((at, _))) = (
            cx.plan.get_untracked(),
            window_of(cx),
            fraction(own, ev.position),
        ) else {
            return;
        };
        let hz = (low + at * visible).round();
        tune_to_band(cx, hz, suggested_at(&identify(&plan, hz)));
    };
    let tip = move || {
        let (at, width) = hover.get()?;
        let plan = cx.plan.get()?;
        let (low, visible) = window_of(cx)?;
        let left = (at * width - TIP_WIDTH / 2.0).clamp(0.0, (width - TIP_WIDTH).max(0.0));
        Some(view! {
            column(class = "scope__band-tip", style:left = Some(format!("{left:.1}px"))) {
                {identify_tip(&plan, low + at * visible)}
            }
        })
    };
    view! {
        box(
            node_ref = own,
            class = "scope__ruler",
            a11y:role = Role::Button,
            a11y:label = "Band plan",
            on:pointer_down:stop = move |_| {},
            on:pointer_move = move |ev: &mut EventCx<'_, events::PointerMove>| {
                hover.set(fraction(own, ev.position));
            },
            on:pointer_leave = move |_| hover.set(None),
            on:click = tune
        ) {
            {spans}
            {tip}
        }
    }
}

fn identify_tip(plan: &BandPlan, hz: f64) -> impl IntoView {
    let found = identify(plan, hz);
    let suggested = suggested_at(&found)
        .map(|params| format!("click to tune · {}", params.type_id()))
        .unwrap_or_else(|| String::from("click to tune"));
    let layer_name = |id: &str| {
        plan.layers
            .iter()
            .find(|layer| layer.id == id)
            .map_or_else(|| id.to_owned(), |layer| layer.authority.clone())
    };
    let first_lane = found.first().map(|entry| entry.lane_id);
    let details: Vec<_> = found
        .iter()
        .map(|entry| {
            AnyView::new(band_detail(
                plan,
                entry,
                &layer_name,
                Some(entry.lane_id) == first_lane,
            ))
        })
        .collect();
    let empty = found
        .is_empty()
        .then(|| view! { text(class = "scope__faint") {"Nothing allocated here"} });
    view! {
        row(class = "scope__tip-head") {
            text(class = "scope__tip-hz") {{format_hz(hz)}}
            spacer()
            text(class = "scope__faint") {{suggested}}
        }
        {empty}
        {details}
    }
}

fn meta_line(allocation: &BandAllocation, layer_name: &dyn Fn(&str) -> String) -> String {
    let mut parts = vec![
        service_label(allocation.service),
        layer_name(&allocation.layer),
        format!(
            "{} - {}",
            format_hz(allocation.start_hz),
            format_hz(allocation.stop_hz)
        ),
    ];
    if let Some(reference) = allocation.reference.as_ref().filter(|reference| {
        **reference != allocation.name && **reference != allocation.official_name
    }) {
        parts.push(reference.clone());
    }
    if let Some(step) = allocation.channel_step_hz {
        parts.push(format!("{} steps", format_hz(step)));
    }
    parts.join(" · ")
}

fn band_detail(
    plan: &BandPlan,
    entry: &BandIdentity<'_>,
    layer_name: &dyn Fn(&str) -> String,
    covered: bool,
) -> impl IntoView {
    let allocation = entry.allocation;
    let service = service_name(allocation.service);
    let secondary =
        (!allocation.primary).then(|| view! { text(class = "scope__faint") {"secondary"} });
    let official = (allocation.official_name != allocation.name).then(
        || view! { text(class = "scope__tip-official") {{allocation.official_name.clone()}} },
    );
    let notes = allocation
        .notes
        .clone()
        .map(|notes| view! { text(class = "scope__tip-notes") {{notes}} });
    let provisions: Vec<_> = allocation
        .provisions
        .iter()
        .map(|id| {
            let known = provision_text(plan, &allocation.layer, id).is_some();
            AnyView::new(
                view! { text(class = "scope__provision", class:known = known) {{id.clone()}} },
            )
        })
        .collect();
    let groups: Vec<_> = if covered {
        covered_by_layer(&entry.covered, |layer| {
            if layer == allocation.layer {
                String::from("also")
            } else {
                format!("over {}", layer_name(layer))
            }
        })
        .into_iter()
        .map(|group| AnyView::new(view! { text(class = "scope__faint") {{format!("{}: {}", group.label, group.names.join(" · "))}} }))
        .collect()
    } else {
        Vec::new()
    };
    view! {
        column(class = "scope__tip-band") {
            row(class = "scope__tip-title") {
                box(class = format!("scope__swatch-dot band--{service}"))
                text(class = "scope__tip-name") {{allocation.name.clone()}}
                {secondary}
            }
            {official}
            text(class = "scope__faint") {{meta_line(allocation, layer_name)}}
            {notes}
            row(class = "scope__provisions") {{provisions}}
            {groups}
        }
    }
}
