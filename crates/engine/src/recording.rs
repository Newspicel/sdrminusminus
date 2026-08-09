//! The lossless IQ recording pipeline (PLAN §5, §7): a DSP-thread tap Arc-copies each
//! drained slice into a bounded queue feeding a dedicated SigMF writer thread. Unlike the
//! audio path's drop-oldest contract, backpressure here is a hard fault — a full queue (or
//! a dead writer) disarms the tap and surfaces one error, so a recording never has silent
//! holes.

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

use crate::EngineError;

/// Whole drained slices queue per send, so the headroom before a stalled writer becomes a
/// surfaced overflow fault is cap × drain cadence: ~1.6 s at the virtual device's 25 ms
/// blocks, but only ~0.13 s at a real backend's ~2 ms hot-loop drain.
const REC_CHANNEL_CAP: usize = 64;

/// What [`crate::Engine::stop_recording`] hands back for indexing (PLAN §11: the files are
/// the source of truth; the server upserts this into its recordings index).
#[derive(Clone, Debug)]
pub struct FinalizedRecording {
    /// Directory-joined extension-less stem, exactly as `sdrmm_recorder::scan_stems` lists it.
    pub stem: PathBuf,
    /// RFC3339 UTC.
    pub started_at: String,
    pub samples: u64,
    pub bytes: u64,
    /// Capture-ring drops while the recording ran — loss upstream of the DSP plane; the file
    /// itself is contiguous as the DSP thread saw the stream.
    pub overruns: u64,
    /// The fault that ended the writer, if any; the pair may then be unfinalized (breadcrumb
    /// meta only) and is never listed.
    pub error: Option<String>,
}

/// Counters and fault state shared between the DSP tap, the writer thread, and the control
/// plane; readable without any lock so snapshots never wait on a busy writer.
#[derive(Debug, Default)]
pub(crate) struct RecordingShared {
    samples: AtomicU64,
    bytes: AtomicU64,
    error: OnceLock<String>,
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

    /// First fault wins: a queue overflow that follows a write error must not mask the cause.
    /// `pub(crate)` so engine tests can inject a writer fault (no honest disk fault is
    /// portably inducible on a live writer).
    pub(crate) fn fail(&self, message: String) {
        let _ = self.error.set(message);
    }
}

/// One drained DSP slice bound for the writer. `start_sample` is the DSP thread's total
/// sample clock (ring drops included), so a gap between blocks marks an upstream overrun.
pub(crate) struct RecBlock {
    start_sample: u64,
    center_hz: f64,
    samples: Arc<[Complex<f32>]>,
}

/// DSP-thread side of the pipeline. `push` is hot-path: one `Arc` copy plus a non-blocking
/// bounded send per drained slice — the sanctioned PCM hand-off precedent (PLAN §7).
pub(crate) struct RecorderTap {
    tx: mpsc::SyncSender<RecBlock>,
    shared: Arc<RecordingShared>,
}

impl RecorderTap {
    /// Hand a slice to the writer. `false` means the recording failed (queue overflow or
    /// writer death): the cause is already in the shared state and the caller must disarm —
    /// continuing would write a file with silent holes.
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
        match self.tx.try_send(block) {
            Ok(()) => true,
            Err(mpsc::TrySendError::Full(_)) => {
                self.shared
                    .fail("recording queue overflow — disk too slow?".to_string());
                false
            }
            // The writer only drops the receiver on its own fault, which it reports first;
            // this fallback covers a panicked writer.
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.shared.fail("recording writer stopped".to_string());
                false
            }
        }
    }
}

pub(crate) fn create_tap() -> (RecorderTap, mpsc::Receiver<RecBlock>, Arc<RecordingShared>) {
    let (tx, rx) = mpsc::sync_channel(REC_CHANNEL_CAP);
    let shared = Arc::new(RecordingShared::default());
    (
        RecorderTap {
            tx,
            shared: shared.clone(),
        },
        rx,
        shared,
    )
}

/// The writer thread. Constructed control-side so spawn errors surface on the REST call; it
/// exits when the tap is gone — dropping the tap is the stop handshake — or on the first
/// write fault, finalizing either way so already-captured data survives as a playable pair.
pub(crate) fn spawn_writer(
    writer: SigmfWriter,
    blocks: mpsc::Receiver<RecBlock>,
    shared: Arc<RecordingShared>,
) -> Result<JoinHandle<()>, EngineError> {
    std::thread::Builder::new()
        .name("sdrmm-rec".to_string())
        .spawn(move || write_loop(writer, &blocks, &shared))
        .map_err(|e| EngineError::RecordingIo(format!("spawn recording writer thread: {e}")))
}

