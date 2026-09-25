use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use sdrmm_wire::{
    frame::{FrameKind, VideoData, VideoFrame},
    ws::ClientCommand,
};
use zgui::prelude::*;

use crate::{
    binding,
    bus::Source,
    store::Store,
    ui::kit_raster::{Raster, Scene, raster_view},
};

const DISPLAY_ASPECT: f32 = 4.0 / 3.0;
const STALE: Duration = Duration::from_secs(2);
const STALE_CHECK: Duration = Duration::from_millis(500);

#[must_use]
pub fn rgba_of(data: VideoData<'_>, width: u16, height: u16) -> Option<Vec<u8>> {
    let count = usize::from(width) * usize::from(height);
    let mut rgba = Vec::with_capacity(count * 4);
    match data {
        VideoData::Gray(luma) => {
            for value in luma.get(..count)? {
                rgba.extend_from_slice(&[*value, *value, *value, 255]);
            }
        }
        VideoData::Rgb(pixels) => {
            for pixel in pixels.get(..count * 3)?.as_chunks::<3>().0 {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
        }
    }
    Some(rgba)
}

#[must_use]
pub fn aspect_of(data: VideoData<'_>, width: u16, height: u16) -> f32 {
    match data {
        VideoData::Rgb(_) if height > 0 => f32::from(width) / f32::from(height),
        _ => DISPLAY_ASPECT,
    }
}

#[must_use]
pub fn caption(geometry: Option<(u16, u16)>, live: bool) -> String {
    match geometry {
        None => "waiting for sync".to_owned(),
        Some((width, height)) if live => format!("{width} \u{d7} {height}"),
        Some((width, height)) => format!("{width} \u{d7} {height} \u{b7} no sync"),
    }
}

#[derive(Default)]
pub struct Picture {
    width: u32,
    height: u32,
    aspect: f32,
    rgba: Vec<u8>,
    stamp: u64,
}

impl Scene for Picture {
    fn stamp(&self) -> u64 {
        self.stamp
    }

    fn paint(&mut self, raster: &mut Raster) {
        if self.rgba.is_empty() {
            raster.fill([0, 0, 0, 255]);
            return;
        }
        raster.resize(self.width, self.height);
        raster.pixels.copy_from_slice(&self.rgba);
        raster.aspect = Some(self.aspect);
    }
}

pub fn face(store: Store, node: String) -> impl IntoView {
    install_stylesheet("wp11-video", SHEET);
    let inputs = Signal::derive(move || binding::sources_of(&store.graph.get(), &node, "video"));
    view! {
        column(class = "face") {
            if move || inputs.get().is_empty() {
                text(class = "hint") {"Wire a video channel's picture in"}
            }
            for source in move || inputs.get(), key = |source: &String| source.clone() {
                {screen(store, source)}
            }
        }
    }
}

fn screen(store: Store, source: String) -> impl IntoView {
    let target = Memo::new(move |_| {
        let set = store.device_set_of(&source)?;
        Some((set, store.channel_of(&source)?.id))
    });
    move || {
        target
            .get()
            .map(|(set, channel)| AnyView::new(watch(store, set, channel)))
    }
}

fn watch(store: Store, device_set: u32, channel: u32) -> impl IntoView {
    let picture = Rc::new(RefCell::new(Picture::default()));
    let geometry = RwSignal::new(None::<(u16, u16)>);
    let live = RwSignal::new(false);
    let seen = Rc::new(RefCell::new(None::<Instant>));
    store.hold(ClientCommand::SubscribeVideo {
        device_set,
        channel,
    });
    let shown = picture.clone();
    let heard = seen.clone();
    store.on_frame(move |frame| {
        if !matches!(frame.kind, FrameKind::VideoGray | FrameKind::VideoRgb)
            || store.source_of(frame.stream_id)
                != Some(Source::Video {
                    device_set,
                    channel,
                })
        {
            return;
        }
        let Some(decoded) = VideoFrame::decode(&frame.bytes) else {
            tracing::warn!(device_set, channel, "a video frame did not decode");
            return;
        };
        let Some(rgba) = rgba_of(decoded.data, decoded.width, decoded.height) else {
            tracing::warn!(device_set, channel, "a video frame was cut short");
            return;
        };
        if let Ok(mut picture) = shown.try_borrow_mut() {
            picture.width = u32::from(decoded.width);
            picture.height = u32::from(decoded.height);
            picture.aspect = aspect_of(decoded.data, decoded.width, decoded.height);
            picture.rgba = rgba;
            picture.stamp += 1;
        }
        *heard.borrow_mut() = Some(Instant::now());
        if geometry.get_untracked() != Some((decoded.width, decoded.height)) {
            geometry.set(Some((decoded.width, decoded.height)));
        }
        if !live.get_untracked() {
            live.set(true);
        }
    });
    let check = set_interval(STALE_CHECK, move || {
        let fresh = seen.borrow().is_some_and(|at| at.elapsed() < STALE);
        if live.get_untracked() != fresh {
            live.set(fresh);
        }
    });
    on_cleanup_local(move || drop(check));
    view! {
        column(class = "vid") {
            {raster_view("vid__screen", picture)}
            text(class = "legend") {{move || caption(geometry.get(), live.get())}}
        }
    }
}

const SHEET: &str = css!(
    r#"
.vid { gap: 4px; }
.vid__screen { width: 100%; height: 220px; border-radius: 2px; }
"#
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grey_picture_is_spread_over_all_three_colours() {
        let rgba = rgba_of(VideoData::Gray(&[10, 20]), 2, 1).expect("a picture");
        assert_eq!(rgba, vec![10, 10, 10, 255, 20, 20, 20, 255]);
    }

    #[test]
    fn a_colour_picture_keeps_its_colours() {
        let rgba = rgba_of(VideoData::Rgb(&[1, 2, 3]), 1, 1).expect("a picture");
        assert_eq!(rgba, vec![1, 2, 3, 255]);
    }

    #[test]
    fn a_picture_shorter_than_its_size_is_refused() {
        assert_eq!(rgba_of(VideoData::Gray(&[1, 2, 3]), 2, 2), None);
        assert_eq!(rgba_of(VideoData::Rgb(&[1, 2, 3]), 2, 1), None);
    }

    #[test]
    fn analogue_television_is_shown_at_four_by_three() {
        assert!((aspect_of(VideoData::Gray(&[]), 400, 300) - DISPLAY_ASPECT).abs() < 1e-6);
        assert!((aspect_of(VideoData::Gray(&[]), 100, 100) - DISPLAY_ASPECT).abs() < 1e-6);
        assert!((aspect_of(VideoData::Rgb(&[]), 320, 160) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn the_caption_says_whether_the_picture_is_still_coming() {
        assert_eq!(caption(None, false), "waiting for sync");
        assert_eq!(caption(Some((320, 240)), true), "320 \u{d7} 240");
        assert_eq!(
            caption(Some((320, 240)), false),
            "320 \u{d7} 240 \u{b7} no sync"
        );
    }

    #[test]
    fn a_picture_paints_at_its_own_size_and_aspect() {
        let mut picture = Picture {
            width: 2,
            height: 1,
            aspect: 2.0,
            rgba: vec![1, 2, 3, 255, 4, 5, 6, 255],
            stamp: 1,
        };
        let mut raster = Raster::sized(10, 10);
        picture.paint(&mut raster);
        assert_eq!((raster.width, raster.height), (2, 1));
        assert_eq!(raster.aspect, Some(2.0));
        assert_eq!(raster.at(1, 0), Some([4, 5, 6, 255]));
    }
}
