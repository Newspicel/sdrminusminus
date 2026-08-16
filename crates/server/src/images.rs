use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

use axum::body::Bytes;
use sdrmm_engine::{Engine, ImageCapture, VideoPicture};
use sdrmm_wire::{CapturedImage, EventImage, ServerEvent, StateScope};
use tokio::{
    sync::broadcast::error::RecvError,
    time::{MissedTickBehavior, interval},
};

const RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
const RECONCILE_INTERVAL: Duration = Duration::from_secs(60);
const MAX_STORED_IMAGES: usize = 512;
const MAX_STORED_BYTES: usize = 256 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct Images {
    inner: Mutex<StoredImages>,
}

#[derive(Default)]
struct StoredImages {
    next_id: u64,
    images: VecDeque<StoredImage>,
    bytes: usize,
}

struct StoredImage {
    image: CapturedImage,
    png: Option<Bytes>,
    expires: Instant,
}

impl Images {
    pub(crate) fn list(&self) -> Vec<CapturedImage> {
        let mut inner = self.lock();
        prune(&mut inner);
        inner
            .images
            .iter()
            .rev()
            .map(|item| item.image.clone())
            .collect()
    }

    pub(crate) fn png(&self, id: u64) -> Option<Bytes> {
        let mut inner = self.lock();
        prune(&mut inner);
        inner
            .images
            .iter()
            .find(|item| item.image.id == id)
            .and_then(|item| item.png.clone())
    }

    fn expire(&self) -> bool {
        let mut inner = self.lock();
        let before = inner.images.len();
        prune(&mut inner);
        inner.images.len() != before
    }

    fn push(&self, capture: &ImageCapture, png: Result<Bytes, String>) -> (CapturedImage, bool) {
        let mut inner = self.lock();
        prune(&mut inner);
        inner.next_id += 1;
        let id = inner.next_id;
        let (png, error) = match png {
            Ok(bytes) => (Some(bytes), None),
            Err(message) => (None, Some(message)),
        };
        let image = CapturedImage {
            id,
            device_set: capture.device_set,
            channel: capture.channel,
            at: capture.at.clone(),
            freq_hz: capture.freq_hz,
            source: capture.source.to_owned(),
            mode: capture.mode.clone(),
            width: capture.picture.width,
            height: capture.picture.height,
            lines: capture.lines,
            complete: capture.complete,
            image: png.as_ref().map(|_| EventImage {
                url: crate::rest::captured_image_path(id),
                media_type: "image/png".to_owned(),
            }),
            image_error: error,
        };
        inner.bytes += png.as_ref().map_or(0, Bytes::len);
        inner.images.push_back(StoredImage {
            image: image.clone(),
            png,
            expires: Instant::now() + RETENTION,
        });
        while inner.images.len() > MAX_STORED_IMAGES {
            let dropped = inner.images.pop_front();
            inner.bytes -= dropped
                .and_then(|item| item.png)
                .as_ref()
                .map_or(0, Bytes::len);
        }
        let evicted = evict(&mut inner);
        (image, evicted)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StoredImages> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn prune(inner: &mut StoredImages) {
    let now = Instant::now();
    let mut freed = 0;
    inner.images.retain(|item| {
        let keep = item.expires > now;
        if !keep {
            freed += item.png.as_ref().map_or(0, Bytes::len);
        }
        keep
    });
    inner.bytes -= freed;
}

fn evict(inner: &mut StoredImages) -> bool {
    let mut evicted = false;
    for item in &mut inner.images {
        if inner.bytes <= MAX_STORED_BYTES {
            break;
        }
        let Some(png) = item.png.take() else {
            continue;
        };
        inner.bytes -= png.len();
        item.image.image = None;
        item.image
            .image_error
            .get_or_insert_with(|| "picture evicted by the temporary buffer limit".to_owned());
        evicted = true;
    }
    evicted
}

pub(crate) fn encode_png(picture: &VideoPicture) -> Result<Bytes, String> {
    let width = u32::from(picture.width);
    let height = u32::from(picture.height);
    if width == 0 || height == 0 {
        return Err("picture has no pixels".to_owned());
    }
    let pixels = (width as usize) * (height as usize);
    let (color, data) = if picture.rgb.len() == pixels * 3 {
        (png::ColorType::Rgb, picture.rgb.as_slice())
    } else if picture.luma.len() == pixels {
        (png::ColorType::Grayscale, picture.luma.as_slice())
    } else {
        return Err(format!(
            "picture claims {width}×{height} but carries {} rgb and {} luma bytes",
            picture.rgb.len(),
            picture.luma.len()
        ));
    };
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(color);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|err| format!("png header: {err}"))?;
    writer
        .write_image_data(data)
        .map_err(|err| format!("png data: {err}"))?;
    writer
        .finish()
        .map_err(|err| format!("png finish: {err}"))?;
    Ok(Bytes::from(out))
}

pub(crate) async fn run(engine: Weak<Engine>, images: Arc<Images>) {
    let Some(strong) = engine.upgrade() else {
        return;
    };
    let mut captures = strong.subscribe_images();
    drop(strong);
    let mut ticker = interval(RECONCILE_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            received = captures.recv() => match received {
                Ok(capture) => store(&capture, &images, &engine),
                Err(RecvError::Lagged(count)) => {
                    tracing::warn!(count, "captured pictures dropped: the store fell behind");
                }
                Err(RecvError::Closed) => break,
            },
            _ = ticker.tick() => {
                if images.expire() && let Some(strong) = engine.upgrade() {
                    strong.emit_scope(StateScope::Images);
                }
            }
        }
    }
}

