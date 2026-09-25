use std::time::Duration;

use sdrmm_wire::{
    AudioRoute,
    frame::{AudioFrame, FrameKind},
    ws::{ClientCommand, ServerEvent, StreamKind},
};
use zgui::prelude::*;

use super::Audio;
use crate::{
    binding,
    bus::{Frame, Source},
    store::Store,
};

const POLL: Duration = Duration::from_millis(500);

pub fn player(store: Store, audio: Audio) -> impl IntoView {
    store.on_frame(move |frame| hear(store, audio, frame));
    store.on_event(move |event| follow(audio, event));
    let dropped = zgui::reactive::RenderEffect::new(move |_| {
        if !store.connected.get() {
            audio
                .engine
                .try_with_value(|engine| engine.borrow_mut().disconnected());
            audio.edit_all(|_, entry| entry.live = false);
        }
    });
    let polling = set_interval(POLL, move || poll(store, audio));
    on_cleanup_local(move || {
        drop(dropped);
        drop(polling);
    });

    let reachable = Signal::derive(move || {
        let state = store.state.get();
        binding::speaker_routes(&store.graph.get(), &state.device_sets, &state.trunk_systems)
    });
    let retaining = zgui::reactive::RenderEffect::new(move |_| {
        let reachable = reachable.get();
        if store.state.with(|state| state.device_sets.is_empty()) {
            return;
        }
        audio.edit_all(|route, entry| {
            if entry.wanted && !reachable.contains(route) {
                entry.wanted = false;
                entry.live = false;
            }
        });
    });
    on_cleanup_local(move || drop(retaining));

    view! {
        for route in move || {
            let reachable = reachable.get();
            audio
                .wanted()
                .into_iter()
                .filter(|route| reachable.contains(route))
                .collect::<Vec<_>>()
        }, key = |route: &AudioRoute| route.clone() {
            {voice(store, audio, route)}
        }
    }
}

fn voice(store: Store, audio: Audio, route: AudioRoute) {
    let gain = audio.entry_untracked(&route).gain();
    let opened = audio
        .engine
        .try_with_value(|engine| engine.borrow_mut().open(&route, gain));
    match opened {
        Some(Ok(())) => {
            store.hold(ClientCommand::SubscribeAudio {
                device_set: route.device_set,
                channel: route.channel,
                fx: route.fx.clone(),
            });
            on_cleanup_local(move || {
                audio
                    .engine
                    .try_with_value(|engine| engine.borrow_mut().close(&route));
            });
        }
        Some(Err(error)) => audio.fail(&route, error.to_string()),
        None => audio.fail(&route, String::from("audio is shutting down")),
    }
}

fn hear(store: Store, audio: Audio, frame: &Frame) {
    if frame.kind != FrameKind::AudioOpus {
        return;
    }
    let Some(Source::Audio {
        device_set,
        channel,
        fx,
    }) = store.source_of(frame.stream_id)
    else {
        return;
    };
    let Some(decoded) = AudioFrame::decode(&frame.bytes) else {
        return;
    };
    let route = AudioRoute {
        device_set,
        channel,
        fx,
    };
    let taps = audio
        .taps
        .try_with_value(|taps| taps.borrow().listeners(&route))
        .unwrap_or_default();
    let fed = audio.engine.try_with_value(|engine| {
        engine.borrow_mut().feed(
            &route,
            decoded.timestamp,
            decoded.ch_layout,
            decoded.opus,
            &taps,
        )
    });
    if let Some(Err(error)) = fed {
        let said = error.to_string();
        audio.edit(&route, |entry| entry.error = Some(said));
    }
}

fn follow(audio: Audio, event: &ServerEvent) {
    match event {
        ServerEvent::AudioStreamStarted {
            stream_id,
            device_set,
            channel,
            fx,
        } => {
            let route = AudioRoute {
                device_set: *device_set,
                channel: *channel,
                fx: fx.clone(),
            };
            let bound = audio
                .engine
                .try_with_value(|engine| engine.borrow_mut().started(*stream_id, route.clone()));
            if bound == Some(true) {
                audio.edit(&route, |entry| {
                    entry.live = entry.wanted;
                    entry.error = None;
                });
            }
        }
        ServerEvent::StreamStopped {
            stream_id,
            kind: StreamKind::Audio,
        } => {
            let stopped = audio
                .engine
                .try_with_value(|engine| engine.borrow_mut().stopped(*stream_id))
                .flatten();
            if let Some(route) = stopped {
                audio.stop(&route);
            }
        }
        ServerEvent::Error { message } => {
            let refused = audio
                .engine
                .try_with_value(|engine| engine.borrow_mut().refused())
                .flatten();
            if let Some(route) = refused {
                audio.fail(&route, message.clone());
            }
        }
        _ => {}
    }
}

fn poll(store: Store, audio: Audio) {
    let polled = audio
        .engine
        .try_with_value(|engine| engine.borrow_mut().poll());
    if let Some(Err(error)) = polled {
        let lost = audio
            .engine
            .try_with_value(|engine| engine.borrow_mut().drop_all())
            .unwrap_or_default();
        let said = format!("audio output lost: {error}");
        for route in &lost {
            audio.fail(route, said.clone());
        }
        store.say(said);
    }
    let engine = audio.engine.get_value();
    audio.edit_all(|route, entry| {
        if let Some(health) = engine.borrow().health(route) {
            entry.health = health;
        }
    });
}
