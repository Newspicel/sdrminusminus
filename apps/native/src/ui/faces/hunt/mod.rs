mod model;

use sdrmm_wire::{
    HuntAction, HuntRequest, HuntSettings, HuntStatus, patch::NodeBody, ws::ServerEvent,
};
use zgui::prelude::*;

use self::model::{
    Bearing, HUNT_INTERVAL_MS, HuntTarget, bearing, format_hunt_db, format_mhz, format_strength,
    hunt_refusal, hunt_target, hunted_hz, live_hunt,
};
use crate::{
    audio::use_audio,
    store::Store,
    ui::{
        kit_audio::{self, button, edit_body, readout},
        widgets::{check, row_field},
    },
};

const SHEET: &str = css!(
    r#"
.hunt__meter { height: 12px; border-radius: 6px; background-color: var(--panel-3); overflow: hidden; }
.hunt__fill { height: 12px; border-radius: 6px; background-color: var(--accent-dim); }
.hunt__fill.near { background-color: var(--accent); }
.hunt__warm { color: var(--accent); }
"#
);

pub fn face(store: Store, node: String) -> impl IntoView {
    kit_audio::install();
    install_stylesheet("hunt", SHEET);
    let target = {
        let node = node.clone();
        Signal::derive(move || {
            hunt_target(&store.graph.get(), &store.state.get().device_sets, &node)
        })
    };
    let pushed = RwSignal::new(None::<HuntStatus>);
    store.on_event(move |event| {
        if let ServerEvent::HuntUpdate { device_set, status } = event
            && target.with_untracked(|target| {
                target.as_ref().is_some_and(|target| {
                    target.set.id == *device_set && target.channel.id == status.settings.channel
                })
            })
        {
            pushed.set(Some((**status).clone()));
        }
    });
    let status = Signal::derive(move || {
        let target = target.get();
        live_hunt(
            target.as_ref().map(|target| &target.set),
            target.as_ref().map(|target| target.channel.id),
            pushed.get().as_ref(),
        )
    });
    let clicks = clicks_of(store, node.clone());
    let strength = Signal::derive(move || status.get().map_or(0.0, |status| status.strength));
    let clicking = Signal::derive(move || status.get().is_some() && clicks.get());
    view! {
        column(class = "face") {
            if move || target.get().is_none() {
                text(class = "hint") {"Wire this node's control out to a decoder"}
            }
            if move || status.get().is_some() {
                {running(status)}
            } else {
                {idle(target)}
            }
            {row_field("Clicks", check(clicks, move |on| set_clicks(store, &node, on)))}
            if move || clicking.get() {
                {clicker(store, strength)}
            }
            row(class = "face__foot") {
                {controls(store, target, status, pushed)}
            }
        }
    }
}

fn clicks_of(store: Store, node: String) -> Signal<bool> {
    Signal::derive(move || {
        store
            .graph
            .get()
            .node(&node)
            .is_none_or(|found| !matches!(&found.body, NodeBody::Hunt(hunt) if !hunt.clicks))
    })
}

fn set_clicks(store: Store, node: &str, on: bool) {
    edit_body(store, node, |body| {
        if let NodeBody::Hunt(hunt) = body {
            hunt.clicks = on;
        }
    });
}

fn clicker(store: Store, strength: Signal<f32>) -> impl IntoView {
    match use_audio().map(|audio| audio.hold_clicks(strength)) {
        Some(Ok(())) => {}
        Some(Err(error)) => store.say(format!("no clicks: {error}")),
        None => store.say("no clicks: audio is off"),
    }
}

fn running(status: Signal<Option<HuntStatus>>) -> impl IntoView {
    let heading = Signal::derive(move || bearing(status.get().as_ref()));
    let fill = move || {
        let strength = status.get().map_or(0.0, |status| status.strength);
        Some(format!("{}%", (strength.clamp(0.0, 1.0) * 100.0).round()))
    };
    let near = move || matches!(heading.get(), Bearing::Closing | Bearing::Steady);
    let read = move |pick: fn(&HuntStatus) -> String| {
        AnyView::new(move || status.get().as_ref().map(pick).unwrap_or_default())
    };
    view! {
        column(class = "audio-controls") {
            box(class = "hunt__meter", a11y:role = Role::Meter, a11y:label = "Distance to the transmitter") {
                box(class = "hunt__fill", class:near = near, style:width = fill)
            }
            {readout(vec![
                ("Hunting", read(|status| format_mhz(hunted_hz(Some(status), None)))),
                ("Bearing", AnyView::new(view! {
                    text(class:hunt__warm = move || heading.get() == Bearing::Closing) {
                        {move || heading.get().label()}
                    }
                })),
                ("Strength", read(|status| format_strength(Some(status)))),
                ("Level", read(|status| format_hunt_db(status.level_db))),
                ("Smoothed", read(|status| format_hunt_db(status.smooth_db))),
                ("Walked", read(|status| format!("{} to {}", format_hunt_db(status.floor_db), format_hunt_db(status.best_db)))),
                ("Readings", read(|status| status.readings.to_string())),
            ])}
            {kit_audio::alert(move || status.get().and_then(|status| status.error))}
        }
    }
}

fn idle(target: Signal<Option<HuntTarget>>) -> impl IntoView {
    let hunting = move || {
        target.get().map_or_else(String::new, |target| {
            format!(
                "{} at {}",
                target.channel.settings.params.type_id(),
                format_mhz(hunted_hz(None, Some(&target.channel)))
            )
        })
    };
    view! {
        column(class = "audio-controls") {
            if move || target.get().is_some() {
                {readout(vec![("Hunting", AnyView::new(hunting))])}
            }
            {kit_audio::alert(move || hunt_refusal(target.get().as_ref()).map(str::to_owned))}
        }
    }
}

fn controls(
    store: Store,
    target: Signal<Option<HuntTarget>>,
    status: Signal<Option<HuntStatus>>,
    pushed: RwSignal<Option<HuntStatus>>,
) -> impl IntoView {
    let busy = RwSignal::new(false);
    let running = Signal::derive(move || status.get().is_some());
    let blocked = Signal::derive(move || {
        let target = target.get();
        busy.get()
            || target.is_none()
            || (!running.get() && hunt_refusal(target.as_ref()).is_some())
    });
    button(
        move || {
            String::from(if running.get() {
                "Stop hunt"
            } else {
                "Start hunt"
            })
        },
        running,
        blocked,
        move || {
            let Some(target) = target.get_untracked() else {
                return;
            };
            let request = if running.get_untracked() {
                HuntRequest {
                    action: HuntAction::Stop,
                    settings: None,
                }
            } else {
                HuntRequest {
                    action: HuntAction::Start,
                    settings: Some(HuntSettings {
                        channel: target.channel.id,
                        interval_ms: HUNT_INTERVAL_MS,
                    }),
                }
            };
            busy.set(true);
            zgui::task::spawn_local(async move {
                let path = format!(
                    "/api/devicesets/{}/channels/{}/hunt",
                    target.set.id, target.channel.id
                );
                match store.api().post::<_, HuntStatus>(&path, &request).await {
                    Ok(_) if request.action == HuntAction::Stop => pushed.set(None),
                    Ok(_) => {}
                    Err(error) => store.say(format!("cannot drive the hunt: {error}")),
                }
                busy.set(false);
                store.refresh_state();
            });
        },
    )
}