fn store(capture: &ImageCapture, images: &Images, engine: &Weak<Engine>) {
    let png = encode_png(&capture.picture);
    if let Err(error) = &png {
        tracing::error!(
            error,
            source = capture.source,
            "a captured picture was lost"
        );
    }
    let (image, evicted) = images.push(capture, png);
    let Some(engine) = engine.upgrade() else {
        return;
    };
    engine.emit_event(ServerEvent::ImageCaptured(Box::new(image)));
    if evicted {
        engine.emit_scope(StateScope::Images);
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_device::DeviceRegistry;

    use super::*;

    fn picture(width: u16, height: u16) -> VideoPicture {
        let pixels = usize::from(width) * usize::from(height);
        VideoPicture {
            width,
            height,
            luma: vec![128; pixels],
            rgb: (0..pixels * 3).map(|i| (i % 251) as u8).collect(),
        }
    }

    fn capture(width: u16, height: u16) -> ImageCapture {
        ImageCapture {
            device_set: 1,
            channel: 2,
            at: "2026-08-16T10:00:00Z".to_owned(),
            freq_hz: 14_230_000.0,
            source: "sstv",
            mode: "Martin M1".to_owned(),
            complete: true,
            lines: height,
            picture: Arc::new(picture(width, height)),
        }
    }

    #[test]
    fn a_png_carries_the_signature_and_the_stated_size() {
        let bytes = encode_png(&picture(320, 256)).expect("encodes");
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&bytes[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 320);
        assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 256);
        assert_eq!(bytes[25], png::ColorType::Rgb as u8);
    }

    #[test]
    fn a_luma_only_picture_encodes_as_greyscale() {
        let mut only_luma = picture(64, 32);
        only_luma.rgb.clear();
        let bytes = encode_png(&only_luma).expect("encodes");
        assert_eq!(bytes[25], png::ColorType::Grayscale as u8);
    }

    #[test]
    fn a_picture_whose_pixels_do_not_match_its_size_is_refused() {
        let mut broken = picture(64, 32);
        broken.rgb.truncate(7);
        broken.luma.truncate(3);
        let err = encode_png(&broken).expect_err("refuses");
        assert!(err.contains("64×32"), "{err}");
        assert!(encode_png(&picture(0, 0)).is_err());
    }

    #[test]
    fn a_stored_picture_is_listed_with_a_url_to_its_png() {
        let images = Images::default();
        let (stored, _) = images.push(&capture(320, 256), encode_png(&picture(320, 256)));
        assert_eq!(stored.id, 1);
        assert_eq!(stored.source, "sstv");
        assert_eq!(stored.mode, "Martin M1");
        assert_eq!((stored.width, stored.height), (320, 256));
        assert_eq!(
            stored.image.as_ref().map(|image| image.url.as_str()),
            Some("/api/images/1/png")
        );
        assert!(stored.image_error.is_none());
        assert_eq!(images.list().len(), 1);
        let png = images.png(1).expect("png");
        assert_eq!(&png[..4], b"\x89PNG");
        assert!(images.png(2).is_none());
    }

    #[test]
    fn a_picture_that_failed_to_encode_is_kept_and_says_why() {
        let images = Images::default();
        let (stored, _) = images.push(&capture(320, 256), Err("no pixels".to_owned()));
        assert!(stored.image.is_none());
        assert_eq!(stored.image_error.as_deref(), Some("no pixels"));
        assert_eq!(images.list().len(), 1);
    }

    #[test]
    fn evicting_pixels_keeps_the_record_and_says_why() {
        let images = Images::default();
        let big = Bytes::from(vec![0u8; MAX_STORED_BYTES / 2 + 1]);
        let mut evictions = 0;
        for _ in 0..3 {
            let (_, evicted) = images.push(&capture(320, 256), Ok(big.clone()));
            evictions += usize::from(evicted);
        }
        assert!(evictions >= 1, "eviction was never reported");
        let listed = images.list();
        assert_eq!(listed.len(), 3);
        assert!(
            listed
                .iter()
                .filter(|image| image.image.is_none())
                .all(|image| image.image_error.is_some())
        );
        assert!(images.lock().bytes <= MAX_STORED_BYTES);
    }

    #[test]
    fn the_store_keeps_only_the_newest_pictures() {
        let images = Images::default();
        for _ in 0..MAX_STORED_IMAGES + 4 {
            images.push(&capture(8, 8), encode_png(&picture(8, 8)));
        }
        let listed = images.list();
        assert_eq!(listed.len(), MAX_STORED_IMAGES);
        assert_eq!(listed[0].id, (MAX_STORED_IMAGES + 4) as u64);
    }

    #[tokio::test]
    async fn a_capture_reaches_the_store_and_announces_itself() {
        let engine = Engine::with_registry(DeviceRegistry::new(), None);
        let images = Arc::new(Images::default());
        let mut events = engine.subscribe_events();
        store(&capture(64, 48), &images, &Arc::downgrade(&engine));
        let listed = images.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].lines, 48);
        let announced = events.try_recv().expect("event");
        let ServerEvent::ImageCaptured(image) = announced else {
            panic!("unexpected event {announced:?}");
        };
        assert_eq!(image.id, listed[0].id);
    }
}
