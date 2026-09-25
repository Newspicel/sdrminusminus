use sdrmm_wire::{decode::DecoderEvent, rest::EventAudio};
use zgui::prelude::*;

use crate::{
    decoders::{detail::event_detail, log::LogRow},
    store::Store,
    ui::kit_decoders,
};

use super::state::Log;

pub fn opened(log: Log) -> Option<AnyView> {
    let key = log.opened.get()?;
    let row = log
        .rows
        .with(|rows| rows.iter().find(|row| row.key == key).cloned())?;
    Some(AnyView::new(view! {
        column(class = "dlog__detail") {
            {row_detail(log.store, &row)}
        }
    }))
}

fn audio_of(event: &DecoderEvent) -> Option<&EventAudio> {
    match event {
        DecoderEvent::Call(call) => call.audio.as_ref(),
        DecoderEvent::Transmission(t) => t.audio.as_ref(),
        _ => None,
    }
}

fn row_detail(store: Store, row: &LogRow) -> impl IntoView + use<> {
    let event = row.source.event();
    let detail = event_detail(event);
    let audio = audio_of(event).map(|audio| {
        let url = audio.url.clone();
        let extension = audio.media_type.rsplit('/').next().unwrap_or("bin");
        let name = format!("{}-{}.{extension}", row.kind, row.at.replace(':', "-"));
        AnyView::new(view! {
            row {
                control(
                    class = "btn",
                    on:click:stop = move |_| kit_decoders::save_download(store, url.clone(), name.clone())
                ) {"Save audio"}
            }
        })
    });
    let object = match event {
        DecoderEvent::BroadcastData(data) => {
            Some(AnyView::new(kit_decoders::broadcast_data_view(store, data)))
        }
        _ => None,
    };
    let empty = (detail.fields.is_empty() && detail.body.is_none())
        .then(|| AnyView::new(view! { text(class = "dk-dim") {"Nothing beyond the summary."} }));
    let fields =
        (!detail.fields.is_empty()).then(|| AnyView::new(kit_decoders::fields_view(detail.fields)));
    let body = detail
        .body
        .map(|body| AnyView::new(view! { text(class = "dk-pre") {{body}} }));
    view! {
        {audio}
        {object}
        {fields}
        {body}
        {empty}
    }
}
