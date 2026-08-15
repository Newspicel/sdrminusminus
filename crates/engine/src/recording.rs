use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
};

use num_complex::Complex;
use sdrmm_recorder::{SigmfError, SigmfWriter, meta_path};
use sdrmm_wire::PositionFix;

use crate::EngineError;

const REC_CHANNEL_CAP: usize = 64;

#[derive(Clone, Debug)]
pub struct FinalizedRecording {
    pub stem: PathBuf,
    pub stream: u32,
    pub started_at: String,
    pub samples: u64,
    pub bytes: u64,
    pub overruns: u64,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct RecordingShared {
    samples: AtomicU64,
    bytes: AtomicU64,
    error: OnceLock<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct RecordingPosition {
    tx: mpsc::SyncSender<RecMessage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PositionUpdateError {
    Full,
    Disconnected,
}

impl RecordingPosition {
    pub(crate) fn update(&self, fix: Option<PositionFix>) -> Result<(), PositionUpdateError> {
        self.tx
            .try_send(RecMessage::Position(fix.map(Box::new)))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => PositionUpdateError::Full,
                mpsc::TrySendError::Disconnected(_) => PositionUpdateError::Disconnected,
            })
    }
}

impl RecordingShared {
    pub(crate) fn samples(&self) -> u64 {
        self.samples.load(Ordering::Relaxed)
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    pub(crate) fn error(&self) -> Option<String> {
        self.error.get().cloned()
    }

    pub(crate) fn fail(&self, message: String) {
        let _ = self.error.set(message);
    }
}

#[derive(Debug)]
pub(crate) struct RecBlock {
    start_sample: u64,
    center_hz: f64,
    samples: Arc<[Complex<f32>]>,
}

#[derive(Debug)]
pub(crate) enum RecMessage {
    Block(RecBlock),
    Position(Option<Box<PositionFix>>),
}

#[derive(Clone)]
pub(crate) struct RecorderTap {
    tx: mpsc::SyncSender<RecMessage>,
    shared: Arc<RecordingShared>,
}

impl RecorderTap {
    #[must_use]
    pub(crate) fn push(&self, slice: &[Complex<f32>], start_sample: u64, center_hz: f64) -> bool {
        if slice.is_empty() {
            return true;
        }
        let block = RecBlock {
            start_sample,
            center_hz,
            samples: Arc::from(slice),
        };
        match self.tx.try_send(RecMessage::Block(block)) {
            Ok(()) => true,
            Err(mpsc::TrySendError::Full(_)) => {
                self.shared
                    .fail("recording queue overflow — disk too slow?".to_string());
                false
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.shared.fail("recording writer stopped".to_string());
                false
            }
        }
    }
}

pub(crate) fn create_tap() -> (
    RecorderTap,
    RecordingPosition,
    mpsc::Receiver<RecMessage>,
    Arc<RecordingShared>,
) {
    let (tx, rx) = mpsc::sync_channel(REC_CHANNEL_CAP);
    let shared = Arc::new(RecordingShared::default());
    let position = RecordingPosition { tx: tx.clone() };
    (
        RecorderTap {
            tx,
            shared: shared.clone(),
        },
        position,
        rx,
        shared,
    )
}

pub(crate) fn spawn_writer(
    writer: SigmfWriter,
    messages: mpsc::Receiver<RecMessage>,
    shared: Arc<RecordingShared>,
) -> Result<JoinHandle<()>, EngineError> {
    std::thread::Builder::new()
        .name("sdrmm-rec".to_string())
        .spawn(move || write_loop(writer, &messages, &shared))
        .map_err(|e| EngineError::RecordingIo(format!("spawn recording writer thread: {e}")))
}

fn write_loop(
    mut writer: SigmfWriter,
    messages: &mpsc::Receiver<RecMessage>,
    shared: &RecordingShared,
) {
    let mut center = writer.meta().captures.last().and_then(|c| c.frequency);
    let mut next_sample: Option<u64> = None;
    while let Ok(message) = messages.recv() {
        let block = match message {
            RecMessage::Block(block) => block,
            RecMessage::Position(fix) => {
                writer.set_position(fix.as_deref());
                continue;
            }
        };
        if let Some(expected) = next_sample
            && block.start_sample != expected
        {
            tracing::debug!(
                gap = block.start_sample.saturating_sub(expected),
                "recording spans a capture ring overrun"
            );
        }
        next_sample = Some(block.start_sample + block.samples.len() as u64);
        if center != Some(block.center_hz) {
            writer.add_capture(block.center_hz);
            center = Some(block.center_hz);
        }
        if let Err(e) = writer.write_block(&block.samples) {
            shared.fail(format!("recording write failed: {e}"));
            break;
        }
        shared
            .samples
            .store(writer.samples_written(), Ordering::Relaxed);
        shared
            .bytes
            .store(writer.bytes_written(), Ordering::Relaxed);
    }
    if let Err(e) = writer.finalize() {
        shared.fail(format!("recording finalize failed: {e}"));
    }
}

pub(crate) fn create_writer(
    dir: &Path,
    prefix: &str,
    stream: u32,
    started_at: jiff::Timestamp,
    sample_rate: f64,
    center_hz: f64,
    hw: &str,
) -> Result<(SigmfWriter, String), EngineError> {
    let base = format!("{prefix}_{}", started_at.strftime("%Y%m%dT%H%M%SZ"));
    let mut name = base.clone();
    let mut n = 1u32;
    loop {
        let stem = dir.join(&name);
        if !meta_path(&stem).exists() {
            match SigmfWriter::create(&stem, sample_rate, center_hz, hw) {
                Ok(mut writer) => {
                    writer.set_rx_stream(stream);
                    return Ok((writer, name));
                }
                Err(SigmfError::StemTaken(_)) => {}
                Err(e) => {
                    return Err(EngineError::RecordingIo(format!(
                        "create {}: {e}",
                        stem.display()
                    )));
                }
            }
        }
        n += 1;
        name = format!("{base}-{n}");
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_recorder::{SigmfReader, data_path};
    use tempfile::TempDir;

    use super::*;

    fn block(n: usize) -> Vec<Complex<f32>> {
        (0..n)
            .map(|i| Complex::new(i as f32, -(i as f32)))
            .collect()
    }

    #[test]
    fn writer_appends_capture_segments_and_finalizes_on_tap_drop() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("rec");
        let writer = SigmfWriter::create(&stem, 48_000.0, 100_000_000.0, "hw").unwrap();
        let (tap, position, messages, shared) = create_tap();
        let handle = spawn_writer(writer, messages, shared.clone()).unwrap();

        let samples = block(16);
        assert!(tap.push(&samples, 0, 100_000_000.0));
        assert!(tap.push(&samples, 16, 101_000_000.0));
        assert!(tap.push(&[], 32, 101_000_000.0), "empty slices are skipped");
        drop(tap);
        drop(position);
        handle.join().unwrap();

        assert_eq!(shared.samples(), 32);
        assert_eq!(shared.bytes(), 32 * sdrmm_recorder::BYTES_PER_SAMPLE);
        assert_eq!(shared.error(), None);
        let reader = SigmfReader::open(&stem).unwrap();
        assert_eq!(reader.total_samples(), 32);
        let captures = &reader.meta().captures;
        assert_eq!(captures.len(), 2);
        assert_eq!(captures[1].sample_start, 16);
        assert_eq!(captures[1].frequency, Some(101_000_000.0));
    }

    #[test]
    fn writer_geotags_the_sample_where_a_live_fix_arrives() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("mobile");
        let writer = SigmfWriter::create(&stem, 48_000.0, 100_000_000.0, "hw").unwrap();
        let (tap, position, messages, shared) = create_tap();
        let handle = spawn_writer(writer, messages, shared).unwrap();
        position
            .update(Some(PositionFix {
                latitude: 52.52,
                longitude: 13.405,
                altitude_m: Some(40.0),
                accuracy_m: Some(3.0),
                speed_mps: None,
                track_deg: None,
                time: "2026-08-14T12:00:00Z".to_owned(),
            }))
            .unwrap();
        assert!(tap.push(&block(16), 0, 100_000_000.0));
        drop(tap);
        drop(position);
        handle.join().unwrap();

        let reader = SigmfReader::open(&stem).unwrap();
        let capture = &reader.meta().captures[0];
        assert_eq!(
            capture.geolocation.as_ref().unwrap().coordinates,
            vec![13.405, 52.52, 40.0]
        );
    }

