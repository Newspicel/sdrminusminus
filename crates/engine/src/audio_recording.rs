use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
};

use sdrmm_recorder::{AUDIO_SUFFIX, AudioWriter};

use crate::{
    EngineError,
    audio::{PcmBlock, PcmPayload},
};

/// PCM blocks queue whole, so the headroom before a stalled writer becomes a surfaced fault is
/// cap × block cadence: about a second and a half at the ~25 ms blocks a channel produces.
const AUDIO_REC_CHANNEL_CAP: usize = 64;

/// The counters and the one fault a live audio recording publishes. Read from the control side
/// while the writer thread owns the file.
#[derive(Debug, Default)]
pub(crate) struct AudioRecordingShared {
    frames: AtomicU64,
    bytes: AtomicU64,
    error: OnceLock<String>,
}

impl AudioRecordingShared {
    pub(crate) fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    pub(crate) fn error(&self) -> Option<String> {
        self.error.get().cloned()
    }

    /// First fault wins, so a queue overflow behind a write error cannot mask the cause.
    pub(crate) fn fail(&self, message: String) {
        let _ = self.error.set(message);
    }
}

/// The DSP-thread end of a channel audio recording. Handed to the host through the lane's
/// command queue; the queue closes — which is the finalize handshake — once *every* clone of it
/// is gone. It is clonable because the control side keeps one: a channel rebuilt for a new rate
/// or a new mode gets a fresh host, and the recording has to carry across that swap.
#[derive(Clone)]
pub(crate) struct AudioRecorderTap {
    tx: mpsc::SyncSender<PcmBlock>,
    shared: Arc<AudioRecordingShared>,
}

impl AudioRecorderTap {
    /// Hand one block of the channel's audio to the writer. `false` means the recording has
    /// failed and the caller must disarm: carrying on would write a file with holes in it that
    /// nothing downstream could tell from silence on the air.
    #[must_use]
    pub(crate) fn push(&self, block: PcmBlock) -> bool {
        match self.tx.try_send(block) {
            Ok(()) => true,
            Err(mpsc::TrySendError::Full(_)) => {
                self.shared
                    .fail("audio recording queue overflow — disk too slow?".to_string());
                false
            }
            // The writer drops its receiver only on its own fault, which it reports first; this
            // covers a panicked writer.
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.shared
                    .fail("audio recording writer stopped".to_string());
                false
            }
        }
    }
}

pub(crate) fn create_tap() -> (
    AudioRecorderTap,
    mpsc::Receiver<PcmBlock>,
    Arc<AudioRecordingShared>,
) {
    let (tx, rx) = mpsc::sync_channel(AUDIO_REC_CHANNEL_CAP);
    let shared = Arc::new(AudioRecordingShared::default());
    (
        AudioRecorderTap {
            tx,
            shared: shared.clone(),
        },
        rx,
        shared,
    )
}

/// The writer thread. Built control-side so a spawn failure surfaces on the REST call; it exits
/// when the tap is gone or at the first fault, finalizing either way so what was already
/// captured stays a playable file.
pub(crate) fn spawn_writer(
    writer: AudioWriter,
    blocks: mpsc::Receiver<PcmBlock>,
    shared: Arc<AudioRecordingShared>,
) -> Result<JoinHandle<()>, EngineError> {
    std::thread::Builder::new()
        .name("sdrmm-audiorec".to_string())
        .spawn(move || write_loop(writer, &blocks, &shared))
        .map_err(|e| EngineError::RecordingIo(format!("spawn audio recording thread: {e}")))
}

fn write_loop(
    mut writer: AudioWriter,
    blocks: &mpsc::Receiver<PcmBlock>,
    shared: &AudioRecordingShared,
) {
    let mut next_frame: Option<u64> = None;
    while let Ok(block) = blocks.recv() {
        // A WAV states its channel count once, in a header the audio already written sits
        // behind. A mode switched to a different layout mid-recording is therefore the end of
        // this file rather than something to convert on the way past.
        if block.channels != writer.channels() {
            shared.fail(format!(
                "the channel switched to {}-channel audio; a WAV cannot change layout mid-file",
                block.channels
            ));
            break;
        }
        let frames = match &block.payload {
            PcmPayload::Samples(samples) => samples.len() / usize::from(block.channels),
            PcmPayload::Silence(frames) => *frames,
        };
        // Stamps are the channel's own frame clock, so a jump means PCM was lost on the way
        // here. Padding keeps the recording's timeline honest — a minute in is a minute in —
        // and the gap is reported rather than left to look like quiet air.
        if let Some(expected) = next_frame
            && block.start_frame > expected
        {
            let missing = block.start_frame - expected;
            tracing::warn!(missing, "audio recording padding pcm lost upstream");
            if let Err(e) = writer.write_silence(missing as usize) {
                shared.fail(format!("audio recording write failed: {e}"));
                break;
            }
        }
        next_frame = Some(block.start_frame + frames as u64);
        let written = match &block.payload {
            PcmPayload::Samples(samples) => writer.write_frames(samples),
            PcmPayload::Silence(frames) => writer.write_silence(*frames),
        };
        if let Err(e) = written {
            shared.fail(format!("audio recording write failed: {e}"));
            break;
        }
        shared
            .frames
            .store(writer.frames_written(), Ordering::Relaxed);
        shared
            .bytes
            .store(writer.bytes_written(), Ordering::Relaxed);
    }
    if let Err(e) = writer.finalize() {
        shared.fail(format!("audio recording finalize failed: {e}"));
    }
}

