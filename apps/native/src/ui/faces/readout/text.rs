use zgui::{
    geom::{DevicePx, Point},
    prelude::*,
    reactive::RenderEffect,
};

use crate::{
    decoders::views::{DecoderScope, TRANSCRIPT_LIMIT, build_transcript, is_at_bottom, latest_wpm},
    store::Store,
};

use super::{frames_of, records};

fn title(kind: &str) -> &'static str {
    match kind {
        "morse" => "Morse",
        "psk" => "PSK",
        _ => "RTTY",
    }
}

pub fn view(store: Store, kind: &'static str, scope: DecoderScope) -> impl IntoView {
    let frames = frames_of(store, kind);
    let transcript =
        Memo::new(move |_| build_transcript(&records(frames, scope), TRANSCRIPT_LIMIT));
    let wpm = Memo::new(move |_| {
        (kind == "morse")
            .then(|| latest_wpm(&records(frames, scope)))
            .flatten()
    });
    let pane = NodeRef::new();
    let stick = StoredValue::new(true);
    let follow = RenderEffect::new(move |_| {
        transcript.track();
        if stick.get_value() {
            pane.scroll_to(
                ScrollTarget::By(Point::new(DevicePx(0.0), DevicePx(1.0e7))),
                ScrollBehavior::Instant,
            );
        }
    });
    on_cleanup_local(move || drop(follow));
    let clipboard = use_clipboard();
    let copy = move |_: &mut EventCx<'_, events::Click>| {
        clipboard.set_text(ClipboardKind::Standard, transcript.get_untracked());
        store.say("Transcript copied");
    };
    let scrolled = move |ev: &mut EventCx<'_, events::Scroll>| {
        stick.set_value(is_at_bottom(
            ev.offset.y.0,
            ev.content_size.height.0,
            ev.scrollport.height.0,
        ));
    };
    view! {
        column(class = "dk-pane") {
            row(class = "dk-line") {
                text(class = "legend") {{title(kind)}}
                text(class = "dk-num") {{move || wpm.get().map(|wpm| format!("{wpm:.0} WPM"))}}
                control(
                    class = "btn dk-push",
                    state:disabled = move || transcript.with(String::is_empty),
                    on:click:stop = copy
                ) {"Copy all"}
            }
            scroll(
                class = "dk-pre",
                node_ref = pane,
                tabindex = Focus::Sequential,
                a11y:label = format!("{kind} transcript"),
                on:scroll = scrolled
            ) {
                text {{move || transcript.get()}}
            }
        }
    }
}