    #[test]
    fn queued_iq_before_a_fix_is_not_backdated() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("ordered-position");
        let writer = SigmfWriter::create(&stem, 48_000.0, 100_000_000.0, "hw").unwrap();
        let (tap, position, messages, shared) = create_tap();
        let handle = spawn_writer(writer, messages, shared).unwrap();
        assert!(tap.push(&block(16), 0, 100_000_000.0));
        position
            .update(Some(PositionFix {
                latitude: 52.52,
                longitude: 13.405,
                altitude_m: None,
                accuracy_m: None,
                speed_mps: None,
                track_deg: None,
                time: "2026-08-14T12:00:01Z".to_owned(),
            }))
            .unwrap();
        assert!(tap.push(&block(16), 16, 100_000_000.0));
        drop(tap);
        drop(position);
        handle.join().unwrap();

        let reader = SigmfReader::open(&stem).unwrap();
        assert_eq!(reader.meta().captures.len(), 2);
        assert_eq!(reader.meta().captures[0].geolocation, None);
        assert_eq!(reader.meta().captures[1].sample_start, 16);
        assert_eq!(
            reader.meta().captures[1]
                .geolocation
                .as_ref()
                .unwrap()
                .coordinates,
            vec![13.405, 52.52]
        );
    }

    #[test]
    fn full_queue_surfaces_overflow_instead_of_dropping() {
        let (tap, position, _messages, shared) = create_tap();
        let samples = block(4);
        for i in 0..REC_CHANNEL_CAP as u64 {
            assert!(tap.push(&samples, i * 4, 1_000_000.0));
        }
        assert_eq!(position.update(None), Err(PositionUpdateError::Full));
        assert!(!tap.push(&samples, REC_CHANNEL_CAP as u64 * 4, 1_000_000.0));
        assert!(shared.error().unwrap().contains("overflow"));
    }