/// Where audio recordings live: beside the IQ library rather than in it, since a directory of
/// SigMF pairs is also the list of replayable devices and a WAV is not one of them.
#[must_use]
pub fn audio_dir(recordings_dir: &Path) -> PathBuf {
    recordings_dir.join("audio")
}

/// Open the file one recording writes into, claiming a name nothing else holds. `ds` and `ch`
/// are in the name because a listener looking at a directory of these has nothing else to tell
/// two channels of the same radio apart by.
pub(crate) fn create_writer(
    dir: &Path,
    ds: u32,
    ch: u32,
    started_at: jiff::Timestamp,
    sample_rate: u32,
    channels: u8,
) -> Result<AudioWriter, EngineError> {
    let base = format!("ch_{ds}_{ch}_{}", started_at.strftime("%Y%m%dT%H%M%SZ"));
    let mut name = format!("{base}{AUDIO_SUFFIX}");
    let mut n = 1u32;
    loop {
        match AudioWriter::create(&dir.join(&name), sample_rate, channels) {
            Ok(writer) => return Ok(writer),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                return Err(EngineError::RecordingIo(format!(
                    "create {}: {e}",
                    dir.join(&name).display()
                )));
            }
        }
        n += 1;
        name = format!("{base}-{n}{AUDIO_SUFFIX}");
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_recorder::read_audio_info;
    use tempfile::TempDir;

    use super::*;

    const RATE: u32 = 48_000;

    fn samples(start_frame: u64, channels: u8, frames: usize) -> PcmBlock {
        PcmBlock {
            start_frame,
            channels,
            payload: PcmPayload::Samples(vec![0.5; frames * usize::from(channels)].into()),
        }
    }

    fn silence(start_frame: u64, channels: u8, frames: usize) -> PcmBlock {
        PcmBlock {
            start_frame,
            channels,
            payload: PcmPayload::Silence(frames),
        }
    }

    #[test]
    fn the_writer_lays_down_audio_and_squelched_silence_alike() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("rec.wav");
        let writer = AudioWriter::create(&path, RATE, 1).expect("create");
        let (tap, blocks, shared) = create_tap();
        let handle = spawn_writer(writer, blocks, shared.clone()).expect("spawn");

        assert!(tap.push(samples(0, 1, 480)));
        assert!(tap.push(silence(480, 1, 480)));
        drop(tap);
        handle.join().expect("join");

        assert_eq!(shared.frames(), 960);
        assert_eq!(shared.bytes(), 1_920);
        assert_eq!(shared.error(), None);
        let info = read_audio_info(&path).expect("info");
        assert_eq!((info.channels, info.frames), (1, 960));
    }

    /// A WAV header states one layout. The recording ends where the channel stops matching it,
    /// with the reason kept rather than frames a reader would misinterpret.
    #[test]
    fn a_layout_change_ends_the_recording_and_says_why() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("layout.wav");
        let writer = AudioWriter::create(&path, RATE, 1).expect("create");
        let (tap, blocks, shared) = create_tap();
        let handle = spawn_writer(writer, blocks, shared.clone()).expect("spawn");

        assert!(tap.push(samples(0, 1, 480)));
        assert!(tap.push(samples(480, 2, 480)));
        drop(tap);
        handle.join().expect("join");

        assert!(
            shared.error().expect("fault").contains("layout"),
            "{:?}",
            shared.error()
        );
        let info = read_audio_info(&path).expect("info");
        assert_eq!(
            info.frames, 480,
            "the mono audio before the switch survives"
        );
    }

    /// PCM lost on the way to the writer must not shorten the recording: what is written stays
    /// as long as the air time it covers.
    #[test]
    fn a_gap_in_the_stamps_is_padded_rather_than_spliced_out() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("gap.wav");
        let writer = AudioWriter::create(&path, RATE, 1).expect("create");
        let (tap, blocks, shared) = create_tap();
        let handle = spawn_writer(writer, blocks, shared.clone()).expect("spawn");

        assert!(tap.push(samples(0, 1, 480)));
        assert!(tap.push(samples(1_440, 1, 480)));
        drop(tap);
        handle.join().expect("join");

        assert_eq!(shared.frames(), 1_920);
        assert_eq!(shared.error(), None);
    }

    #[test]
    fn a_full_queue_surfaces_overflow_instead_of_dropping_audio() {
        // No writer thread, so the queue backs up exactly as a wedged disk would make it.
        let (tap, _blocks, shared) = create_tap();
        for i in 0..AUDIO_REC_CHANNEL_CAP as u64 {
            assert!(tap.push(samples(i * 480, 1, 480)));
        }
        assert!(!tap.push(samples(0, 1, 480)));
        assert!(shared.error().expect("fault").contains("overflow"));
    }

    #[test]
    fn a_dead_writer_surfaces_instead_of_dropping_audio() {
        let (tap, blocks, shared) = create_tap();
        drop(blocks);
        assert!(!tap.push(samples(0, 1, 480)));
        assert_eq!(
            shared.error().as_deref(),
            Some("audio recording writer stopped")
        );
    }

    #[test]
    fn a_claimed_name_advances_the_suffix() {
        let dir = TempDir::new().expect("tempdir");
        let at = jiff::Timestamp::UNIX_EPOCH;
        let names: Vec<String> = (0..3)
            .map(|_| {
                let writer = create_writer(dir.path(), 2, 7, at, RATE, 1).expect("create");
                let name = writer
                    .path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .expect("name")
                    .to_owned();
                writer.finalize().expect("finalize");
                name
            })
            .collect();
        assert_eq!(
            names,
            [
                "ch_2_7_19700101T000000Z.wav",
                "ch_2_7_19700101T000000Z-2.wav",
                "ch_2_7_19700101T000000Z-3.wav"
            ]
        );
    }
}
