use zgui::prelude::*;

use crate::{store::Store, ui::kit_audio};

pub fn face(store: Store, node: String) -> impl IntoView {
    let playing = Signal::derive(move || {
        store.device_set_of(&node).filter(|set| {
            store
                .set_of(*set)
                .is_some_and(|found| found.playback.is_some())
        })
    });
    view! {
        column(class = "face") {
            {move || match playing.get() {
                Some(set) => AnyView::new(kit_audio::transport(store, set)),
                None => AnyView::new(super::plain("recording")),
            }}
        }
    }
}