    #[test]
    fn dead_writer_surfaces_instead_of_dropping() {
        let (tap, position, messages, shared) = create_tap();
        drop(position);
        drop(messages);
        assert!(!tap.push(&block(4), 0, 1_000_000.0));
        assert_eq!(shared.error().as_deref(), Some("recording writer stopped"));
    }

    #[test]
    fn create_writer_advances_the_suffix_on_claimed_stems() {
        let dir = TempDir::new().unwrap();
        let ts = jiff::Timestamp::UNIX_EPOCH;
        let (first, name) =
            create_writer(dir.path(), "rec_3", 0, ts, 48_000.0, 1_000_000.0, "hw").unwrap();
        assert_eq!(name, "rec_3_19700101T000000Z");
        let base_stem = first.stem().to_path_buf();

        let (second, name) =
            create_writer(dir.path(), "rec_3", 0, ts, 48_000.0, 1_000_000.0, "hw").unwrap();
        assert_eq!(name, "rec_3_19700101T000000Z-2");

        first.finalize().unwrap();
        drop(second);
        let (_third, name) =
            create_writer(dir.path(), "rec_3", 0, ts, 48_000.0, 1_000_000.0, "hw").unwrap();
        assert_eq!(name, "rec_3_19700101T000000Z-3");

        std::fs::remove_file(data_path(&base_stem)).unwrap();
        let (_fourth, name) =
            create_writer(dir.path(), "rec_3", 0, ts, 48_000.0, 1_000_000.0, "hw").unwrap();
        assert_eq!(name, "rec_3_19700101T000000Z-4");
    }
}