fn write_loop(
    mut writer: SigmfWriter,
    blocks: &mpsc::Receiver<RecBlock>,
    shared: &RecordingShared,
) {
    let mut center = writer.meta().captures.last().and_then(|c| c.frequency);
    let mut next_sample: Option<u64> = None;
    while let Ok(block) = blocks.recv() {
        if let Some(expected) = next_sample
            && block.start_sample != expected
        {
            // Loss upstream of the DSP plane, already surfaced as `DeviceSet.overruns` and
            // `RecordingStatus.overruns`; the file stays contiguous as the DSP thread saw it.
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

/// Create the writer under a dir-unique stem `rec_<ds>_<compact UTC timestamp>` (suffixed
/// `-2`, `-3`, … on a same-second collision), returning it with the claimed file name.
/// Claiming is atomic — [`SigmfWriter::create`] opens `create_new` and reports a taken stem
/// as [`SigmfError::StemTaken`] — so concurrent starts can never share or truncate one
/// another's files; probing `exists()` here would be a TOCTOU. The one non-atomic probe is
/// the final meta: a meta-only stem (data file removed by hand) must not be reclaimed, or
/// finalize would rename over the surviving meta — and final metas appear only via that
/// rename, never mid-race.
pub(crate) fn create_writer(
    dir: &Path,
    ds: u32,
    started_at: jiff::Timestamp,
    sample_rate: f64,
    center_hz: f64,
    hw: &str,
) -> Result<(SigmfWriter, String), EngineError> {
    let base = format!("rec_{ds}_{}", started_at.strftime("%Y%m%dT%H%M%SZ"));
    let mut name = base.clone();
    let mut n = 1u32;
    loop {
        let stem = dir.join(&name);
        if !meta_path(&stem).exists() {
            match SigmfWriter::create(&stem, sample_rate, center_hz, hw) {
                Ok(writer) => return Ok((writer, name)),
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
        let (tap, blocks, shared) = create_tap();
        let handle = spawn_writer(writer, blocks, shared.clone()).unwrap();

        let samples = block(16);
        assert!(tap.push(&samples, 0, 100_000_000.0));
        assert!(tap.push(&samples, 16, 101_000_000.0));
        assert!(tap.push(&[], 32, 101_000_000.0), "empty slices are skipped");
        drop(tap);
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
    fn full_queue_surfaces_overflow_instead_of_dropping() {
        // No writer thread: the queue backs up exactly like a wedged disk would make it.
        let (tap, _blocks, shared) = create_tap();
        let samples = block(4);
        for i in 0..REC_CHANNEL_CAP as u64 {
            assert!(tap.push(&samples, i * 4, 1_000_000.0));
        }
        assert!(!tap.push(&samples, REC_CHANNEL_CAP as u64 * 4, 1_000_000.0));
        assert!(shared.error().unwrap().contains("overflow"));
    }

    #[test]
    fn dead_writer_surfaces_instead_of_dropping() {
        let (tap, blocks, shared) = create_tap();
        drop(blocks);
        assert!(!tap.push(&block(4), 0, 1_000_000.0));
        assert_eq!(shared.error().as_deref(), Some("recording writer stopped"));
    }

    #[test]
    fn create_writer_advances_the_suffix_on_claimed_stems() {
        let dir = TempDir::new().unwrap();
        let ts = jiff::Timestamp::UNIX_EPOCH;
        let (first, name) = create_writer(dir.path(), 3, ts, 48_000.0, 1_000_000.0, "hw").unwrap();
        assert_eq!(name, "rec_3_19700101T000000Z");
        let base_stem = first.stem().to_path_buf();

        // The in-flight first attempt (breadcrumb + data, no final meta) claims its stem.
        let (second, name) = create_writer(dir.path(), 3, ts, 48_000.0, 1_000_000.0, "hw").unwrap();
        assert_eq!(name, "rec_3_19700101T000000Z-2");

        // A finalized pair and a crashed attempt (breadcrumb left behind) both claim theirs.
        first.finalize().unwrap();
        drop(second);
        let (_third, name) = create_writer(dir.path(), 3, ts, 48_000.0, 1_000_000.0, "hw").unwrap();
        assert_eq!(name, "rec_3_19700101T000000Z-3");

        // A meta-only stem (data file removed by hand) is never reclaimed: finalize would
        // rename over the surviving meta.
        std::fs::remove_file(data_path(&base_stem)).unwrap();
        let (_fourth, name) =
            create_writer(dir.path(), 3, ts, 48_000.0, 1_000_000.0, "hw").unwrap();
        assert_eq!(name, "rec_3_19700101T000000Z-4");
    }
}
