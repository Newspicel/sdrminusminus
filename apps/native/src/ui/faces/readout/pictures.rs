use std::collections::HashMap;

use sdrmm_wire::{
    rest::{CapturedImage, CapturedImagesResponse},
    ws::ServerEvent,
};
use zgui::prelude::*;
use zgui_image::ImageBytes;

use crate::{
    decoders::views::{DecoderScope, format_clock},
    store::Store,
};

const KEPT_PICTURES: usize = 24;

#[derive(Clone, Copy)]
struct Pictures {
    store: Store,
    scope: DecoderScope,
    images: RwSignal<Vec<CapturedImage>>,
    urls: RwSignal<HashMap<u64, String>>,
    bytes: StoredValue<HashMap<u64, ImageBytes>, LocalStorage>,
    asked: StoredValue<Vec<u64>>,
    selected: RwSignal<Option<u64>>,
}

impl Pictures {
    fn new(store: Store, scope: DecoderScope) -> Self {
        let pictures = Self {
            store,
            scope,
            images: RwSignal::new(Vec::new()),
            urls: RwSignal::new(HashMap::new()),
            bytes: StoredValue::new_local(HashMap::new()),
            asked: StoredValue::new(Vec::new()),
            selected: RwSignal::new(None),
        };
        pictures.fetch();
        store.on_event(move |event: &ServerEvent| {
            if let ServerEvent::ImageCaptured(image) = event {
                pictures.receive(vec![(**image).clone()]);
            }
        });
        pictures
    }

    fn fetch(self) {
        zgui::task::spawn_local(async move {
            match self
                .store
                .api()
                .get::<CapturedImagesResponse>("/api/images")
                .await
            {
                Ok(response) => self.receive(response.images),
                Err(error) => self.store.say(format!("Cannot list pictures: {error}")),
            }
        });
    }

    fn receive(self, arrived: Vec<CapturedImage>) {
        let scope = self.scope;
        self.images.update(|images| {
            for image in arrived {
                if scope.holds(image.device_set, image.channel)
                    && !images.iter().any(|held| held.id == image.id)
                {
                    images.push(image);
                }
            }
            images.sort_by_key(|image| std::cmp::Reverse(image.id));
            images.truncate(KEPT_PICTURES);
        });
        let kept: Vec<u64> = self
            .images
            .with_untracked(|images| images.iter().map(|i| i.id).collect());
        self.bytes
            .update_value(|bytes| bytes.retain(|id, _| kept.contains(id)));
        self.asked
            .update_value(|asked| asked.retain(|id| kept.contains(id)));
        self.urls
            .update(|urls| urls.retain(|id, _| kept.contains(id)));
        for image in self.images.get_untracked() {
            self.load(image);
        }
    }

    fn load(self, image: CapturedImage) {
        let Some(source) = image.image else {
            return;
        };
        if self.asked.with_value(|asked| asked.contains(&image.id)) {
            return;
        }
        self.asked.update_value(|asked| asked.push(image.id));
        zgui::task::spawn_local(async move {
            match self.store.api().bytes(&source.url).await {
                Ok(bytes) => {
                    let registered = ImageBytes::new(bytes);
                    let url = registered.url();
                    self.bytes.update_value(|held| {
                        held.insert(image.id, registered);
                    });
                    self.urls.update(|urls| {
                        urls.insert(image.id, url);
                    });
                }
                Err(error) => self
                    .store
                    .say(format!("Cannot load picture {}: {error}", image.id)),
            }
        });
    }
}

pub fn view(store: Store, scope: DecoderScope) -> impl IntoView {
    let pictures = Pictures::new(store, scope);
    move || {
        let images = pictures.images.get();
        let chosen = pictures.selected.get();
        let Some(open) = images
            .iter()
            .find(|image| Some(image.id) == chosen)
            .or_else(|| images.first())
            .cloned()
        else {
            return AnyView::new(view! {
                text(class = "hint") {"No picture yet: one takes 36 s to four minutes."}
            });
        };
        let thumbs =
            (images.len() > 1).then(|| AnyView::new(thumbnails(pictures, &images, open.id)));
        AnyView::new(view! {
            column(class = "dk-pane") {
                {large(pictures, &open)}
                {thumbs}
            }
        })
    }
}

fn large(pictures: Pictures, open: &CapturedImage) -> impl IntoView + use<> {
    let id = open.id;
    let picture = match &open.image {
        None => AnyView::new(view! {
            text(class = "dk-danger") {{open.image_error.clone().unwrap_or_else(|| "The pixels were not kept".to_owned())}}
        }),
        Some(_) => AnyView::new(view! {
            image(
                class = "dk-picture",
                src = move || pictures.urls.with(|urls| urls.get(&id).cloned()),
                alt = Some(format!("{} picture", open.mode))
            )
        }),
    };
    let state = if open.complete {
        "complete".to_owned()
    } else {
        format!("{} of {} lines", open.lines, open.height)
    };
    view! {
        column(class = "dk-pane") {
            {picture}
            row(class = "dk-line") {
                text(class = "dk-num dk-accent") {{open.mode.clone()}}
                text(class = "legend") {{format!("{}×{}", open.width, open.height)}}
                text(class = "legend") {{state}}
                text(class = "dk-num dk-push") {{format_clock(&open.at)}}
            }
        }
    }
}

fn thumbnails(pictures: Pictures, images: &[CapturedImage], open: u64) -> impl IntoView + use<> {
    let items: Vec<_> = images
        .iter()
        .map(|image| {
            let id = image.id;
            let label = format!("{} at {}", image.mode, format_clock(&image.at));
            let inner = if image.image.is_some() {
                AnyView::new(view! {
                    image(src = move || pictures.urls.with(|urls| urls.get(&id).cloned()), alt = Some(String::new()))
                })
            } else {
                AnyView::new(view! { text(class = "legend") {"no pixels"} })
            };
            view! {
                control(
                    class = "dk-thumb",
                    class:on = id == open,
                    a11y:label = label,
                    on:click:stop = move |_| pictures.selected.set(Some(id))
                ) {
                    {inner}
                }
            }
        })
        .collect();
    view! { row(class = "dk-thumbs") {{items}} }
}
