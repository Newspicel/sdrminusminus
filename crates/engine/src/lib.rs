//! `sdrmm-engine` — the flowgraph runtime (PLAN §2, §7). Owns the authoritative device-set
//! state, hosts each device set's capture + DSP threads plus per-channel Opus encoders and
//! an optional SigMF recording writer (see [`recording`]), and pushes `StateChanged` events,
//! spectrum snapshots, and audio packets outward. The control
//! plane (this facade) uses a mutex; the DSP plane (see [`runtime`]) is lock-free and never
//! shares mutable state with it directly — channel changes cross over via a command queue.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration,
};

use sdrmm_channels::{ChannelCtx, ChannelError};
use sdrmm_device::{DeviceError, DeviceRegistry, check_stream_settings};
use sdrmm_device_virtual::VirtualDriver;
use sdrmm_recorder::{data_path, meta_path};
use sdrmm_wire::{
    Capabilities, ChannelDescriptor, ChannelInfo, ChannelParams, ChannelSettings, DecodedRecord,
    DeviceInfo, DeviceSet, DeviceSetStatus, DeviceSettings, RecordingStatus, ScanSettings,
    ScannerStatus, ServerEvent, StateScope, StateSnapshot,
};
use tokio::sync::broadcast;

pub mod audio;
pub mod recording;
pub mod runtime;
pub mod scanner;
pub use audio::AudioPacket;
pub use recording::FinalizedRecording;
pub use runtime::{SpectrumSnapshot, adaptive_db_window};

use crate::{
    audio::PcmBlock,
    recording::RecordingShared,
    runtime::{CaptureRuntime, ChannelHost, DecodedSink, DspCommand, RawDecoded},
    scanner::{ScanPlan, ScannerState},
};

/// Merge priority for the built-in virtual driver (native backends register higher, PLAN §6).
const VIRTUAL_PRIORITY: u8 = 10;
/// Soapy sits above virtual.
#[cfg(feature = "soapy")]
const SOAPY_PRIORITY: u8 = 20;
/// Native backends win the serial merge against Soapy for the same physical device (PLAN §6):
/// they expose what Soapy hides — direct sampling, bias-T, per-stage gain — and they need no
/// C library to be installed.
#[cfg(any(feature = "rtl-native", feature = "hackrf-native"))]
const NATIVE_PRIORITY: u8 = 30;
const EVENT_CHANNEL_CAP: usize = 256;
/// Decoder frames buffered between the DSP plane and the stamping pump. Deep enough to
/// absorb an ADS-B burst; overflow is counted and reported, never silently swallowed.
const DECODED_QUEUE_CAP: usize = 4096;
/// Fan-out depth for stamped decoder records. Consumers that fall behind lag the broadcast
/// (drop-oldest) rather than stalling the pump — the UI path is lossy by design (PLAN §5).
const DECODED_CHANNEL_CAP: usize = 1024;
/// Fallbacks for devices that report no tuning/rate; mirrored wherever settings are read.
const DEFAULT_CENTER_HZ: f64 = 100_000_000.0;
const DEFAULT_SAMPLE_RATE: f64 = 2_048_000.0;

/// The driver registry every binary gets: the virtual driver plus whichever hardware backends
/// this build compiled in (PLAN §6 merge priority — native above Soapy above virtual). Split
/// out of [`Engine::new`] so `sdrmm --doctor` reports the same set the server would open,
/// rather than forking the registration policy.
#[must_use]
pub fn builtin_registry(recordings_dir: Option<PathBuf>) -> DeviceRegistry {
    let mut registry = DeviceRegistry::new();
    let virtual_driver = match recordings_dir {
        Some(dir) => VirtualDriver::with_recordings(dir),
        None => VirtualDriver::new(),
    };
    registry.register(VIRTUAL_PRIORITY, Box::new(virtual_driver));
    #[cfg(feature = "soapy")]
    registry.register(
        SOAPY_PRIORITY,
        Box::new(sdrmm_device_soapy::SoapyDriver::new()),
    );
    #[cfg(feature = "rtl-native")]
    registry.register(
        NATIVE_PRIORITY,
        Box::new(sdrmm_device_rtlsdr::RtlSdrDriver::new()),
    );
    #[cfg(feature = "hackrf-native")]
    registry.register(
        NATIVE_PRIORITY,
        Box::new(sdrmm_device_hackrf::HackRfDriver::new()),
    );
    registry
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("device set {0} not found")]
    DeviceSetNotFound(u32),
    #[error("channel {0} not found in device set {1}")]
    ChannelNotFound(u32, u32),
    /// A stream index past the device's lane count — a bad request naming the count, never a
    /// panic and never a silent fallback to stream 0 (design §6).
    #[error("stream {stream} is out of range: this device has {streams} rx streams")]
    StreamOutOfRange { stream: u32, streams: u32 },
    #[error(transparent)]
    Device(#[from] DeviceError),
    #[error(transparent)]
    Channel(#[from] ChannelError),
    #[error("audio pipeline: {0}")]
    Audio(String),
    #[error("recording: {0}")]
    Recording(String),
    /// Server-side I/O failure on the recording path (disk full, unwritable dir): mapped to
    /// 500, unlike [`EngineError::Recording`]'s client mistakes.
    #[error("recording: {0}")]
    RecordingIo(String),
    #[error("scan: {0}")]
    Scan(String),
}

impl EngineError {
    /// Maps to HTTP 404 (missing device set / channel / device).
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::DeviceSetNotFound(_) | Self::ChannelNotFound(..))
            || matches!(self, Self::Device(DeviceError::NotFound(_)))
    }

    /// Maps to HTTP 400 (a well-formed request the device or channel validation rejected).
    #[must_use]
    pub fn is_bad_request(&self) -> bool {
        matches!(
            self,
            Self::Device(
                DeviceError::Unsupported(_)
                    | DeviceError::AlreadyStreaming
                    | DeviceError::DuplexConflict { .. },
            ) | Self::Channel(_)
                | Self::Recording(_)
                | Self::Scan(_)
                | Self::StreamOutOfRange { .. }
        )
    }
}

/// A hosted channel queued for a pipeline rebuild swap: the settings snapshot it was listed
/// under, plus the audio identity (PCM sender + shared sample position) the replacement host
/// must reuse so the audio stream and its timestamps survive the swap.
struct RebuildEntry {
    id: u32,
    /// The rx stream the channel taps; the replacement host must land on the same lane.
    stream: u32,
    settings: ChannelSettings,
    pcm_tx: broadcast::Sender<PcmBlock>,
    pcm_pos: Arc<AtomicU64>,
}

fn sample_rate_of(settings: &DeviceSettings) -> f64 {
    settings.sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE)
}

fn descriptor_for(params: &ChannelParams) -> Result<ChannelDescriptor, EngineError> {
    let type_id = params.type_id();
    sdrmm_channels::descriptors()
        .into_iter()
        .find(|d| d.type_id == type_id)
        .ok_or_else(|| ChannelError::UnknownType(type_id.to_owned()).into())
}

/// A channel's occupied band must fit inside the device passband and its DDC must decimate
/// (never interpolate); non-finite knobs are rejected before they can reach the DSP plane.
/// The band comes from the configured params ([`sdrmm_channels::occupied_band`]), not the
/// descriptor nominal — the nominal would pass configs (wide NFM, SSB's one-sided band)
/// whose real occupancy sticks out past Nyquist and silently truncates the audio.
fn validate_channel(
    descriptor: &ChannelDescriptor,
    settings: &ChannelSettings,
    device_rate: f64,
) -> Result<(), EngineError> {
    if !settings.offset_hz.is_finite() {
        return Err(ChannelError::InvalidSettings(format!(
            "offset_hz must be finite, got {}",
            settings.offset_hz
        ))
        .into());
    }
    if let Some(db) = settings.squelch_db
        && !db.is_finite()
    {
        return Err(
            ChannelError::InvalidSettings(format!("squelch_db must be finite, got {db}")).into(),
        );
    }
    if device_rate < descriptor.input_rate_hz {
        return Err(ChannelError::InvalidSettings(format!(
            "{} needs a device rate of at least {} Hz, device runs at {device_rate} Hz",
            descriptor.type_id, descriptor.input_rate_hz
        ))
        .into());
    }
    let (low, high) = sdrmm_channels::occupied_band(&settings.params);
    let band_low = settings.offset_hz + low;
    let band_high = settings.offset_hz + high;
    let nyquist = device_rate / 2.0;
    if band_low < -nyquist || band_high > nyquist {
        return Err(ChannelError::InvalidSettings(format!(
            "channel band [{band_low}, {band_high}] Hz exceeds the ±{nyquist} Hz device passband"
        ))
        .into());
    }
    // A native-rate channel takes the device's samples as they are, so there is no DDC
    // conversion to refuse — only a ceiling, because the scan costs a magnitude per sample and
    // the Pi 4 is the budget floor (PLAN §18, amended).
    if let Some((low, high)) = descriptor.native_rate_range() {
        if device_rate > high {
            return Err(ChannelError::InvalidSettings(format!(
                "{} reads the radio's own samples, so it runs with the receiver between \
                 {:.3} and {:.3} MHz — above that there is nothing left for a slicer to gain \
                 and the scan costs more than the smallest machine this has to run on can \
                 spare. The receiver is at {:.3} MHz.",
                descriptor.name,
                low / 1e6,
                high / 1e6,
                device_rate / 1e6,
            ))
            .into());
        }
        return Ok(());
    }
    // A rate conversion needs a guard band for its filter transition, so a channel that
    // occupies its full output rate can only be served by a transparent DDC — no mode is in
    // that position today (ADS-B was, and reads the device rate directly instead), but the
    // check stays: it is the thing that would otherwise fail silently.
    if device_rate != descriptor.input_rate_hz {
        let widest = sdrmm_dsp::resamplable_bandwidth_hz(descriptor.input_rate_hz);
        if high - low >= widest {
            return Err(ChannelError::InvalidSettings(format!(
                "{} fills its whole {:.3} MHz channel, so there is no guard band left for a \
                 resampler to filter in — at {:.3} MHz the signal would arrive smeared and \
                 decode nothing. Set the receiver to exactly {:.3} MHz.",
                descriptor.name,
                (high - low) / 1e6,
                device_rate / 1e6,
                descriptor.input_rate_hz / 1e6,
            ))
            .into());
        }
    }
    Ok(())
}

/// Whether the radio's advertised ranges reach `hz`. No ranges means unconstrained — the
/// same reading as `DeviceProfile::reaches`, so the engine and the picker cannot disagree.
fn tuner_reaches(capabilities: &Capabilities, hz: f64) -> bool {
    capabilities.freq_ranges.is_empty()
        || capabilities
            .freq_ranges
            .iter()
            .any(|r| hz >= r.min && hz <= r.max)
}

/// Refuse a per-stream delta the capability cannot honour — an entry for a stream the radio
/// lacks, or for a setting it does not scope per-stream — before any device I/O (design §4).
/// The backends refuse the same things through the shared [`check_stream_settings`], but the
/// refusal must come back as the engine's bad request naming the problem, not as an apply
/// failure after other fields already reached the device. The range check mirrors the
/// radio-wide dial's: a per-stream tuner is still this tuner.
fn validate_streams(
    capabilities: &Capabilities,
    delta: &DeviceSettings,
) -> Result<(), EngineError> {
    check_stream_settings(delta, capabilities)?;
    for entry in &delta.streams {
        if let Some(hz) = entry.center_hz
            && !tuner_reaches(capabilities, hz)
        {
            return Err(DeviceError::Unsupported(format!(
                "streams[{}].center_hz: {hz} Hz is outside this device's tuning range",
                entry.stream
            ))
            .into());
        }
    }
    Ok(())
}

/// A channel's audio identity: the PCM fan-in and Opus fan-out survive pipeline rebuilds
/// (params type change, device rate change), so audio subscribers never notice a swap.
struct ChannelAudio {
    pcm_tx: broadcast::Sender<PcmBlock>,
    /// 48 kHz-domain position of the channel's next PCM sample, shared into every host
    /// built for this channel so packet timestamps stay continuous across rebuilds.
    pcm_pos: Arc<AtomicU64>,
    audio_tx: broadcast::Sender<AudioPacket>,
    encoder: Option<std::thread::JoinHandle<()>>,
}

impl ChannelAudio {
    /// `channels` is the layout the channel starts in; the encoder follows the PCM from there,
    /// so a later stereo toggle needs no new identity.
    fn new(channels: u8) -> Result<Self, EngineError> {
        let (pcm_tx, pcm_rx) = broadcast::channel(audio::PCM_CHANNEL_CAP);
        let (audio_tx, _) = broadcast::channel(audio::AUDIO_CHANNEL_CAP);
        let encoder = audio::spawn_encoder(channels, pcm_rx, audio_tx.clone())?;
        Ok(Self {
            pcm_tx,
            pcm_pos: Arc::new(AtomicU64::new(0)),
            audio_tx,
            encoder: Some(encoder),
        })
    }

    /// Joins the encoder thread. The caller must already have arranged for the DSP-side PCM
    /// sender clone to drop (`RemoveChannel` sent, or the runtime stopped) or this blocks.
    fn shutdown(mut self) {
        let encoder = self.encoder.take();
        drop(self);
        if let Some(handle) = encoder
            && handle.join().is_err()
        {
            tracing::error!("opus encoder thread panicked");
        }
    }
}

/// Control-plane handle to a live recording: the writer thread plus the shared state the
/// DSP tap and the writer update, projected into `DeviceSet.recording`.
struct RecordingState {
    /// Stem file name (no directory, no `.sigmf-*` suffix) — what clients display.
    file: String,
    /// The rx stream being recorded; the stop command must reach this lane's tap.
    stream: u32,
    /// RFC3339 UTC.
    started_at: String,
    /// Directory-joined stem, kept for the finalized handoff to the server's index.
    stem: PathBuf,
    shared: Arc<RecordingShared>,
    writer: JoinHandle<()>,
    /// `DeviceSet.overruns` when the recording armed; the difference is the drops the
    /// recording spans (loss upstream of the DSP plane, PLAN §5).
    overruns_at_start: u64,
    /// Counter/fault values already surfaced to clients; the hotplug tick diffs against them.
    samples_seen: u64,
    error_seen: bool,
}

impl RecordingState {
    fn status(&self, overruns_now: u64) -> RecordingStatus {
        RecordingStatus {
            file: self.file.clone(),
            stream: self.stream,
            started_at: self.started_at.clone(),
            samples: self.shared.samples(),
            bytes: self.shared.bytes(),
            overruns: overruns_now - self.overruns_at_start,
            error: self.shared.error(),
        }
    }

    /// Joins the writer thread. The caller must already have arranged for the DSP-side tap
    /// to drop (`StopRecording` queued, or the runtime stopped) or this blocks.
    fn join(self) {
        join_recording_writer(self.writer);
    }
}

/// Per-device-set control-plane state plus its running capture (PLAN §7). The runtime owns the
/// device and DSP thread; the rest is the serializable projection sent to clients.
struct DeviceSetState {
    info: DeviceInfo,
    capabilities: Capabilities,
    settings: DeviceSettings,
    status: DeviceSetStatus,
    channels: Vec<ChannelInfo>,
    /// Audio plumbing per channel id, kept beside the projected `channels` so rebuild swaps
    /// can preserve a channel's streams while replacing its DSP pipeline.
    audio: HashMap<u32, ChannelAudio>,
    next_channel_id: u32,
    error: Option<String>,
    recording: Option<RecordingState>,
    /// Running frequency scan (M5). The scan thread drives this set's centre frequency, so
    /// while it is present client retunes are refused rather than fought over.
    scanner: Option<ScannerState>,
    /// In-flight `patch_device` calls that will change the sample rate (pre-validated, device
    /// I/O or merge still pending). `start_recording`'s commit refuses while non-zero: a
    /// recording committed inside that window would pin a rate the patch is about to change —
    /// the reverse ordering of the recording guard in the patch pre-validation. Cleared by
    /// [`RatePatchGuard`] on every patch exit path.
    rate_patches: u32,
    /// Clones of the runtime's per-stream DSP command queues, index = stream. Channel
    /// commands go through these while holding the engine `inner` lock with this set's entry
    /// present — that ordering is what keeps DSP-plane channel membership consistent with
    /// control-plane state (a removal or swap can never interleave into a stale rebuild and
    /// re-add a deleted channel, which would strand a live PCM sender and hang the encoder
    /// join). `mpsc` sends never block, so sending under `inner` is safe; the
    /// never-hold-both rule below concerns only the `runtime` mutex, which these sends never
    /// touch.
    cmd_txs: Vec<mpsc::Sender<DspCommand>>,
    /// Per-lane capture-ring drop counters shared with the runtime, index = stream;
    /// readable without its lock so snapshots never wait on a wedged device.
    overruns: Vec<Arc<AtomicU64>>,
    /// Overrun count already surfaced to clients; the hotplug tick diffs against it.
    overruns_seen: u64,
    /// Control-plane mutex around the runtime: device I/O (`apply`, `stop`) happens under it,
    /// never under the engine-wide `inner` lock, so a wedged device stalls only its own set.
    /// Never hold `inner` and this lock at once — clone the `Arc` under `inner`, drop `inner`,
    /// then lock. The DSP hot path never touches this mutex.
    runtime: Arc<Mutex<CaptureRuntime>>,
}

impl DeviceSetState {
    fn project(&self, id: u32) -> DeviceSet {
        let overruns = self.overruns_total();
        DeviceSet {
            id,
            // Identity only: the radio is open, so `capabilities` below is what it actually
            // reports, and the probe-time profile beside it would be a second answer to the
            // same question.
            device: self.info.identity(),
            capabilities: self.capabilities.clone(),
            settings: self.settings.clone(),
            status: self.status,
            channels: self.channels.clone(),
            overruns,
            error: self.error.clone(),
            recording: self.recording.as_ref().map(|r| r.status(overruns)),
            scanner: self.scanner.as_ref().map(ScannerState::status),
        }
    }

    /// How many rx streams this set's runtime hosts — the bound every stream-taking call is
    /// checked against, and the count its refusal names.
    fn rx_streams(&self) -> u32 {
        self.cmd_txs.len() as u32
    }

    /// `Ok(())` iff `stream` addresses one of this set's lanes.
    fn check_stream(&self, stream: u32) -> Result<(), EngineError> {
        if stream < self.rx_streams() {
            Ok(())
        } else {
            Err(EngineError::StreamOutOfRange {
                stream,
                streams: self.rx_streams(),
            })
        }
    }

    /// `DeviceSet.overruns` stays one number: the set-wide sum (per-lane counts exist for
    /// per-lane sample clocks, not for the projection).
    fn overruns_total(&self) -> u64 {
        self.overruns
            .iter()
            .map(|counter| counter.load(Ordering::Relaxed))
            .sum()
    }

    /// Queue a DSP command on `stream`'s lane. Callers hold `inner` with this set still
    /// listed, so the DSP threads are alive (`remove_device_set` unlists the set before
    /// stopping it), and they derive `stream` from state committed under `inner` — a missing
    /// lane or a closed queue here is an engine bug and is surfaced rather than swallowed.
    fn send_dsp(&self, stream: u32, cmd: DspCommand) {
        match self.cmd_txs.get(stream as usize) {
            Some(cmd_tx) => {
                if cmd_tx.send(cmd).is_err() {
                    tracing::error!(
                        stream,
                        "dsp command queue closed while its device set is still listed"
                    );
                }
            }
            None => tracing::error!(
                stream,
                streams = self.rx_streams(),
                "dsp command for a stream this device set does not have"
            ),
        }
    }
}

/// Where a fault from a reconnect's fresh capture goes. Before the swap installs the new
/// runtime the fault has nowhere to be applied (the set still describes the old, dead one), so
/// it is parked; afterwards it takes the normal fault path like any other.
enum FaultGate {
    Pending(Option<DeviceError>),
    Armed,
}

/// Who is retuning a device set. A scan owns its set's centre frequency, so its own retunes
/// must skip the guard that keeps clients from fighting it — and must stay silent, because a
/// `StateChanged` per scan step would cost every client a full-state refetch several times a
/// second (progress rides [`ServerEvent::ScannerUpdate`] instead).
#[derive(Clone, Copy, PartialEq, Eq)]
enum PatchOrigin {
    Client,
    Scan,
}

/// Clears a device set's `rate_patches` claim on drop, so no `patch_device` exit path
/// (device-apply failure, set removal, success) can leave the claim stuck and block every
/// future `start_recording` on the set. Must never be dropped while `inner` is held.
struct RatePatchGuard<'a> {
    engine: &'a Engine,
    ds: u32,
}

impl Drop for RatePatchGuard<'_> {
    fn drop(&mut self) {
        let mut inner = self.engine.lock();
        if let Some(state) = inner.device_sets.get_mut(&self.ds) {
            state.rate_patches = state.rate_patches.saturating_sub(1);
        }
    }
}

#[derive(Default)]
struct Inner {
    device_sets: BTreeMap<u32, DeviceSetState>,
    /// Ids reserved by `create_device_set` but not yet inserted into `device_sets`. A fault
    /// arriving in that window is stashed in `pending_faults` instead of dropped; ids are
    /// never reused, so membership is unambiguous.
    creating: HashSet<u32>,
    pending_faults: HashMap<u32, DeviceError>,
    next_ds_id: u32,
    revision: u64,
}

/// The engine. All methods are `&self`; constructors hand back an `Arc` because the fault
/// drainer (and the optional hotplug prober) hold `Weak` back-references into it.
pub struct Engine {
    registry: DeviceRegistry,
    inner: Mutex<Inner>,
    event_tx: broadcast::Sender<ServerEvent>,
    /// Cloned into each capture runtime's fatal handler; the fault drainer holds the receiver.
    fault_tx: mpsc::Sender<(u32, DeviceError)>,
    /// Cloned into every hosted channel's [`DecodedSink`]; the pump holds the receiver.
    decoded_tx: mpsc::SyncSender<RawDecoded>,
    /// Frames the DSP plane could not hand over because [`DECODED_QUEUE_CAP`] was full.
    decoded_dropped: Arc<AtomicU64>,
    /// Stamped decoder records, fanned out to the WS hub and the decoder-log writer.
    decoded_tx_out: broadcast::Sender<DecodedRecord>,
    /// Where `start_recording` writes SigMF pairs; `None` disables recording (PLAN §11).
    recordings_dir: Option<PathBuf>,
}

impl Engine {
    /// Build the engine with the built-in drivers registered (virtual always; native backends
    /// join here as their milestones land, PLAN §16). `recordings_dir` is both where
    /// `start_recording` writes and what the virtual driver scans for playback devices, so
    /// a finalized recording is immediately replayable.
    #[must_use]
    pub fn new(recordings_dir: Option<PathBuf>) -> Arc<Self> {
        let registry = builtin_registry(recordings_dir.clone());
        Self::with_registry(registry, recordings_dir)
    }

    /// Build the engine over a caller-supplied registry, so tests can register mock drivers
    /// (and point `recordings_dir` at a scoped temp dir).
    #[must_use]
    pub fn with_registry(registry: DeviceRegistry, recordings_dir: Option<PathBuf>) -> Arc<Self> {
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAP);
        let (fault_tx, fault_rx) = mpsc::channel();
        let (decoded_tx, decoded_rx) = mpsc::sync_channel(DECODED_QUEUE_CAP);
        let (decoded_tx_out, _) = broadcast::channel(DECODED_CHANNEL_CAP);
        let engine = Arc::new(Self {
            registry,
            inner: Mutex::new(Inner::default()),
            event_tx,
            fault_tx,
            decoded_tx,
            decoded_dropped: Arc::new(AtomicU64::new(0)),
            decoded_tx_out,
            recordings_dir,
        });
        engine.spawn_fault_drainer(fault_rx);
        engine.spawn_decoded_pump(decoded_rx);
        engine
    }

    /// Stamp decoder frames with wall-clock time and fan them out (PLAN §5). Runs off the
    /// DSP thread so no decoder ever formats a timestamp on the hot path. Holds a `Weak`,
    /// and exits once every sink sender is gone (engine dropped, DSP threads joined).
    fn spawn_decoded_pump(self: &Arc<Self>, decoded_rx: mpsc::Receiver<RawDecoded>) {
        let weak = Arc::downgrade(self);
        let spawned = std::thread::Builder::new()
            .name("sdrmm-decoded".to_string())
            .spawn(move || {
                let mut lost_seen = 0u64;
                while let Ok(raw) = decoded_rx.recv() {
                    let Some(engine) = weak.upgrade() else { return };
                    // Fixed nanosecond precision, not `Timestamp::to_string()`: that trims
                    // trailing fractional zeros, so `12:00:00.5Z` sorts before `12:00:00Z`.
                    // The log's index is a text comparison over exactly this string, and the
                    // live record a client sees must be byte-identical to the stored one.
                    let at = format!("{:.9}", jiff::Timestamp::now());
                    // send() only errors with no subscribers — the common headless case.
                    let _ = engine.decoded_tx_out.send(DecodedRecord {
                        device_set: raw.device_set,
                        channel: raw.channel,
                        at,
                        freq_hz: raw.freq_hz,
                        event: raw.event,
                    });
                    // Report queue overflow from here rather than the DSP thread, which must
                    // not emit: the count is cumulative, so one event per growth suffices.
                    let lost = engine.decoded_dropped.load(Ordering::Relaxed);
                    if lost > lost_seen {
                        let count = lost - lost_seen;
                        lost_seen = lost;
                        tracing::warn!(count, "decoder frames dropped: control plane behind");
                        engine.emit(ServerEvent::DecodedLost { count });
                    }
                }
            });
        if let Err(e) = spawned {
            tracing::error!("failed to spawn decoder pump: {e}");
        }
    }

    /// Subscribe to stamped decoder frames (PLAN §5). Lossy by design: a subscriber that
    /// falls behind lags the broadcast rather than stalling the DSP plane.
    #[must_use]
    pub fn subscribe_decoded(&self) -> broadcast::Receiver<DecodedRecord> {
        self.decoded_tx_out.subscribe()
    }

    /// Cumulative decoder frames dropped before reaching the pump (PLAN §5: surfaced loss).
    #[must_use]
    pub fn decoded_dropped(&self) -> u64 {
        self.decoded_dropped.load(Ordering::Relaxed)
    }

    /// A decoder-frame outlet bound to one channel, handed to its [`ChannelHost`].
    fn decoded_sink(&self, ds: u32, channel: u32) -> DecodedSink {
        DecodedSink::new(
            self.decoded_tx.clone(),
            self.decoded_dropped.clone(),
            ds,
            channel,
        )
    }

    /// The directory `start_recording` writes into (PLAN §11: files on disk are the source
    /// of truth); the server's recordings index scans this same directory.
    #[must_use]
    pub fn recordings_dir(&self) -> Option<&Path> {
        self.recordings_dir.as_deref()
    }

    /// Serialize capture-thread faults into state changes. Holds only a `Weak` so it never
    /// keeps a dropped engine alive; it exits once every fault sender is gone (engine dropped,
    /// all capture threads joined).
    fn spawn_fault_drainer(self: &Arc<Self>, fault_rx: mpsc::Receiver<(u32, DeviceError)>) {
        let weak = Arc::downgrade(self);
        let spawned = std::thread::Builder::new()
            .name("sdrmm-fault".to_string())
            .spawn(move || {
                while let Ok((ds, err)) = fault_rx.recv() {
                    let Some(engine) = weak.upgrade() else { return };
                    engine.mark_device_fault(ds, err);
                }
            });
        if let Err(e) = spawned {
            // Without the drainer, device faults would be lost; say so loudly.
            tracing::error!("failed to spawn fault drainer: {e}");
        }
    }

    /// A capture thread died (PLAN §16 M1 hotplug robustness): keep the set listed but flag it
    /// so clients render the failure. Joins nothing — `remove_device_set` may concurrently be
    /// joining the very capture thread that raised this fault, and it needs the lock released
    /// (its take-out-then-drop pattern) while we only mutate under it.
    fn mark_device_fault(&self, ds: u32, err: DeviceError) {
        let mut inner = self.lock();
        if let Some(state) = inner.device_sets.get_mut(&ds) {
            state.status = DeviceSetStatus::Error;
            state.error = Some(err.to_string());
            // A dead capture feeds no more samples, so finalize any live recording now: the
            // data captured so far becomes a playable pair instead of a dangling breadcrumb.
            // The DSP thread is still alive (only removal stops it), so the queued command
            // is guaranteed to drop the tap and the join below cannot hang.
            let recording = state.recording.take();
            if let Some(recording) = &recording {
                state.send_dsp(recording.stream, DspCommand::StopRecording);
            }
            // A dead device accepts no more retunes, so the scan is over; take it here and
            // join outside the lock — the scan thread takes `inner` on every step.
            let scanner = state.scanner.take();
            let runtime = state.runtime.clone();
            inner.revision += 1;
            drop(inner);
            if let Some(scanner) = scanner {
                scanner.stop_and_join();
            }
            // Release the dead device. Its capture thread is already gone, but a USB backend
            // holds its interface claim until the handle drops — and auto-reconnect re-opens
            // this exact radio, so keeping it would make every replug recovery fail "busy".
            // Outside `inner`, like every other runtime lock (the per-set lock rule).
            lock_runtime(&runtime).stop();
            if let Some(recording) = recording {
                recording.join();
                // The implicit stop just finalized a playable pair; without this scope
                // nothing ever invalidates the recordings library for a fault-stopped
                // recording (clients never poll).
                self.emit(ServerEvent::StateChanged {
                    scope: StateScope::Recordings,
                });
            }
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(ds),
            });
        } else if inner.creating.contains(&ds) {
            // The capture died before `create_device_set` could insert the set; stash the
            // fault so the insert applies it instead of leaving a dead set Running.
            inner.pending_faults.insert(ds, err);
        } else {
            drop(inner);
            // The fault raced a removal; the set is already gone, so log instead of dropping
            // the error on the floor.
            tracing::warn!(ds, error = %err, "fault for removed device set");
        }
    }

    /// Poll for attach/detach every `interval` (PLAN §16 M1): a changed probe result pushes
    /// `StateChanged{Devices}` so clients refetch `GET /api/devices`. Opt-in per binary — unit
    /// tests must never race a background prober. Holds a `Weak`; the thread exits at the
    /// first wake after the engine is dropped.
    pub fn start_hotplug_prober(self: &Arc<Self>, interval: Duration) -> std::io::Result<()> {
        let weak = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("sdrmm-hotplug".to_string())
            .spawn(move || {
                let mut known = None;
                let mut missing_once = HashSet::new();
                loop {
                    let Some(engine) = weak.upgrade() else { return };
                    engine.hotplug_tick(&mut known, &mut missing_once);
                    drop(engine);
                    std::thread::sleep(interval);
                }
            })?;
        Ok(())
    }

    /// One prober step, split from the thread so tests drive it without sleeping: probe, diff
    /// against `known`, emit on change. The first probe is the baseline and never emits.
    /// `probe_all` output is id-sorted, so plain `Vec` equality is order-insensitive.
    ///
    /// The probe also guards running sets against silent unplug: Soapy backends cannot detect
    /// it from inside the capture thread (their hardware-key reads do no live I/O), so a
    /// running set whose device is absent from the probe on two consecutive ticks — one miss
    /// may be a transient enumerate hiccup — is faulted via `mark_device_fault`.
    /// `missing_once` carries the single-miss ids between ticks.
    ///
    /// Capture-ring overrun growth surfaces here too, rather than from the DSP thread: the
    /// prober cadence rate-limits both the warn and the `StateChanged` fan-out to one per
    /// set per tick, and clients then read the cumulative count from `DeviceSet.overruns`.
    /// Live-recording progress (and writer faults) ride the same diff, so a recording's
    /// counters refresh for clients without any extra event source.
    /// [`Engine::hotplug_tick`] for the server's tests, which have to drive a replug without
    /// waiting out the prober's interval.
    pub fn hotplug_tick_for_test(
        &self,
        known: &mut Option<Vec<String>>,
        missing_once: &mut HashSet<u32>,
    ) -> bool {
        self.hotplug_tick(known, missing_once)
    }

    fn hotplug_tick(
        &self,
        known: &mut Option<Vec<String>>,
        missing_once: &mut HashSet<u32>,
    ) -> bool {
        let (grown, rec_faults, changed) = {
            let mut inner = self.lock();
            let mut grown: Vec<(u32, u64)> = Vec::new();
            let mut rec_faults: Vec<(u32, String)> = Vec::new();
            let mut changed: Vec<u32> = Vec::new();
            for (id, s) in inner.device_sets.iter_mut() {
                let now = s.overruns_total();
                let delta = now - s.overruns_seen;
                s.overruns_seen = now;
                let mut dirty = delta > 0;
                if delta > 0 {
                    grown.push((*id, delta));
                }
                if let Some(rec) = &mut s.recording {
                    let samples = rec.shared.samples();
                    if samples != rec.samples_seen {
                        rec.samples_seen = samples;
                        dirty = true;
                    }
                    if let Some(error) = rec.shared.error()
                        && !rec.error_seen
                    {
                        rec.error_seen = true;
                        rec_faults.push((*id, error));
                        dirty = true;
                    }
                }
                if dirty {
                    changed.push(*id);
                }
            }
            if !changed.is_empty() {
                inner.revision += 1;
            }
            (grown, rec_faults, changed)
        };
        for (ds, dropped) in grown {
            tracing::warn!(ds, dropped, "capture ring overrun: device samples dropped");
        }
        for (ds, error) in rec_faults {
            tracing::warn!(ds, error = %error, "recording fault");
        }
        for ds in changed {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(ds),
            });
        }

        let ids: Vec<String> = self
            .registry
            .probe_all()
            .iter()
            .map(DeviceInfo::id)
            .collect();

        let (absent, returned): (HashSet<u32>, Vec<u32>) = {
            let inner = self.lock();
            let absent = inner
                .device_sets
                .iter()
                .filter(|(_, s)| {
                    s.status == DeviceSetStatus::Running && !ids.contains(&s.info.id())
                })
                .map(|(id, _)| *id)
                .collect();
            // A faulted set whose device is attached again is the replug case (PLAN §16 M5).
            let returned = inner
                .device_sets
                .iter()
                .filter(|(_, s)| s.status == DeviceSetStatus::Error && ids.contains(&s.info.id()))
                .map(|(id, _)| *id)
                .collect();
            (absent, returned)
        };
        for ds in absent.intersection(missing_once) {
            self.mark_device_fault(
                *ds,
                DeviceError::Io("device disappeared from probe".to_string()),
            );
        }
        *missing_once = absent;
        for ds in returned {
            self.reconnect(ds);
        }

        let changed = known.as_ref().is_some_and(|prev| *prev != ids);
        *known = Some(ids);
        if changed {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::Devices,
            });
        }
        changed
    }

    /// Re-open a faulted device set whose device has re-enumerated, restoring its tuning and
    /// its channels (PLAN §16 M5: auto-reconnect on replug). Driven by the hotplug tick, so an
    /// attempt costs one open per probe interval at worst, and a device that is present but
    /// still unopenable (settling, claimed elsewhere) simply keeps the set faulted with the
    /// live reason. Best-effort by nature: failures update the visible error, never panic and
    /// never leave the set half-swapped.
    ///
    /// Not restored: a scan that was running when the device died (it was stopped with the
    /// device and the operator chooses when to sweep again), and a recording (already
    /// finalized into a playable pair by [`Engine::mark_device_fault`]).
    fn reconnect(&self, ds: u32) {
        let stored = {
            let inner = self.lock();
            let Some(state) = inner.device_sets.get(&ds) else {
                return;
            };
            if state.status != DeviceSetStatus::Error {
                return;
            }
            (state.info.id(), state.settings.clone())
        };
        let (device_id, stored_settings) = stored;

        // All device I/O outside `inner`: an unresponsive USB stack must not stall snapshots
        // or the other sets, exactly as in `patch_device`.
        let opened = self
            .registry
            .open(&device_id)
            .and_then(|(info, mut device)| {
                // Restore the tuning before the stream starts, so the set comes back on the
                // frequency the operator left it on rather than the driver's power-on default.
                device.apply(&stored_settings)?;
                Ok((info, device))
            });
        let (info, device) = match opened {
            Ok(opened) => opened,
            Err(e) => {
                self.note_reconnect_failure(ds, &e.to_string());
                return;
            }
        };
        let capabilities = device.capabilities().clone();
        // A reconnect is a create followed by a patch, and it projects state the same way
        // both of those do: the fresh device's own settings as the base, the stored ones
        // merged over them. Reading `device.settings()` alone would depend on every backend
        // reflecting `apply` back into its own state, which the trait does not require.
        // Same rule as `patch_device`: the configuration being restored, with whatever the
        // reopened device actually reports laid over it.
        let mut settings = stored_settings.clone();
        settings.merge_from(&device.settings().clone());
        let rate = sample_rate_of(&settings);
        // A fault from the *fresh* capture that lands before the swap below would be applied
        // to a set that is still `Error` and then overwritten by the swap — leaving a dead
        // capture advertised as Running forever. The gate parks such a fault until the swap
        // has finished, and the normal fault path takes over from there.
        let gate = Arc::new(Mutex::new(FaultGate::Pending(None)));
        let fault_tx = self.fault_tx.clone();
        let handler_gate = gate.clone();
        let runtime = match CaptureRuntime::start(device, &settings, move |err| {
            let mut gate = handler_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match &mut *gate {
                FaultGate::Pending(slot) => *slot = Some(err),
                FaultGate::Armed => {
                    let _ = fault_tx.send((ds, err));
                }
            }
        }) {
            Ok(runtime) => runtime,
            Err(e) => {
                self.note_reconnect_failure(ds, &e.to_string());
                return;
            }
        };
        let cmd_txs = runtime.command_senders();
        let overruns = runtime.overruns_counters();
        let runtime = Arc::new(Mutex::new(runtime));

        // Swap under `inner` so a concurrent removal or a fault on the new capture cannot
        // interleave into a half-replaced set.
        let (old_runtime, rebuilds, early_fault) = {
            let mut inner = self.lock();
            let Some(state) = inner.device_sets.get_mut(&ds) else {
                drop(inner);
                lock_runtime(&runtime).stop();
                return;
            };
            if state.status != DeviceSetStatus::Error {
                // Something already revived or replaced the set; drop the spare device.
                drop(inner);
                lock_runtime(&runtime).stop();
                return;
            }
            // Arming under `inner` closes the window: the handler only ever takes the gate
            // lock, never `inner`, so a fault racing this line lands on exactly one side.
            let early_fault = match std::mem::replace(
                &mut *gate
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                FaultGate::Armed,
            ) {
                FaultGate::Pending(slot) => slot,
                FaultGate::Armed => None,
            };
            let old_runtime = std::mem::replace(&mut state.runtime, runtime);
            state.cmd_txs = cmd_txs;
            // The fresh counters start at zero, so the seen-watermark has to follow them or
            // the next tick would compute a negative delta.
            state.overruns = overruns;
            state.overruns_seen = 0;
            state.info = info;
            state.capabilities = capabilities;
            state.settings = settings;
            state.status = DeviceSetStatus::Running;
            state.error = None;
            let rebuilds: Vec<RebuildEntry> = state
                .channels
                .iter()
                .filter_map(|c| {
                    state.audio.get(&c.id).map(|a| RebuildEntry {
                        id: c.id,
                        stream: c.stream,
                        settings: c.settings.clone(),
                        pcm_tx: a.pcm_tx.clone(),
                        pcm_pos: a.pcm_pos.clone(),
                    })
                })
                .collect();
            inner.revision += 1;
            (old_runtime, rebuilds, early_fault)
        };
        // The old DSP thread still runs (only the capture half died); stop it before the
        // replacements start, so one channel is never hosted on two threads at once.
        lock_runtime(&old_runtime).stop();
        drop(old_runtime);

        let mut dead: Vec<ChannelAudio> = Vec::new();
        for rebuild in rebuilds {
            self.rebuild_channel(ds, rebuild, rate, &mut dead);
        }
        for handle in dead {
            handle.shutdown();
        }
        if let Some(err) = early_fault {
            // The replacement died before it was installed; hand it to the normal fault path
            // so the set is faulted, its device released, and the next probe can try again.
            tracing::warn!(ds, error = %err, "reconnected capture died immediately");
            self.mark_device_fault(ds, err);
            return;
        }
        tracing::info!(ds, device = %device_id, "device set reconnected after replug");
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
    }

    /// Record why a reconnect attempt failed, emitting only when the reason changes: the
    /// hotplug tick retries every interval, and a device that stays unopenable would
    /// otherwise invalidate every client's state on a timer.
    fn note_reconnect_failure(&self, ds: u32, reason: &str) {
        let message = format!("device present but not reopenable: {reason}");
        let changed = {
            let mut inner = self.lock();
            let Some(state) = inner.device_sets.get_mut(&ds) else {
                return;
            };
            if state.status != DeviceSetStatus::Error || state.error.as_deref() == Some(&message) {
                false
            } else {
                state.error = Some(message);
                inner.revision += 1;
                true
            }
        };
        if changed {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(ds),
            });
        }
    }

    /// Subscribe to the low-rate `ServerEvent` stream (state changes, errors).
    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<ServerEvent> {
        self.event_tx.subscribe()
    }

    /// Emit a `StateChanged` for state the engine does not own (presets, bookmarks): the WS
    /// hub forwards engine events only, so server-side stores invalidate through here.
    pub fn emit_scope(&self, scope: StateScope) {
        self.emit(ServerEvent::StateChanged { scope });
    }

    /// Discovered devices across all drivers (PLAN §5 `GET /api/devices`).
    #[must_use]
    pub fn probe_devices(&self) -> Vec<DeviceInfo> {
        self.registry.probe_all()
    }

    /// The registry this engine opens devices through. Exposed so diagnostics report the same
    /// backends the server would actually use, instead of building a second registry — a
    /// concurrent second enumerate is what crashed libusb in the post-M2 field sessions.
    #[must_use]
    pub fn registry(&self) -> &DeviceRegistry {
        &self.registry
    }

    /// Full authoritative snapshot (PLAN §5 `GET /api/state`).
    #[must_use]
    pub fn snapshot(&self) -> StateSnapshot {
        let inner = self.lock();
        StateSnapshot {
            device_sets: inner
                .device_sets
                .iter()
                .map(|(id, s)| s.project(*id))
                .collect(),
            revision: inner.revision,
        }
    }

    /// Open a device into a new device set and start streaming (PLAN §5 POST devicesets).
    pub fn create_device_set(&self, device_id: &str) -> Result<u32, EngineError> {
        let (info, device) = self.registry.open(device_id)?;
        let capabilities = device.capabilities().clone();
        let settings = device.settings().clone();

        // Reserve the id before the runtime exists: the fatal handler must name its device
        // set, and `creating` membership lets `mark_device_fault` stash a fault raised before
        // the insert below instead of dropping it.
        let id = {
            let mut inner = self.lock();
            let id = inner.next_ds_id;
            inner.next_ds_id += 1;
            inner.creating.insert(id);
            id
        };
        let fault_tx = self.fault_tx.clone();
        let started = CaptureRuntime::start(device, &settings, move |err| {
            // Unbounded send: the dying capture thread never blocks on the control plane.
            let _ = fault_tx.send((id, err));
        });
        let runtime = match started {
            Ok(runtime) => runtime,
            Err(e) => {
                let mut inner = self.lock();
                inner.creating.remove(&id);
                inner.pending_faults.remove(&id);
                return Err(e.into());
            }
        };

        let cmd_txs = runtime.command_senders();
        let overruns = runtime.overruns_counters();
        let faulted = {
            let mut inner = self.lock();
            inner.creating.remove(&id);
            let pending = inner.pending_faults.remove(&id);
            inner.device_sets.insert(
                id,
                DeviceSetState {
                    info,
                    capabilities,
                    settings,
                    status: if pending.is_some() {
                        DeviceSetStatus::Error
                    } else {
                        DeviceSetStatus::Running
                    },
                    channels: Vec::new(),
                    audio: HashMap::new(),
                    next_channel_id: 1,
                    error: pending.as_ref().map(ToString::to_string),
                    recording: None,
                    scanner: None,
                    rate_patches: 0,
                    cmd_txs,
                    overruns,
                    overruns_seen: 0,
                    runtime: Arc::new(Mutex::new(runtime)),
                },
            );
            inner.revision += 1;
            pending.is_some()
        };
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::All,
        });
        if faulted {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(id),
            });
        }
        Ok(id)
    }

    /// Close a device set and stop its threads (PLAN §5 DELETE devicesets).
    pub fn remove_device_set(&self, ds: u32) -> Result<(), EngineError> {
        // Take ownership out of the map, then stop (joining threads) OUTSIDE the engine lock;
        // the per-set lock lets a concurrent `patch_device` finish its device I/O first.
        let removed = {
            let mut inner = self.lock();
            let removed = inner.device_sets.remove(&ds);
            if removed.is_some() {
                inner.revision += 1;
            }
            removed
        };
        let removed = removed.ok_or(EngineError::DeviceSetNotFound(ds))?;
        let finalized = teardown_set(removed);
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::All,
        });
        if finalized {
            // The implicit stop just finalized a playable pair; clients only learn about it
            // through this scope (GET /api/recordings reconciles on fetch).
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::Recordings,
            });
        }
        Ok(())
    }

    /// Tear down every device set — capture stop, recording writer join, encoder joins — so
    /// live recordings finalize into playable pairs instead of dying as breadcrumbs when the
    /// process exits. Idempotent; `Drop` calls it too, but binaries whose exit path never
    /// unwinds the engine (Tauri, `process::exit`) must call it explicitly.
    pub fn shutdown(&self) {
        let removed: Vec<DeviceSetState> = {
            let mut inner = self.lock();
            if inner.device_sets.is_empty() {
                return;
            }
            inner.revision += 1;
            std::mem::take(&mut inner.device_sets)
                .into_values()
                .collect()
        };
        let mut finalized = false;
        for set in removed {
            finalized |= teardown_set(set);
        }
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::All,
        });
        if finalized {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::Recordings,
            });
        }
    }

    /// Apply a device settings delta (PLAN §5 PATCH device). The device I/O runs under the
    /// per-set lock only; `inner` is re-taken afterwards to merge, so a wedged device never
    /// blocks `snapshot` or other sets. A sample-rate change rebuilds every hosted channel
    /// pipeline at the new rate (ids and audio streams preserved); center-frequency changes
    /// need nothing — channel offsets are center-relative.
    pub fn patch_device(&self, ds: u32, delta: DeviceSettings) -> Result<(), EngineError> {
        self.patch_device_from(ds, delta, PatchOrigin::Client)
    }

    /// [`Engine::patch_device`] with the caller's identity: a scan owns its set's centre
    /// frequency, so its own retunes skip the anti-fighting guard and stay silent (progress
    /// rides [`ServerEvent::ScannerUpdate`] instead of a state refetch per step).
    fn patch_device_from(
        &self,
        ds: u32,
        delta: DeviceSettings,
        origin: PatchOrigin,
    ) -> Result<(), EngineError> {
        let (runtime, _rate_guard) = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            if origin == PatchOrigin::Client && state.scanner.is_some() {
                return Err(EngineError::Scan(
                    "the device is being tuned by a running scan; stop the scan first".to_string(),
                ));
            }
            validate_streams(&state.capabilities, &delta)?;
            // Refuse a rate the hosted channels cannot run at, before any device I/O —
            // rejecting up front beats stranding a channel after the device already retuned.
            let mut rate_change = false;
            if let Some(new_rate) = delta.sample_rate
                && new_rate != sample_rate_of(&state.settings)
            {
                // SigMF `core:sample_rate` is global-scope — one rate per file — so a live
                // recording pins the device rate; center retunes stay allowed (they land as
                // capture segments).
                if state.recording.is_some() {
                    return Err(EngineError::Recording(
                        "sample rate is locked while recording; stop the recording first"
                            .to_string(),
                    ));
                }
                for channel in &state.channels {
                    let descriptor = descriptor_for(&channel.settings.params)?;
                    validate_channel(&descriptor, &channel.settings, new_rate)?;
                }
                rate_change = true;
            }
            let runtime = state.runtime.clone();
            // The guard closes the patch-vs-record race for the whole apply-to-merge window:
            // `start_recording`'s commit refuses while it is up, so a recording can never pin
            // a rate this patch is about to change.
            let guard = rate_change.then(|| {
                state.rate_patches += 1;
                RatePatchGuard { engine: self, ds }
            });
            (runtime, guard)
        };
        // Read the device's own view back while the runtime lock is already held: what it
        // actually holds is not always what was asked for (a gain lands on the tuner's step
        // grid, a rate on the resampler's achievable ratio), and the client renders this.
        let actual = {
            let mut runtime = lock_runtime(&runtime);
            runtime.apply(&delta)?;
            runtime.device_settings()
        };
        let (settings, rate, rebuilds) = {
            let mut inner = self.lock();
            // The set may have been removed while `apply` ran; its runtime was stopped by
            // `remove_device_set`, so just report the removal.
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let old_rate = sample_rate_of(&state.settings);
            // A same-rate delta carries no guard, so if another patch moved the rate while
            // `apply` ran, this delta may now be a rate change under a recording that
            // committed in between. Merging would break the one-rate-per-file invariant;
            // revert the device instead and lose cleanly.
            if state.recording.is_some() && delta.sample_rate.is_some_and(|r| r != old_rate) {
                drop(inner);
                let revert = DeviceSettings {
                    sample_rate: Some(old_rate),
                    ..DeviceSettings::default()
                };
                if let Err(e) = lock_runtime(&runtime).apply(&revert) {
                    return Err(EngineError::Recording(format!(
                        "sample rate is locked while recording, and reverting the device to \
                         {old_rate} Hz failed: {e}"
                    )));
                }
                return Err(EngineError::Recording(
                    "sample rate is locked while recording; stop the recording first".to_string(),
                ));
            }
            // The request first, then the device's truth over the top. Both are needed: a
            // backend that reports a field must win (a HackRF asked for 13 dB of LNA holds
            // 16 — the grid has no 13), and one that reports nothing for a field must not
            // erase what was asked. Reporting the request alone is a lie the whole
            // capability-driven UI is built on top of.
            state.settings.merge_from(&delta);
            if let Some(actual) = &actual {
                state.settings.merge_from(actual);
            }
            let rate = sample_rate_of(&state.settings);
            let rebuilds: Vec<RebuildEntry> = if rate == old_rate {
                Vec::new()
            } else {
                state
                    .channels
                    .iter()
                    .filter_map(|c| {
                        state.audio.get(&c.id).map(|a| RebuildEntry {
                            id: c.id,
                            stream: c.stream,
                            settings: c.settings.clone(),
                            pcm_tx: a.pcm_tx.clone(),
                            pcm_pos: a.pcm_pos.clone(),
                        })
                    })
                    .collect()
            };
            let settings = state.settings.clone();
            inner.revision += 1;
            (settings, rate, rebuilds)
        };
        lock_runtime(&runtime).set_meta(&settings);
        let mut dead: Vec<ChannelAudio> = Vec::new();
        for rebuild in rebuilds {
            self.rebuild_channel(ds, rebuild, rate, &mut dead);
        }
        // Encoder joins happen outside `inner`; the RemoveChannel each dead entry already
        // queued guarantees the DSP-side PCM sender drops, so these cannot hang.
        for handle in dead {
            handle.shutdown();
        }
        if origin == PatchOrigin::Client {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(ds),
            });
        }
        Ok(())
    }

    /// Swap one hosted channel onto the post-patch device rate. The host is built outside
    /// any lock; `inner` is then re-taken to verify the channel still exists with exactly
    /// the settings (and rate) the host was built for — retrying against the fresher state
    /// otherwise — and the Remove+Add pair is queued under `inner`. A concurrently removed
    /// channel is skipped: the unused host just drops (taking its PCM sender clone with it),
    /// so the removal's encoder join stays sound. A channel that no longer validates at the
    /// current rate is dropped with a loud error; its audio teardown lands in `dead` for the
    /// caller to join outside the lock.
    fn rebuild_channel(
        &self,
        ds: u32,
        rebuild: RebuildEntry,
        rate: f64,
        dead: &mut Vec<ChannelAudio>,
    ) {
        let RebuildEntry {
            id,
            stream,
            mut settings,
            pcm_tx,
            pcm_pos,
        } = rebuild;
        let mut built_rate = rate;
        loop {
            let built = descriptor_for(&settings.params)
                .and_then(|d| validate_channel(&d, &settings, built_rate))
                .and_then(|()| {
                    ChannelHost::build(
                        built_rate,
                        &settings,
                        pcm_tx.clone(),
                        pcm_pos.clone(),
                        self.decoded_sink(ds, id),
                    )
                    .map_err(EngineError::from)
                });
            let mut inner = self.lock();
            let Some(state) = inner.device_sets.get_mut(&ds) else {
                // The set was removed while building; that removal owns all teardown.
                return;
            };
            let current_rate = sample_rate_of(&state.settings);
            let Some(info) = state.channels.iter().find(|c| c.id == id) else {
                // The channel was removed while building — nothing to swap.
                return;
            };
            if current_rate != built_rate || info.settings != settings {
                // The snapshot went stale (another patch landed); rebuild against it.
                settings = info.settings.clone();
                built_rate = current_rate;
                continue;
            }
            match built {
                Ok(host) => {
                    state.send_dsp(stream, DspCommand::RemoveChannel { id });
                    state.send_dsp(stream, DspCommand::AddChannel { id, host });
                }
                Err(e) => {
                    // The rate was pre-validated against every channel before the device
                    // I/O, so only a racing settings change lands here; drop the channel
                    // rather than leave a stale-rate pipeline running.
                    tracing::error!(ds, channel = id, error = %e, "channel rebuild failed after rate change; removing channel");
                    state.channels.retain(|c| c.id != id);
                    dead.extend(state.audio.remove(&id));
                    state.send_dsp(stream, DspCommand::RemoveChannel { id });
                    inner.revision += 1;
                }
            }
            return;
        }
    }

    /// Pre-flight a whole replacement configuration (a preset or a template) against a device
    /// set, without changing anything.
    ///
    /// Applying one is destructive by construction — the existing channels have to go before
    /// the rate can move (`patch_device` validates a new rate against the channels hosted
    /// *now*, which would veto a perfectly good preset on behalf of channels the apply is
    /// about to delete). That ordering is only acceptable if the configuration is known to be
    /// applicable first; otherwise a request the device was always going to reject leaves the
    /// operator with an empty device set.
    pub fn validate_configuration(
        &self,
        ds: u32,
        settings: &DeviceSettings,
        channels: &[ChannelSettings],
    ) -> Result<(), EngineError> {
        let inner = self.lock();
        let state = inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let rate = settings
            .sample_rate
            .unwrap_or_else(|| sample_rate_of(&state.settings));
        let caps = &state.capabilities;
        if let Some(center) = settings.center_hz
            && !tuner_reaches(caps, center)
        {
            return Err(DeviceError::Unsupported(format!(
                "{center} Hz is outside this device's tuning range"
            ))
            .into());
        }
        validate_streams(caps, settings)?;
        for channel in channels {
            let descriptor = descriptor_for(&channel.params)?;
            validate_channel(&descriptor, channel, rate)?;
        }
        Ok(())
    }

    /// Add a channel to a device set (PLAN §5 POST channels), tapping the rx stream
    /// `stream`: validate and build the whole DDC → demod pipeline control-side, then hand
    /// it to that stream's DSP thread via the command queue. Construction failures surface
    /// here as bad requests.
    pub fn add_channel(
        &self,
        ds: u32,
        stream: u32,
        settings: ChannelSettings,
    ) -> Result<u32, EngineError> {
        let descriptor = descriptor_for(&settings.params)?;
        // Reserve the id before building: the host's decoder sink is bound to it, and ids
        // are never reused, so a failed add simply leaves a gap.
        let (mut device_rate, id) = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            state.check_stream(stream)?;
            let id = state.next_channel_id;
            state.next_channel_id += 1;
            (sample_rate_of(&state.settings), id)
        };
        let created = ChannelAudio::new(sdrmm_channels::audio_channels(&settings.params))?;
        let pcm_tx = created.pcm_tx.clone();
        let pcm_pos = created.pcm_pos.clone();
        let mut audio = Some(created);

        // A rate patch racing between build and insert would leave a wrong-rate DDC, so the
        // rate is re-checked under the lock and the pipeline rebuilt if it moved. The
        // AddChannel goes out in the same critical section as the state commit, so no
        // concurrent removal or swap can interleave between them.
        let staged = loop {
            let built = validate_channel(&descriptor, &settings, device_rate).and_then(|()| {
                ChannelHost::build(
                    device_rate,
                    &settings,
                    pcm_tx.clone(),
                    pcm_pos.clone(),
                    self.decoded_sink(ds, id),
                )
                .map_err(EngineError::from)
            });
            let host = match built {
                Ok(host) => host,
                Err(e) => break Err(e),
            };
            let mut inner = self.lock();
            let Some(state) = inner.device_sets.get_mut(&ds) else {
                break Err(EngineError::DeviceSetNotFound(ds));
            };
            // Re-checked at commit: a reconnect may have swapped the runtime (and its lane
            // count) while the host was building outside the lock.
            if let Err(e) = state.check_stream(stream) {
                break Err(e);
            }
            let current_rate = sample_rate_of(&state.settings);
            if current_rate != device_rate {
                device_rate = current_rate;
                continue;
            }
            state.channels.push(ChannelInfo {
                id,
                stream,
                settings: settings.clone(),
            });
            if let Some(handle) = audio.take() {
                state.audio.insert(id, handle);
            }
            state.send_dsp(stream, DspCommand::AddChannel { id, host });
            inner.revision += 1;
            break Ok(id);
        };
        let id = match staged {
            Ok(id) => id,
            Err(e) => {
                // The local sender clone must go first or the encoder join would wait on it.
                drop(pcm_tx);
                if let Some(handle) = audio.take() {
                    handle.shutdown();
                }
                return Err(e);
            }
        };
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(id)
    }

    /// Apply a channel settings delta (PLAN §5 PATCH channels). Retunes and param tweaks
    /// reach the live pipeline as commands (no rebuild); a params *type* change swaps in a
    /// freshly built pipeline while the channel keeps its id and audio stream. Commands are
    /// queued under `inner` in the same critical section as the state commit, so what the
    /// DSP thread hosts always converges to the last-committed settings.
    pub fn patch_channel(
        &self,
        ds: u32,
        ch: u32,
        settings: ChannelSettings,
    ) -> Result<(), EngineError> {
        let descriptor = descriptor_for(&settings.params)?;
        let (old, pcm_tx, pcm_pos, mut device_rate) = {
            let inner = self.lock();
            let state = inner
                .device_sets
                .get(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let info = state
                .channels
                .iter()
                .find(|c| c.id == ch)
                .ok_or(EngineError::ChannelNotFound(ch, ds))?;
            let handle = state
                .audio
                .get(&ch)
                .ok_or(EngineError::ChannelNotFound(ch, ds))?;
            (
                info.settings.clone(),
                handle.pcm_tx.clone(),
                handle.pcm_pos.clone(),
                sample_rate_of(&state.settings),
            )
        };
        let mut need_host = old.params.type_id() != settings.params.type_id();
        if !need_host {
            // Construct-and-discard so invalid params surface here as a bad request instead
            // of failing later on the DSP thread.
            drop(sdrmm_channels::create(
                ChannelCtx {
                    input_rate: descriptor.input_rate_hz,
                },
                &settings,
            )?);
        }
        let staged = loop {
            if let Err(e) = validate_channel(&descriptor, &settings, device_rate) {
                break Err(e);
            }
            let host = if need_host {
                match ChannelHost::build(
                    device_rate,
                    &settings,
                    pcm_tx.clone(),
                    pcm_pos.clone(),
                    self.decoded_sink(ds, ch),
                ) {
                    Ok(host) => Some(host),
                    Err(e) => break Err(e.into()),
                }
            } else {
                None
            };
            let mut inner = self.lock();
            let Some(state) = inner.device_sets.get_mut(&ds) else {
                break Err(EngineError::DeviceSetNotFound(ds));
            };
            let current_rate = sample_rate_of(&state.settings);
            if current_rate != device_rate {
                device_rate = current_rate;
                continue;
            }
            let Some(info) = state.channels.iter_mut().find(|c| c.id == ch) else {
                break Err(EngineError::ChannelNotFound(ch, ds));
            };
            // Diff against what is committed *now*, not the entry snapshot: a concurrent
            // patch may have landed in between, and skipping the command then would leave
            // the DSP pipeline on its settings while state shows ours.
            if info.settings.params.type_id() != settings.params.type_id() && host.is_none() {
                need_host = true;
                continue;
            }
            // A patch never moves a channel between streams; the lane is the one the channel
            // was created on.
            let stream = info.stream;
            let prev = std::mem::replace(&mut info.settings, settings.clone());
            match host {
                Some(host) => {
                    state.send_dsp(stream, DspCommand::RemoveChannel { id: ch });
                    state.send_dsp(stream, DspCommand::AddChannel { id: ch, host });
                }
                None => {
                    if prev.offset_hz != settings.offset_hz {
                        state.send_dsp(
                            stream,
                            DspCommand::Retune {
                                id: ch,
                                offset_hz: settings.offset_hz,
                            },
                        );
                    }
                    if prev.params != settings.params || prev.squelch_db != settings.squelch_db {
                        state.send_dsp(
                            stream,
                            DspCommand::ApplySettings {
                                id: ch,
                                settings: settings.clone(),
                            },
                        );
                    }
                }
            }
            inner.revision += 1;
            break Ok(());
        };
        staged?;
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(())
    }

    /// Remove a channel from a device set (PLAN §5 DELETE channels), tearing down its DSP
    /// pipeline and joining its encoder thread.
    pub fn remove_channel(&self, ds: u32, ch: u32) -> Result<(), EngineError> {
        let handle = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let stream = state
                .channels
                .iter()
                .find(|c| c.id == ch)
                .map(|c| c.stream)
                .ok_or(EngineError::ChannelNotFound(ch, ds))?;
            state.channels.retain(|c| c.id != ch);
            let handle = state.audio.remove(&ch);
            // Queued under `inner` in the same critical section as the state removal: every
            // rebuild swap re-checks membership under `inner` before queueing, so nothing
            // can re-add the host after this — the DSP-side PCM sender is guaranteed to
            // drop and the encoder join below cannot hang.
            state.send_dsp(stream, DspCommand::RemoveChannel { id: ch });
            inner.revision += 1;
            handle
        };
        if let Some(handle) = handle {
            handle.shutdown();
        }
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(())
    }

    /// Start recording one rx stream of a device set's raw IQ into a SigMF pair under the
    /// recordings dir (PLAN §5; the path is lossless — see [`recording`]). One recording per
    /// set, on the named stream (design §6b); the SigMF meta records which. Writer, files,
    /// and thread come up control-side so open errors surface here; the tap then arms via
    /// that stream's command queue in the same critical section as the state commit (the
    /// `send_dsp` invariant), with the commit re-verifying the rate the meta was written
    /// with — an `add_channel`-style retry against racing patches.
    pub fn start_recording(&self, ds: u32, stream: u32) -> Result<(), EngineError> {
        loop {
            let (rate, center, hw) = {
                let inner = self.lock();
                let state = inner
                    .device_sets
                    .get(&ds)
                    .ok_or(EngineError::DeviceSetNotFound(ds))?;
                state.check_stream(stream)?;
                if state.recording.is_some() {
                    return Err(EngineError::Recording("already recording".to_string()));
                }
                if state.status != DeviceSetStatus::Running {
                    return Err(EngineError::Recording(
                        "device set is not running".to_string(),
                    ));
                }
                // The SigMF meta opens with the recorded *lane's* centre: the tap stamps
                // every block with that lane's DSP meta, so a radio-wide value here would
                // file the capture under a frequency the lane never sat on (and open with a
                // spurious extra capture segment on a per-stream-retuned lane).
                let center = state
                    .settings
                    .for_stream(stream, &state.capabilities.per_stream)
                    .center_hz
                    .unwrap_or(DEFAULT_CENTER_HZ);
                (
                    sample_rate_of(&state.settings),
                    center,
                    state.info.label.clone(),
                )
            };
            // After the set lookup, so a missing set stays a 404 even with recording disabled.
            let Some(dir) = self.recordings_dir.clone() else {
                return Err(EngineError::Recording(
                    "no recordings directory configured".to_string(),
                ));
            };
            std::fs::create_dir_all(&dir)
                .map_err(|e| EngineError::RecordingIo(format!("create {}: {e}", dir.display())))?;
            let started_at = jiff::Timestamp::now();
            let (sigmf, file) =
                recording::create_writer(&dir, ds, stream, started_at, rate, center, &hw)?;
            let stem = sigmf.stem().to_path_buf();
            let (tap, blocks, shared) = recording::create_tap();
            let writer = recording::spawn_writer(sigmf, blocks, shared.clone())?;

            let (aborted, patch_in_flight) = {
                let mut inner = self.lock();
                match inner.device_sets.get_mut(&ds) {
                    // The stream bound is re-checked too: a reconnect may have swapped the
                    // runtime while the files were opened, and an aborted attempt re-loops
                    // into the entry check, which then refuses with the count.
                    Some(state)
                        if state.status == DeviceSetStatus::Running
                            && state.recording.is_none()
                            && state.rate_patches == 0
                            && state.check_stream(stream).is_ok()
                            && sample_rate_of(&state.settings) == rate =>
                    {
                        state.recording = Some(RecordingState {
                            file,
                            stream,
                            started_at: started_at.to_string(),
                            stem: stem.clone(),
                            shared,
                            writer,
                            overruns_at_start: state.overruns_total(),
                            samples_seen: 0,
                            error_seen: false,
                        });
                        state.send_dsp(stream, DspCommand::StartRecording { tap });
                        inner.revision += 1;
                        (None, false)
                    }
                    // A rate patch is between its pre-validation and its merge: committing
                    // now would pin a rate the device is about to leave, and retrying would
                    // spin for as long as the patch sits in device I/O — fail instead.
                    Some(state) if state.rate_patches > 0 => (Some((tap, writer)), true),
                    _ => (Some((tap, writer)), false),
                }
            };
            let Some((tap, writer)) = aborted else {
                self.emit(ServerEvent::StateChanged {
                    scope: StateScope::DeviceSet(ds),
                });
                return Ok(());
            };
            // The set moved while the files were opened (removed, faulted, rate patched, or
            // a concurrent start won): closing the tap finalizes the empty attempt, whose
            // files — exclusively this attempt's, by the create_new stem claim — are then
            // discarded before re-evaluating against fresh state.
            drop(tap);
            join_recording_writer(writer);
            for path in [meta_path(&stem), data_path(&stem)] {
                if let Err(e) = std::fs::remove_file(&path)
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    tracing::warn!(path = %path.display(), error = %e, "aborted recording attempt left a file behind");
                }
            }
            if patch_in_flight {
                return Err(EngineError::Recording(
                    "a sample-rate change is in flight; retry once it completes".to_string(),
                ));
            }
        }
    }

    /// Stop a live recording, join its writer, and hand back the finalized pair for indexing
    /// (PLAN §11). The join happens outside `inner`; the `StopRecording` queued in the same
    /// critical section as the take guarantees the DSP-side tap drops (or the whole runtime
    /// stopped, dropping tap and queue together), so the join cannot hang.
    pub fn stop_recording(&self, ds: u32) -> Result<FinalizedRecording, EngineError> {
        let (recording, overruns) = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let Some(recording) = state.recording.take() else {
                return Err(EngineError::Recording("not recording".to_string()));
            };
            state.send_dsp(recording.stream, DspCommand::StopRecording);
            let overruns = state.overruns.clone();
            inner.revision += 1;
            (recording, overruns)
        };
        let RecordingState {
            stem,
            stream,
            started_at,
            shared,
            writer,
            overruns_at_start,
            ..
        } = recording;
        join_recording_writer(writer);
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        let overruns_now: u64 = overruns
            .iter()
            .map(|counter| counter.load(Ordering::Relaxed))
            .sum();
        Ok(FinalizedRecording {
            stem,
            stream,
            started_at,
            samples: shared.samples(),
            bytes: shared.bytes(),
            overruns: overruns_now - overruns_at_start,
            error: shared.error(),
        })
    }

    /// Subscribe to a channel's Opus packet stream (PLAN §5 SubscribeAudio).
    pub fn subscribe_audio(
        &self,
        ds: u32,
        ch: u32,
    ) -> Result<broadcast::Receiver<AudioPacket>, EngineError> {
        let inner = self.lock();
        let state = inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let handle = state
            .audio
            .get(&ch)
            .ok_or(EngineError::ChannelNotFound(ch, ds))?;
        Ok(handle.audio_tx.subscribe())
    }

    /// Every compiled-in channel type (PLAN §5 GET channel types). The registry lives in
    /// `sdrmm-channels`; the server reaches it through here and never depends on it directly.
    #[must_use]
    pub fn channel_types(&self) -> Vec<ChannelDescriptor> {
        sdrmm_channels::descriptors()
    }

    /// Start a frequency scan on a device set (PLAN §13 P2, M5). The scan owns the set's
    /// centre frequency until it is stopped, so client retunes are refused meanwhile.
    pub fn start_scan(
        self: &Arc<Self>,
        ds: u32,
        settings: ScanSettings,
    ) -> Result<ScannerStatus, EngineError> {
        let plan = ScanPlan::build(&settings)?;
        {
            let inner = self.lock();
            let state = inner
                .device_sets
                .get(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            if state.scanner.is_some() {
                return Err(EngineError::Scan("a scan is already running".to_string()));
            }
            if state.status != DeviceSetStatus::Running {
                return Err(EngineError::Scan(
                    "the device set is not running".to_string(),
                ));
            }
            // A scan drives the radio-wide dial. Where tuning is per-stream that dial is only
            // the default for lanes without an override (design §6.3), so a sweep would drag
            // every unpinned lane along and skip the pinned ones — there is no whole-radio
            // tuning for the scan to own. Refuse rather than silently sweep every lane
            // (design §6.5).
            if state.capabilities.per_stream.tuning {
                return Err(EngineError::Scan(
                    "this radio tunes each receive stream independently, so a sweep of the \
                     shared dial would retune every lane at once; scanning one stream is not \
                     supported yet"
                        .to_string(),
                ));
            }
            // Reject targets the tuner cannot reach up front: discovering it mid-sweep would
            // stop the scan halfway through with a device error instead of a usable message.
            if !state.capabilities.freq_ranges.is_empty() {
                let reachable = |hz: f64| {
                    state
                        .capabilities
                        .freq_ranges
                        .iter()
                        .any(|r| hz >= r.min && hz <= r.max)
                };
                if let Some(&bad) = plan.targets.iter().find(|&&hz| !reachable(hz)) {
                    let ranges = state
                        .capabilities
                        .freq_ranges
                        .iter()
                        .map(|r| format!("{}–{} Hz", r.min, r.max))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(EngineError::Scan(format!(
                        "{bad} Hz is outside this device's tuning range ({ranges})"
                    )));
                }
            }
            if let Some(channel) = settings.hold_channel
                && !state.channels.iter().any(|c| c.id == channel)
            {
                return Err(EngineError::ChannelNotFound(channel, ds));
            }
        }
        let scanner = scanner::spawn(self, ds, plan, settings)?;
        let status = scanner.status();
        {
            let mut inner = self.lock();
            let Some(state) = inner.device_sets.get_mut(&ds) else {
                // The set went away while the thread was starting; stop it outside the lock.
                drop(inner);
                scanner.stop_and_join();
                return Err(EngineError::DeviceSetNotFound(ds));
            };
            if state.scanner.is_some() {
                drop(inner);
                scanner.stop_and_join();
                return Err(EngineError::Scan("a scan is already running".to_string()));
            }
            state.scanner = Some(scanner);
            inner.revision += 1;
        }
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(status)
    }

    /// Stop a running scan and return its final status. The device stays wherever the scan
    /// left it — that is the frequency the operator was listening to.
    pub fn stop_scan(&self, ds: u32) -> Result<ScannerStatus, EngineError> {
        let scanner = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let scanner = state
                .scanner
                .take()
                .ok_or_else(|| EngineError::Scan("no scan is running".to_string()))?;
            inner.revision += 1;
            scanner
        };
        // Outside `inner`: the scan thread takes that lock on every step.
        let status = scanner.stop_and_join();
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(status)
    }

    /// The device set's current sample rate, or `None` if it is gone — the scan thread's
    /// "is this set still mine to drive" check.
    pub(crate) fn scan_sample_rate(&self, ds: u32) -> Option<f64> {
        let inner = self.lock();
        let state = inner.device_sets.get(&ds)?;
        (state.status == DeviceSetStatus::Running).then(|| sample_rate_of(&state.settings))
    }

    /// Retune for a scan step and hand back a fresh spectrum subscription. Subscribing per
    /// tuning (rather than once per scan) is what lets a scan survive a runtime replacement:
    /// after an auto-reconnect the old broadcast is closed, and the next step picks up the
    /// new one instead of ending the scan.
    pub(crate) fn scan_retune(
        &self,
        ds: u32,
        center_hz: f64,
    ) -> Result<broadcast::Receiver<SpectrumSnapshot>, EngineError> {
        self.patch_device_from(
            ds,
            DeviceSettings {
                center_hz: Some(center_hz),
                ..DeviceSettings::default()
            },
            PatchOrigin::Scan,
        )?;
        // The scan owns the whole radio, not a stream; `start_scan` refuses radios whose
        // streams tune apart, so every lane here shares the tuning the scan drives and
        // stream 0 — the one every device has — speaks for all of them.
        self.subscribe_spectrum(ds, 0)
    }

    /// Park the scan's listening channel on `offset_hz` from the current centre.
    pub(crate) fn scan_park_channel(
        &self,
        ds: u32,
        ch: u32,
        offset_hz: f64,
    ) -> Result<(), EngineError> {
        let settings = {
            let inner = self.lock();
            let state = inner
                .device_sets
                .get(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let info = state
                .channels
                .iter()
                .find(|c| c.id == ch)
                .ok_or(EngineError::ChannelNotFound(ch, ds))?;
            ChannelSettings {
                offset_hz,
                ..info.settings.clone()
            }
        };
        self.patch_channel(ds, ch, settings)
    }

    /// Subscribe to one rx stream of a device set's spectrum (PLAN §5 SubscribeSpectrum).
    pub fn subscribe_spectrum(
        &self,
        ds: u32,
        stream: u32,
    ) -> Result<broadcast::Receiver<SpectrumSnapshot>, EngineError> {
        let (runtime, streams) = {
            let inner = self.lock();
            let state = inner
                .device_sets
                .get(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            (state.runtime.clone(), state.rx_streams())
        };
        lock_runtime(&runtime)
            .subscribe(stream)
            .ok_or(EngineError::StreamOutOfRange { stream, streams })
    }

    fn emit(&self, event: ServerEvent) {
        let _ = self.event_tx.send(event);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn lock_runtime(runtime: &Mutex<CaptureRuntime>) -> std::sync::MutexGuard<'_, CaptureRuntime> {
    runtime
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Stop an unlisted set's threads in the one order that cannot hang: the runtime stop joins
/// the DSP thread, dropping the recorder tap (closing the writer queue) and the last DSP-side
/// PCM senders — only then can the writer and encoder joins complete. Returns whether a live
/// recording was finalized.
fn teardown_set(mut removed: DeviceSetState) -> bool {
    // The scan goes first: it drives the device, and it exits on its own once the set is
    // unlisted (its retunes then report the set gone), so this join cannot hang.
    if let Some(scanner) = removed.scanner.take() {
        scanner.stop_and_join();
    }
    lock_runtime(&removed.runtime).stop();
    let finalized = removed.recording.take().map(RecordingState::join).is_some();
    for (_, handle) in removed.audio.drain() {
        handle.shutdown();
    }
    finalized
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn join_recording_writer(writer: JoinHandle<()>) {
    if writer.join().is_err() {
        tracing::error!("recording writer thread panicked");
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        thread::JoinHandle,
        time::{Duration, Instant},
    };

    use num_complex::Complex;
    use sdrmm_device::{DeviceDriver, DeviceRegistry, RxSink, SdrDevice, single_rx_sink};
    use sdrmm_wire::{
        ChannelSettings, Duplex, NfmParams, ScanState, Sideband, SsbParams, StreamScope,
    };

    use super::*;

    fn mock_info(key: &str, serial: Option<&str>) -> DeviceInfo {
        DeviceInfo {
            driver: "mock".to_string(),
            key: key.to_string(),
            label: format!("Mock {key}"),
            serial: serial.map(str::to_string),
            profile: None,
        }
    }

    fn empty_capabilities() -> Capabilities {
        Capabilities {
            freq_ranges: Vec::new(),
            sample_rates: Vec::new(),
            sample_rate_range: None,
            gains: Vec::new(),
            antennas: Vec::new(),
            bandwidths: Vec::new(),
            extra: Vec::new(),
            duplex: Duplex::RxOnly,
            rx_streams: 1,
            tx_streams: 0,
            per_stream: StreamScope::default(),
        }
    }

    /// Driver whose device streams a few blocks and then dies with an I/O error.
    struct DyingDriver;

    impl DeviceDriver for DyingDriver {
        fn id(&self) -> &'static str {
            "mock"
        }

        fn probe(&self) -> Vec<DeviceInfo> {
            vec![mock_info("dying", Some("MOCK-1"))]
        }

        fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
            Ok(Box::new(DyingDevice {
                capabilities: empty_capabilities(),
                settings: DeviceSettings::default(),
                worker: None,
            }))
        }
    }

    struct DyingDevice {
        capabilities: Capabilities,
        settings: DeviceSettings,
        worker: Option<JoinHandle<()>>,
    }

    impl SdrDevice for DyingDevice {
        fn capabilities(&self) -> &Capabilities {
            &self.capabilities
        }

        fn settings(&self) -> &DeviceSettings {
            &self.settings
        }

        fn apply(&mut self, _settings: &DeviceSettings) -> Result<(), DeviceError> {
            Ok(())
        }

        fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
            let mut sink = single_rx_sink(sinks)?;
            self.worker = Some(std::thread::spawn(move || {
                let block = [Complex::new(0.0f32, 0.0); 256];
                for _ in 0..3 {
                    sink.push(&block);
                }
                sink.fail(DeviceError::Io("mock stream died".to_string()));
            }));
            Ok(())
        }

        fn rx_stop(&mut self) {
            if let Some(handle) = self.worker.take() {
                let _ = handle.join();
            }
        }
    }

    /// Driver whose device reports a fatal error synchronously inside `rx_start`, so the fault
    /// is on the drainer's queue before `create_device_set` can insert the set — the
    /// stash-then-apply window made deterministic.
    struct InstantFailDriver;

    impl DeviceDriver for InstantFailDriver {
        fn id(&self) -> &'static str {
            "mock"
        }

        fn probe(&self) -> Vec<DeviceInfo> {
            vec![mock_info("instafail", None)]
        }

        fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
            Ok(Box::new(InstantFailDevice {
                capabilities: empty_capabilities(),
                settings: DeviceSettings::default(),
            }))
        }
    }

    struct InstantFailDevice {
        capabilities: Capabilities,
        settings: DeviceSettings,
    }

    impl SdrDevice for InstantFailDevice {
        fn capabilities(&self) -> &Capabilities {
            &self.capabilities
        }

        fn settings(&self) -> &DeviceSettings {
            &self.settings
        }

        fn apply(&mut self, _settings: &DeviceSettings) -> Result<(), DeviceError> {
            Ok(())
        }

        fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
            let mut sink = single_rx_sink(sinks)?;
            sink.fail(DeviceError::Io("died at start".to_string()));
            Ok(())
        }

        fn rx_stop(&mut self) {}
    }

    /// Driver whose probe result can be emptied mid-test, simulating an unplug the capture
    /// thread never notices (the Soapy case).
    struct VanishingDriver {
        present: Arc<AtomicBool>,
    }

    impl DeviceDriver for VanishingDriver {
        fn id(&self) -> &'static str {
            "mock"
        }

        fn probe(&self) -> Vec<DeviceInfo> {
            if self.present.load(Ordering::SeqCst) {
                vec![mock_info("vanish", None)]
            } else {
                Vec::new()
            }
        }

        fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
            Ok(Box::new(SilentDevice {
                capabilities: empty_capabilities(),
                settings: DeviceSettings::default(),
            }))
        }
    }

    /// Device that streams nothing and never raises a fault on its own.
    struct SilentDevice {
        capabilities: Capabilities,
        settings: DeviceSettings,
    }

    impl SdrDevice for SilentDevice {
        fn capabilities(&self) -> &Capabilities {
            &self.capabilities
        }

        fn settings(&self) -> &DeviceSettings {
            &self.settings
        }

        fn apply(&mut self, _settings: &DeviceSettings) -> Result<(), DeviceError> {
            Ok(())
        }

        fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
            single_rx_sink(sinks).map(|_| ())
        }

        fn rx_stop(&mut self) {}
    }

    /// Driver whose device floods the capture ring in a single oversized push before the
    /// DSP thread can drain, guaranteeing a deterministic overrun count.
    struct FloodingDriver;

    impl DeviceDriver for FloodingDriver {
        fn id(&self) -> &'static str {
            "mock"
        }

        fn probe(&self) -> Vec<DeviceInfo> {
            vec![mock_info("flood", None)]
        }

        fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
            Ok(Box::new(FloodingDevice {
                capabilities: empty_capabilities(),
                settings: DeviceSettings::default(),
            }))
        }
    }

    struct FloodingDevice {
        capabilities: Capabilities,
        settings: DeviceSettings,
    }

    impl SdrDevice for FloodingDevice {
        fn capabilities(&self) -> &Capabilities {
            &self.capabilities
        }

        fn settings(&self) -> &DeviceSettings {
            &self.settings
        }

        fn apply(&mut self, _settings: &DeviceSettings) -> Result<(), DeviceError> {
            Ok(())
        }

        fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
            let mut sink = single_rx_sink(sinks)?;
            // 2× the ring in one push: at most RING_CAPACITY fits, the rest must be counted.
            let block = vec![Complex::new(0.0f32, 0.0); crate::runtime::RING_CAPACITY * 2];
            sink.push(&block);
            Ok(())
        }

        fn rx_stop(&mut self) {}
    }

    /// Driver whose device streams small paced blocks until told to die, so tests can fault
    /// a capture mid-recording at a chosen moment.
    struct FaultOnDemandDriver {
        die: Arc<AtomicBool>,
    }

    impl DeviceDriver for FaultOnDemandDriver {
        fn id(&self) -> &'static str {
            "mock"
        }

        fn probe(&self) -> Vec<DeviceInfo> {
            vec![mock_info("ondemand", None)]
        }

        fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
            Ok(Box::new(FaultOnDemandDevice {
                capabilities: empty_capabilities(),
                settings: DeviceSettings::default(),
                die: self.die.clone(),
                stop: Arc::new(AtomicBool::new(false)),
                worker: None,
            }))
        }
    }

    struct FaultOnDemandDevice {
        capabilities: Capabilities,
        settings: DeviceSettings,
        die: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }

    impl SdrDevice for FaultOnDemandDevice {
        fn capabilities(&self) -> &Capabilities {
            &self.capabilities
        }

        fn settings(&self) -> &DeviceSettings {
            &self.settings
        }

        fn apply(&mut self, _settings: &DeviceSettings) -> Result<(), DeviceError> {
            Ok(())
        }

        fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
            let mut sink = single_rx_sink(sinks)?;
            let die = self.die.clone();
            let stop = self.stop.clone();
            self.worker = Some(std::thread::spawn(move || {
                let block = [Complex::new(0.1f32, 0.0); 2_048];
                while !stop.load(Ordering::SeqCst) {
                    if die.load(Ordering::SeqCst) {
                        sink.fail(DeviceError::Io("mock stream died".to_string()));
                        return;
                    }
                    sink.push(&block);
                    std::thread::sleep(Duration::from_millis(2));
                }
            }));
            Ok(())
        }

        fn rx_stop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(handle) = self.worker.take() {
                let _ = handle.join();
            }
        }
    }

    /// Driver whose device blocks inside `apply` for rate-bearing deltas until released,
    /// so tests can hold a rate patch mid-flight deterministically.
    struct BlockingApplyDriver {
        entered_tx: mpsc::Sender<()>,
        release_rx: Mutex<Option<mpsc::Receiver<()>>>,
    }

    impl DeviceDriver for BlockingApplyDriver {
        fn id(&self) -> &'static str {
            "mock"
        }

        fn probe(&self) -> Vec<DeviceInfo> {
            vec![mock_info("blocking", None)]
        }

        fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
            Ok(Box::new(BlockingApplyDevice {
                capabilities: empty_capabilities(),
                settings: DeviceSettings::default(),
                entered_tx: self.entered_tx.clone(),
                release_rx: self.release_rx.lock().unwrap().take(),
            }))
        }
    }

    struct BlockingApplyDevice {
        capabilities: Capabilities,
        settings: DeviceSettings,
        entered_tx: mpsc::Sender<()>,
        release_rx: Option<mpsc::Receiver<()>>,
    }

    impl SdrDevice for BlockingApplyDevice {
        fn capabilities(&self) -> &Capabilities {
            &self.capabilities
        }

        fn settings(&self) -> &DeviceSettings {
            &self.settings
        }

        fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
            if settings.sample_rate.is_some() {
                let _ = self.entered_tx.send(());
                if let Some(rx) = &self.release_rx {
                    let _ = rx.recv();
                }
            }
            Ok(())
        }

        fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
            single_rx_sink(sinks).map(|_| ())
        }

        fn rx_stop(&mut self) {}
    }

    /// Absolute frequency of [`SignalDriver`]'s synthesized carrier.
    const SIGNAL_HZ: f64 = 100_100_000.0;
    /// [`SignalDriver`]'s fixed rate. Small so the spectrum tap's hop (rate/30) is short and a
    /// dwell sees several frames while the mock pushes faster than real time.
    const SIGNAL_RATE_HZ: f64 = 240_000.0;

    /// Driver whose device synthesizes one carrier at a fixed *absolute* frequency: retuning
    /// moves the carrier within the passband and out of it, which is what a scan reacts to.
    /// Without this a scanner test could only assert that it stepped, not that it heard.
    struct SignalDriver;

    impl DeviceDriver for SignalDriver {
        fn id(&self) -> &'static str {
            "mock"
        }

        fn probe(&self) -> Vec<DeviceInfo> {
            vec![mock_info("signal", Some("MOCK-SIG"))]
        }

        fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
            Ok(Box::new(SignalDevice {
                capabilities: Capabilities {
                    freq_ranges: vec![sdrmm_wire::Range {
                        min: 80_000_000.0,
                        max: 120_000_000.0,
                        step: None,
                    }],
                    sample_rates: vec![SIGNAL_RATE_HZ],
                    ..empty_capabilities()
                },
                settings: DeviceSettings {
                    center_hz: Some(100_000_000.0),
                    sample_rate: Some(SIGNAL_RATE_HZ),
                    ..DeviceSettings::default()
                },
                center: Arc::new(Mutex::new(100_000_000.0)),
                stop: Arc::new(AtomicBool::new(false)),
                worker: None,
            }))
        }
    }

    struct SignalDevice {
        capabilities: Capabilities,
        settings: DeviceSettings,
        /// Read by the capture thread every block so a retune takes effect immediately.
        center: Arc<Mutex<f64>>,
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }

    impl SdrDevice for SignalDevice {
        fn capabilities(&self) -> &Capabilities {
            &self.capabilities
        }

        fn settings(&self) -> &DeviceSettings {
            &self.settings
        }

        fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
            if let Some(center) = settings.center_hz {
                *self
                    .center
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = center;
            }
            self.settings.merge_from(settings);
            Ok(())
        }

        fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
            let mut sink = single_rx_sink(sinks)?;
            let center = self.center.clone();
            let stop = self.stop.clone();
            self.worker = Some(std::thread::spawn(move || {
                let mut phase = 0.0f64;
                let mut block = vec![Complex::new(0.0f32, 0.0); 2_048];
                while !stop.load(Ordering::SeqCst) {
                    let center = *center
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let offset = SIGNAL_HZ - center;
                    // Outside the passband the receiver hears nothing at all — the mirror of
                    // a real tuner, and what makes an inactive target read as inactive.
                    if offset.abs() >= SIGNAL_RATE_HZ / 2.0 {
                        block.fill(Complex::new(0.0, 0.0));
                    } else {
                        let step = std::f64::consts::TAU * offset / SIGNAL_RATE_HZ;
                        for slot in &mut block {
                            phase = (phase + step).rem_euclid(std::f64::consts::TAU);
                            *slot =
                                Complex::new(0.5 * phase.cos() as f32, 0.5 * phase.sin() as f32);
                        }
                    }
                    sink.push(&block);
                    std::thread::sleep(Duration::from_millis(2));
                }
            }));
            Ok(())
        }

        fn rx_stop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(handle) = self.worker.take() {
                let _ = handle.join();
            }
        }
    }

    /// Driver whose device quantises what it is asked for, the way real tuners do: a HackRF's
    /// LNA moves in 8 dB steps and an RTL-SDR's resampler lands on achievable ratios, so the
    /// value the hardware holds is routinely not the value that was requested.
    struct SnappingDriver;

    impl DeviceDriver for SnappingDriver {
        fn id(&self) -> &'static str {
            "mock"
        }

        fn probe(&self) -> Vec<DeviceInfo> {
            vec![mock_info("snapping", None)]
        }

        fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
            Ok(Box::new(SnappingDevice {
                capabilities: empty_capabilities(),
                settings: DeviceSettings::default(),
            }))
        }
    }

    struct SnappingDevice {
        capabilities: Capabilities,
        settings: DeviceSettings,
    }

    impl SdrDevice for SnappingDevice {
        fn capabilities(&self) -> &Capabilities {
            &self.capabilities
        }

        fn settings(&self) -> &DeviceSettings {
            &self.settings
        }

        fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
            let mut snapped = settings.clone();
            if let Some(center) = snapped.center_hz {
                snapped.center_hz = Some((center / 1_000_000.0).round() * 1_000_000.0);
            }
            for gain in &mut snapped.gains {
                gain.value_db = (gain.value_db / 8.0).round() * 8.0;
            }
            self.settings.merge_from(&snapped);
            Ok(())
        }

        fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
            single_rx_sink(sinks).map(|_| ())
        }

        fn rx_stop(&mut self) {}
    }

    /// Driver whose device can only be open once at a time — which every USB backend is, and
    /// which is what makes releasing the handle on fault load-bearing for replug recovery.
    struct ExclusiveDriver {
        claimed: Arc<AtomicBool>,
        die: Arc<AtomicBool>,
    }

    impl DeviceDriver for ExclusiveDriver {
        fn id(&self) -> &'static str {
            "mock"
        }

        fn probe(&self) -> Vec<DeviceInfo> {
            vec![mock_info("exclusive", Some("MOCK-X"))]
        }

        fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
            if self.claimed.swap(true, Ordering::SeqCst) {
                return Err(DeviceError::Io("device is busy".to_string()));
            }
            Ok(Box::new(ExclusiveDevice {
                capabilities: empty_capabilities(),
                settings: DeviceSettings::default(),
                claimed: self.claimed.clone(),
                die: self.die.clone(),
                stop: Arc::new(AtomicBool::new(false)),
                worker: None,
            }))
        }
    }

    struct ExclusiveDevice {
        capabilities: Capabilities,
        settings: DeviceSettings,
        claimed: Arc<AtomicBool>,
        die: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }

    impl Drop for ExclusiveDevice {
        fn drop(&mut self) {
            self.claimed.store(false, Ordering::SeqCst);
        }
    }

    impl SdrDevice for ExclusiveDevice {
        fn capabilities(&self) -> &Capabilities {
            &self.capabilities
        }

        fn settings(&self) -> &DeviceSettings {
            &self.settings
        }

        fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
            self.settings.merge_from(settings);
            Ok(())
        }

        fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
            let mut sink = single_rx_sink(sinks)?;
            let die = self.die.clone();
            let stop = self.stop.clone();
            self.worker = Some(std::thread::spawn(move || {
                let block = [Complex::new(0.1f32, 0.0); 2_048];
                while !stop.load(Ordering::SeqCst) {
                    if die.load(Ordering::SeqCst) {
                        sink.fail(DeviceError::Io("mock stream died".to_string()));
                        return;
                    }
                    sink.push(&block);
                    std::thread::sleep(Duration::from_millis(2));
                }
            }));
            Ok(())
        }

        fn rx_stop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(handle) = self.worker.take() {
                let _ = handle.join();
            }
        }
    }

    /// Driver that opens exactly once and then refuses, so a reconnect attempt against a
    /// present-but-claimed device can be driven deterministically.
    struct UnopenableDriver {
        opens: AtomicUsize,
    }

    impl DeviceDriver for UnopenableDriver {
        fn id(&self) -> &'static str {
            "mock"
        }

        fn probe(&self) -> Vec<DeviceInfo> {
            vec![mock_info("refuse", None)]
        }

        fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
            if self.opens.fetch_add(1, Ordering::SeqCst) == 0 {
                return Ok(Box::new(SilentDevice {
                    capabilities: empty_capabilities(),
                    settings: DeviceSettings::default(),
                }));
            }
            Err(DeviceError::Io(
                "still claimed by another process".to_string(),
            ))
        }
    }

    /// Driver whose probe result grows after the first call, simulating an attach.
    struct FlappingDriver {
        probes: AtomicUsize,
    }

    impl DeviceDriver for FlappingDriver {
        fn id(&self) -> &'static str {
            "mock"
        }

        fn probe(&self) -> Vec<DeviceInfo> {
            let n = self.probes.fetch_add(1, Ordering::SeqCst);
            let mut out = vec![mock_info("a", None)];
            if n >= 1 {
                out.push(mock_info("b", None));
            }
            out
        }

        fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
            Err(DeviceError::NotFound(info.id()))
        }
    }

    async fn wait_for_deviceset_event(events: &mut broadcast::Receiver<ServerEvent>, ds: u32) {
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(3), events.recv())
                .await
                .expect("event within timeout")
                .expect("event");
            if matches!(
                ev,
                ServerEvent::StateChanged {
                    scope: StateScope::DeviceSet(id)
                } if id == ds
            ) {
                return;
            }
        }
    }

    #[tokio::test]
    async fn device_fault_surfaces_and_removal_completes() {
        let mut registry = DeviceRegistry::new();
        registry.register(50, Box::new(DyingDriver));
        let engine = Engine::with_registry(registry, None);
        let mut events = engine.subscribe_events();
        let ds = engine.create_device_set("mock:dying").unwrap();

        wait_for_deviceset_event(&mut events, ds).await;

        let snap = engine.snapshot();
        assert_eq!(snap.device_sets[0].status, DeviceSetStatus::Error);
        assert!(
            snap.device_sets[0]
                .error
                .as_deref()
                .unwrap()
                .contains("mock stream died"),
            "fault message must surface: {:?}",
            snap.device_sets[0].error
        );
        // registry.open must have carried the probed info through, not a synthesized one.
        assert_eq!(snap.device_sets[0].device.label, "Mock dying");
        assert_eq!(snap.device_sets[0].device.serial.as_deref(), Some("MOCK-1"));
        // ...but not its probe-time profile: the set reports what the opened radio said, and a
        // second capability answer beside it is one a reader can pick by accident.
        assert!(snap.device_sets[0].device.profile.is_none());

        let removal = {
            let engine = engine.clone();
            tokio::task::spawn_blocking(move || engine.remove_device_set(ds))
        };
        tokio::time::timeout(Duration::from_secs(5), removal)
            .await
            .expect("removal must not hang on a dead capture thread")
            .expect("join")
            .expect("remove ok");
    }

    #[tokio::test]
    async fn fault_raised_before_insert_still_surfaces() {
        let mut registry = DeviceRegistry::new();
        registry.register(50, Box::new(InstantFailDriver));
        let engine = Engine::with_registry(registry, None);
        let ds = engine.create_device_set("mock:instafail").unwrap();

        // The fault was sent before the insert; whether the drainer processed it before the
        // insert (stashed in pending_faults) or after (marked directly), the set must converge
        // to Error instead of staying Running forever.
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let snap = engine.snapshot();
            let set = snap
                .device_sets
                .iter()
                .find(|s| s.id == ds)
                .expect("faulted set must stay listed");
            if set.status == DeviceSetStatus::Error {
                assert!(
                    set.error
                        .as_deref()
                        .expect("error message")
                        .contains("died at start"),
                    "fault message must surface: {:?}",
                    set.error
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "set stuck in {:?} without surfacing the fault",
                set.status
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn probe_disappearance_faults_running_set_after_two_misses() {
        let present = Arc::new(AtomicBool::new(true));
        let mut registry = DeviceRegistry::new();
        registry.register(
            50,
            Box::new(VanishingDriver {
                present: present.clone(),
            }),
        );
        let engine = Engine::with_registry(registry, None);
        let mut events = engine.subscribe_events();
        let ds = engine.create_device_set("mock:vanish").unwrap();

        let mut known = None;
        let mut missing_once = HashSet::new();
        engine.hotplug_tick(&mut known, &mut missing_once);
        assert_eq!(
            engine.snapshot().device_sets[0].status,
            DeviceSetStatus::Running,
            "present device must not be faulted"
        );

        present.store(false, Ordering::SeqCst);
        engine.hotplug_tick(&mut known, &mut missing_once);
        assert_eq!(
            engine.snapshot().device_sets[0].status,
            DeviceSetStatus::Running,
            "one missed probe may be a transient enumerate hiccup"
        );

        engine.hotplug_tick(&mut known, &mut missing_once);
        let snap = engine.snapshot();
        assert_eq!(snap.device_sets[0].status, DeviceSetStatus::Error);
        assert!(
            snap.device_sets[0]
                .error
                .as_deref()
                .unwrap()
                .contains("disappeared from probe"),
            "unplug reason must surface: {:?}",
            snap.device_sets[0].error
        );
        wait_for_deviceset_event(&mut events, ds).await;
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn hotplug_tick_emits_only_on_probe_change() {
        let mut registry = DeviceRegistry::new();
        registry.register(
            50,
            Box::new(FlappingDriver {
                probes: AtomicUsize::new(0),
            }),
        );
        let engine = Engine::with_registry(registry, None);
        let mut events = engine.subscribe_events();

        let mut known = None;
        let mut missing_once = HashSet::new();
        assert!(
            !engine.hotplug_tick(&mut known, &mut missing_once),
            "first probe is baseline"
        );
        assert!(
            engine.hotplug_tick(&mut known, &mut missing_once),
            "attach must be detected"
        );

        let ev = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("event within timeout")
            .expect("event");
        assert!(matches!(
            ev,
            ServerEvent::StateChanged {
                scope: StateScope::Devices
            }
        ));

        assert!(
            !engine.hotplug_tick(&mut known, &mut missing_once),
            "steady state stays quiet"
        );
    }

    /// Hermetic engine: virtual driver only. `Engine::new()` registers the Soapy driver, whose
    /// probe enumerates live system modules — forbidden in tests (PLAN §14: no hardware in CI).
    fn virtual_engine() -> Arc<Engine> {
        let mut registry = DeviceRegistry::new();
        registry.register(VIRTUAL_PRIORITY, Box::new(VirtualDriver::new()));
        Engine::with_registry(registry, None)
    }

    #[tokio::test]
    async fn probes_virtual_device() {
        let engine = virtual_engine();
        assert!(
            engine
                .probe_devices()
                .iter()
                .any(|d| d.id() == "virtual:siggen")
        );
    }

    #[tokio::test]
    async fn spectrum_flows_with_a_visible_tone() {
        let engine = virtual_engine();
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        let mut rx = engine.subscribe_spectrum(ds, 0).unwrap();

        let snap = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("spectrum within timeout")
            .expect("snapshot");
        assert_eq!(snap.db.len(), 4096);

        let mut sorted = snap.db.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = sorted[sorted.len() / 2];
        let peak = *sorted.last().unwrap();
        assert!(
            peak - median > 20.0,
            "expected tone peak above floor (peak {peak}, median {median})"
        );

        engine.remove_device_set(ds).unwrap();
        assert!(engine.snapshot().device_sets.is_empty());
    }

    #[tokio::test]
    async fn create_emits_state_changed() {
        let engine = virtual_engine();
        let mut events = engine.subscribe_events();
        let ds = engine.create_device_set("virtual:siggen").unwrap();

        let ev = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("event within timeout")
            .expect("event");
        assert!(matches!(
            ev,
            ServerEvent::StateChanged {
                scope: StateScope::All
            }
        ));
        engine.remove_device_set(ds).unwrap();
    }

    fn nfm_settings(offset_hz: f64) -> ChannelSettings {
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            params: ChannelParams::Nfm(NfmParams::default()),
        }
    }

    #[tokio::test]
    async fn channel_crud_updates_state() {
        let engine = virtual_engine();
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        let ch = engine.add_channel(ds, 0, nfm_settings(0.0)).unwrap();
        assert_eq!(engine.snapshot().device_sets[0].channels.len(), 1);
        engine.remove_channel(ds, ch).unwrap();
        assert!(engine.snapshot().device_sets[0].channels.is_empty());
        assert!(engine.remove_channel(ds, 999).is_err());
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn add_channel_rejects_out_of_passband_offset() {
        let engine = virtual_engine();
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        // Default rate 2.048 Msps → ±1.024 MHz passband.
        let err = engine
            .add_channel(ds, 0, nfm_settings(1_100_000.0))
            .unwrap_err();
        assert!(err.is_bad_request(), "expected bad request, got {err}");
        assert!(engine.snapshot().device_sets[0].channels.is_empty());
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn patch_channel_rejects_missing_channel() {
        let engine = virtual_engine();
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        let err = engine.patch_channel(ds, 7, nfm_settings(0.0)).unwrap_err();
        assert!(err.is_not_found(), "expected not found, got {err}");
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn rate_change_stranding_a_channel_is_rejected_before_device_io() {
        let engine = virtual_engine();
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        engine.add_channel(ds, 0, nfm_settings(900_000.0)).unwrap();
        // At 250 ksps the ±125 kHz passband cannot contain a channel at +900 kHz.
        let err = engine
            .patch_device(
                ds,
                DeviceSettings {
                    sample_rate: Some(250_000.0),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(err.is_bad_request(), "expected bad request, got {err}");
        // The rejected patch must not have reached the device.
        assert_eq!(
            engine.snapshot().device_sets[0].settings.sample_rate,
            Some(2_048_000.0)
        );
        engine.remove_device_set(ds).unwrap();
    }

    /// The engine used to send a rate-change rebuild's Remove+Add from a stale snapshot
    /// outside `inner`: a concurrent DELETE could interleave, its channel got re-added on
    /// the DSP thread as a zombie holding a live PCM sender, and the DELETE's encoder join
    /// hung forever. Commands now go out under `inner` with membership re-checked, so this
    /// loop must never wedge.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_rate_rebuild_and_remove_never_strands_a_channel() {
        let engine = virtual_engine();
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        for i in 0..40u32 {
            let ch = engine.add_channel(ds, 0, nfm_settings(100_000.0)).unwrap();
            let rate = if i % 2 == 0 { 2_400_000.0 } else { 2_048_000.0 };
            let patch = {
                let engine = engine.clone();
                tokio::task::spawn_blocking(move || {
                    engine.patch_device(
                        ds,
                        DeviceSettings {
                            sample_rate: Some(rate),
                            ..Default::default()
                        },
                    )
                })
            };
            let remove = {
                let engine = engine.clone();
                tokio::task::spawn_blocking(move || engine.remove_channel(ds, ch))
            };
            let (patch, remove) = tokio::time::timeout(Duration::from_secs(10), async {
                tokio::join!(patch, remove)
            })
            .await
            .unwrap_or_else(|_| panic!("iteration {i}: patch_device/remove_channel deadlocked"));
            patch.expect("join").expect("patch ok");
            remove.expect("join").expect("remove ok");
            assert!(
                engine.snapshot().device_sets[0].channels.is_empty(),
                "iteration {i}: channel survived its removal"
            );
        }
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn ring_overrun_surfaces_in_state_and_emits_event() {
        let mut registry = DeviceRegistry::new();
        registry.register(50, Box::new(FloodingDriver));
        let engine = Engine::with_registry(registry, None);
        let mut events = engine.subscribe_events();
        let ds = engine.create_device_set("mock:flood").unwrap();

        let snap = engine.snapshot();
        assert!(
            snap.device_sets[0].overruns >= crate::runtime::RING_CAPACITY as u64,
            "flooded ring must report drops, got {}",
            snap.device_sets[0].overruns
        );

        let mut known = None;
        let mut missing_once = HashSet::new();
        engine.hotplug_tick(&mut known, &mut missing_once);
        wait_for_deviceset_event(&mut events, ds).await;

        // No further growth: the next tick must stay quiet instead of re-announcing.
        let mut quiet = engine.subscribe_events();
        engine.hotplug_tick(&mut known, &mut missing_once);
        assert!(
            matches!(quiet.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
            "tick without overrun growth must not emit"
        );
        engine.remove_device_set(ds).unwrap();
    }

    /// Hermetic recording engine: virtual driver + a scoped temp recordings dir shared by
    /// `start_recording` and the driver's playback probe.
    fn recording_engine(dir: &Path) -> Arc<Engine> {
        let mut registry = DeviceRegistry::new();
        registry.register(
            VIRTUAL_PRIORITY,
            Box::new(VirtualDriver::with_recordings(dir.to_path_buf())),
        );
        Engine::with_registry(registry, Some(dir.to_path_buf()))
    }

    /// The virtual device is real-time paced, so recording progress needs polling.
    async fn wait_for_recorded_samples(engine: &Engine, ds: u32, min: u64) -> RecordingStatus {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let snap = engine.snapshot();
            let recording = snap
                .device_sets
                .iter()
                .find(|s| s.id == ds)
                .expect("set listed")
                .recording
                .clone();
            if let Some(rec) = recording
                && rec.samples >= min
            {
                return rec;
            }
            assert!(
                Instant::now() < deadline,
                "recording never reached {min} samples"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn record_start_stop_produces_a_finalized_sigmf_pair() {
        let dir = tempfile::TempDir::new().unwrap();
        let engine = recording_engine(dir.path());
        let mut events = engine.subscribe_events();
        let ds = engine.create_device_set("virtual:siggen").unwrap();

        engine.start_recording(ds, 0).unwrap();
        wait_for_deviceset_event(&mut events, ds).await;
        let live = wait_for_recorded_samples(&engine, ds, 1).await;
        assert!(!live.file.is_empty());
        live.started_at.parse::<jiff::Timestamp>().unwrap();
        assert_eq!(live.error, None);

        let finalized = engine.stop_recording(ds).unwrap();
        assert_eq!(finalized.error, None);
        assert!(finalized.samples > 0);
        assert_eq!(
            finalized.bytes,
            finalized.samples * sdrmm_recorder::BYTES_PER_SAMPLE
        );
        assert!(engine.snapshot().device_sets[0].recording.is_none());

        let reader = sdrmm_recorder::SigmfReader::open(&finalized.stem).unwrap();
        assert_eq!(reader.total_samples(), finalized.samples);
        assert_eq!(reader.meta().global.sample_rate, Some(2_048_000.0));
        assert_eq!(reader.meta().captures[0].frequency, Some(100_000_000.0));

        // The finalized pair is immediately probeable as a playback device.
        let playback_id = format!("virtual:file:{}", finalized.stem.display());
        assert!(engine.probe_devices().iter().any(|d| d.id() == playback_id));
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn double_start_and_idle_stop_are_rejected() {
        let dir = tempfile::TempDir::new().unwrap();
        let engine = recording_engine(dir.path());
        let ds = engine.create_device_set("virtual:siggen").unwrap();

        let err = engine.stop_recording(ds).unwrap_err();
        assert!(err.is_bad_request(), "expected bad request, got {err}");

        engine.start_recording(ds, 0).unwrap();
        let err = engine.start_recording(ds, 0).unwrap_err();
        assert!(err.is_bad_request(), "expected bad request, got {err}");

        engine.stop_recording(ds).unwrap();
        let err = engine.stop_recording(ds).unwrap_err();
        assert!(err.is_bad_request(), "expected bad request, got {err}");
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn start_without_a_recordings_dir_is_rejected() {
        let engine = virtual_engine();
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        let err = engine.start_recording(ds, 0).unwrap_err();
        assert!(err.is_bad_request(), "expected bad request, got {err}");
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn rate_patch_is_rejected_while_recording_center_retune_is_captured() {
        let dir = tempfile::TempDir::new().unwrap();
        let engine = recording_engine(dir.path());
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        engine.start_recording(ds, 0).unwrap();
        let before = wait_for_recorded_samples(&engine, ds, 1).await;

        let err = engine
            .patch_device(
                ds,
                DeviceSettings {
                    sample_rate: Some(2_400_000.0),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(err.is_bad_request(), "expected bad request, got {err}");
        let snap = engine.snapshot();
        assert_eq!(snap.device_sets[0].settings.sample_rate, Some(2_048_000.0));
        assert!(
            snap.device_sets[0].recording.is_some(),
            "rejected patch must not kill the recording"
        );

        // A center retune stays allowed and lands as a capture segment. Blocks are stamped
        // with the meta center at drain time, so waiting out a full ring of samples (the
        // largest possible in-flight drain) plus margin guarantees post-retune blocks.
        engine
            .patch_device(
                ds,
                DeviceSettings {
                    center_hz: Some(88_500_000.0),
                    ..Default::default()
                },
            )
            .unwrap();
        wait_for_recorded_samples(
            &engine,
            ds,
            before.samples + crate::runtime::RING_CAPACITY as u64 + 200_000,
        )
        .await;
        let finalized = engine.stop_recording(ds).unwrap();
        engine.remove_device_set(ds).unwrap();

        let reader = sdrmm_recorder::SigmfReader::open(&finalized.stem).unwrap();
        let captures = &reader.meta().captures;
        assert_eq!(captures.len(), 2, "retune must append one capture segment");
        assert_eq!(captures[1].frequency, Some(88_500_000.0));
        assert!(captures[1].sample_start > 0);
    }

    #[tokio::test]
    async fn device_fault_finalizes_the_recording() {
        let dir = tempfile::TempDir::new().unwrap();
        let die = Arc::new(AtomicBool::new(false));
        let mut registry = DeviceRegistry::new();
        registry.register(50, Box::new(FaultOnDemandDriver { die: die.clone() }));
        let engine = Engine::with_registry(registry, Some(dir.path().to_path_buf()));
        let ds = engine.create_device_set("mock:ondemand").unwrap();

        engine.start_recording(ds, 0).unwrap();
        let live = wait_for_recorded_samples(&engine, ds, 1).await;

        // The fault event is emitted only after the writer join, so the pair is finalized
        // once it arrives. The implicit stop must also announce the Recordings scope, or
        // clients never refetch the library for a fault-stopped recording.
        let mut events = engine.subscribe_events();
        die.store(true, Ordering::SeqCst);
        let mut saw_recordings = false;
        let mut saw_device_set = false;
        while !(saw_recordings && saw_device_set) {
            let ev = tokio::time::timeout(Duration::from_secs(3), events.recv())
                .await
                .expect("event within timeout")
                .expect("event");
            match ev {
                ServerEvent::StateChanged {
                    scope: StateScope::Recordings,
                } => saw_recordings = true,
                ServerEvent::StateChanged {
                    scope: StateScope::DeviceSet(id),
                } if id == ds => saw_device_set = true,
                _ => {}
            }
        }

        let snap = engine.snapshot();
        assert_eq!(snap.device_sets[0].status, DeviceSetStatus::Error);
        assert!(
            snap.device_sets[0].recording.is_none(),
            "fault must finalize and clear the recording"
        );
        let reader = sdrmm_recorder::SigmfReader::open(&dir.path().join(&live.file)).unwrap();
        assert!(reader.total_samples() > 0);
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn recording_growth_rides_the_hotplug_tick() {
        let dir = tempfile::TempDir::new().unwrap();
        let engine = recording_engine(dir.path());
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        engine.start_recording(ds, 0).unwrap();
        wait_for_recorded_samples(&engine, ds, 1).await;

        let mut events = engine.subscribe_events();
        let mut known = None;
        let mut missing_once = HashSet::new();
        engine.hotplug_tick(&mut known, &mut missing_once);
        wait_for_deviceset_event(&mut events, ds).await;

        engine.stop_recording(ds).unwrap();
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn start_during_rate_patch_cannot_commit_a_wrong_rate_recording() {
        let dir = tempfile::TempDir::new().unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut registry = DeviceRegistry::new();
        registry.register(
            50,
            Box::new(BlockingApplyDriver {
                entered_tx,
                release_rx: Mutex::new(Some(release_rx)),
            }),
        );
        let engine = Engine::with_registry(registry, Some(dir.path().to_path_buf()));
        let ds = engine.create_device_set("mock:blocking").unwrap();

        let patch = {
            let engine = engine.clone();
            tokio::task::spawn_blocking(move || {
                engine.patch_device(
                    ds,
                    DeviceSettings {
                        sample_rate: Some(2_400_000.0),
                        ..Default::default()
                    },
                )
            })
        };
        // The device is now blocked inside `apply`, with the pre-validation (and the
        // rate-patch claim) already committed.
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let err = engine.start_recording(ds, 0).unwrap_err();
        assert!(err.is_bad_request(), "expected bad request, got {err}");
        assert!(err.to_string().contains("in flight"), "{err}");
        // The rejected attempt must leave no files behind.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);

        release_tx.send(()).unwrap();
        patch.await.expect("join").expect("patch ok");
        let snap = engine.snapshot();
        assert_eq!(snap.device_sets[0].settings.sample_rate, Some(2_400_000.0));
        assert!(snap.device_sets[0].recording.is_none());

        // Once the patch merged, recording works again — at the new rate.
        engine.start_recording(ds, 0).unwrap();
        let finalized = engine.stop_recording(ds).unwrap();
        let reader = sdrmm_recorder::SigmfReader::open(&finalized.stem).unwrap();
        assert_eq!(reader.meta().global.sample_rate, Some(2_400_000.0));
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn engine_drop_finalizes_a_live_recording() {
        let dir = tempfile::TempDir::new().unwrap();
        let engine = recording_engine(dir.path());
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        engine.start_recording(ds, 0).unwrap();
        let live = wait_for_recorded_samples(&engine, ds, 1).await;

        drop(engine);

        let stem = dir.path().join(&live.file);
        assert!(
            sdrmm_recorder::meta_path(&stem).exists(),
            "drop must join the writer and finalize the pair"
        );
        assert!(
            !dir.path()
                .join(format!("{}.sigmf-meta.tmp", live.file))
                .exists(),
            "no breadcrumb may survive an orderly teardown"
        );
        let reader = sdrmm_recorder::SigmfReader::open(&stem).unwrap();
        assert!(reader.total_samples() > 0);
    }

    #[tokio::test]
    async fn shutdown_finalizes_recordings_emits_scopes_and_is_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let engine = recording_engine(dir.path());
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        engine.start_recording(ds, 0).unwrap();
        let live = wait_for_recorded_samples(&engine, ds, 1).await;

        let mut events = engine.subscribe_events();
        engine.shutdown();
        assert!(engine.snapshot().device_sets.is_empty());
        let mut saw_all = false;
        let mut saw_recordings = false;
        while !(saw_all && saw_recordings) {
            let ev = tokio::time::timeout(Duration::from_secs(3), events.recv())
                .await
                .expect("event within timeout")
                .expect("event");
            match ev {
                ServerEvent::StateChanged {
                    scope: StateScope::All,
                } => saw_all = true,
                ServerEvent::StateChanged {
                    scope: StateScope::Recordings,
                } => saw_recordings = true,
                _ => {}
            }
        }
        sdrmm_recorder::SigmfReader::open(&dir.path().join(&live.file)).unwrap();

        // Second call (and the Drop-driven third) must be no-ops, not double teardowns.
        engine.shutdown();
        drop(engine);
    }

    #[tokio::test]
    async fn writer_fault_surfaces_in_state_via_the_hotplug_tick() {
        let dir = tempfile::TempDir::new().unwrap();
        let engine = recording_engine(dir.path());
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        engine.start_recording(ds, 0).unwrap();
        wait_for_recorded_samples(&engine, ds, 1).await;

        // No portable way to make a live writer's disk I/O fail on demand; inject through
        // the same shared cell `write_loop` reports into.
        {
            let mut inner = engine.lock();
            let state = inner.device_sets.get_mut(&ds).unwrap();
            state
                .recording
                .as_ref()
                .unwrap()
                .shared
                .fail("recording write failed: injected".to_string());
        }

        let mut events = engine.subscribe_events();
        let mut known = None;
        let mut missing_once = HashSet::new();
        engine.hotplug_tick(&mut known, &mut missing_once);
        wait_for_deviceset_event(&mut events, ds).await;

        let rec = engine.snapshot().device_sets[0].recording.clone().unwrap();
        assert_eq!(
            rec.error.as_deref(),
            Some("recording write failed: injected")
        );

        let finalized = engine.stop_recording(ds).unwrap();
        assert_eq!(
            finalized.error.as_deref(),
            Some("recording write failed: injected")
        );
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn record_start_on_a_missing_set_is_not_found_even_without_a_recordings_dir() {
        let engine = virtual_engine();
        let err = engine.start_recording(99, 0).unwrap_err();
        assert!(err.is_not_found(), "expected not found, got {err}");
    }

    #[tokio::test]
    async fn record_start_io_failure_is_a_server_error_not_a_bad_request() {
        // The recordings dir nests under a regular file, so create_dir_all must fail.
        let blocker = tempfile::NamedTempFile::new().unwrap();
        let mut registry = DeviceRegistry::new();
        registry.register(VIRTUAL_PRIORITY, Box::new(VirtualDriver::new()));
        let engine = Engine::with_registry(registry, Some(blocker.path().join("recordings")));
        let ds = engine.create_device_set("virtual:siggen").unwrap();

        let err = engine.start_recording(ds, 0).unwrap_err();
        assert!(matches!(err, EngineError::RecordingIo(_)), "got {err}");
        assert!(!err.is_bad_request() && !err.is_not_found());
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn validate_honors_configured_bandwidth_and_sideband() {
        let engine = virtual_engine();
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        engine
            .patch_device(
                ds,
                DeviceSettings {
                    sample_rate: Some(250_000.0),
                    ..Default::default()
                },
            )
            .unwrap();

        let usb = |offset_hz: f64| ChannelSettings {
            offset_hz,
            squelch_db: None,
            params: ChannelParams::Ssb(SsbParams {
                sideband: Sideband::Usb,
                bandwidth_hz: 10_000.0,
                agc: true,
            }),
        };
        let wide_nfm = |offset_hz: f64| ChannelSettings {
            offset_hz,
            squelch_db: None,
            params: ChannelParams::Nfm(NfmParams {
                bandwidth_hz: 25_000.0,
            }),
        };

        // USB at +120 kHz occupies +120.1…+130 kHz — past the +125 kHz Nyquist edge even
        // though the descriptor-nominal ±1.5 kHz check would pass it.
        let err = engine.add_channel(ds, 0, usb(120_000.0)).unwrap_err();
        assert!(err.is_bad_request(), "expected bad request, got {err}");
        // A 25 kHz NFM at +118 kHz reaches +130.5 kHz.
        let err = engine.add_channel(ds, 0, wide_nfm(118_000.0)).unwrap_err();
        assert!(err.is_bad_request(), "expected bad request, got {err}");
        assert!(engine.snapshot().device_sets[0].channels.is_empty());

        // The same configs fit once their occupied band stays inside the passband — the
        // check must not become a blunt nominal-width rejection.
        engine.add_channel(ds, 0, usb(-124_000.0)).unwrap();
        engine.add_channel(ds, 0, wide_nfm(112_000.0)).unwrap();
        engine.remove_device_set(ds).unwrap();
    }

    #[tokio::test]
    async fn patch_retunes_without_error() {
        let engine = virtual_engine();
        let ds = engine.create_device_set("virtual:siggen").unwrap();
        engine
            .patch_device(
                ds,
                DeviceSettings {
                    center_hz: Some(88_500_000.0),
                    sample_rate: Some(2_400_000.0),
                    ..Default::default()
                },
            )
            .unwrap();
        let snap = engine.snapshot();
        assert_eq!(snap.device_sets[0].settings.center_hz, Some(88_500_000.0));
        assert_eq!(snap.device_sets[0].settings.sample_rate, Some(2_400_000.0));
        engine.remove_device_set(ds).unwrap();
    }

    /// A device set that faulted and whose device is attached again must come back with its
    /// tuning and its channels — including live audio subscriptions, which is the whole point
    /// of preserving the channel's PCM identity across the swap (PLAN §16 M5).
    #[tokio::test]
    async fn faulted_set_reconnects_and_restores_its_channels() {
        let die = Arc::new(AtomicBool::new(false));
        let mut registry = DeviceRegistry::new();
        registry.register(50, Box::new(FaultOnDemandDriver { die: die.clone() }));
        let engine = Engine::with_registry(registry, None);
        let ds = engine.create_device_set("mock:ondemand").unwrap();
        engine
            .patch_device(
                ds,
                DeviceSettings {
                    center_hz: Some(145_000_000.0),
                    ..DeviceSettings::default()
                },
            )
            .unwrap();
        let ch = engine
            .add_channel(
                ds,
                0,
                ChannelSettings {
                    offset_hz: 25_000.0,
                    squelch_db: None,
                    params: ChannelParams::Nfm(NfmParams::default()),
                },
            )
            .unwrap();
        let mut audio = engine.subscribe_audio(ds, ch).unwrap();

        // Subscribe only now: `patch_device` and `add_channel` emit this same scope, so an
        // earlier subscription would satisfy the wait below before the device ever died.
        let mut events = engine.subscribe_events();
        die.store(true, Ordering::SeqCst);
        loop {
            wait_for_deviceset_event(&mut events, ds).await;
            if engine.snapshot().device_sets[0].status == DeviceSetStatus::Error {
                break;
            }
        }

        // The device is attached again; the next probe tick is what notices.
        die.store(false, Ordering::SeqCst);
        let mut known = None;
        let mut missing_once = HashSet::new();
        engine.hotplug_tick(&mut known, &mut missing_once);

        let set = &engine.snapshot().device_sets[0];
        assert_eq!(set.status, DeviceSetStatus::Running);
        assert_eq!(set.error, None);
        assert_eq!(set.settings.center_hz, Some(145_000_000.0));
        assert_eq!(set.channels.len(), 1);
        assert_eq!(set.channels[0].id, ch);
        assert_eq!(set.channels[0].settings.offset_hz, 25_000.0);

        // The rebuilt pipeline feeds the same encoder, so a subscription taken before the
        // fault keeps delivering without being re-established.
        let packet = tokio::time::timeout(Duration::from_secs(10), audio.recv())
            .await
            .expect("audio within timeout")
            .expect("audio packet after reconnect");
        assert!(!packet.opus.is_empty());
        engine.remove_device_set(ds).unwrap();
    }

    /// The client renders `DeviceSet.settings` as the truth about the radio, so a patch must
    /// report what the device *holds*, not what was asked for. Found on a HackRF: asking for
    /// 13 dB of LNA gain (a value its 8 dB grid cannot express) reported 13 dB back while the
    /// radio sat at 16.
    #[tokio::test]
    async fn a_patch_reports_what_the_device_holds_not_what_was_asked() {
        let mut registry = DeviceRegistry::new();
        registry.register(50, Box::new(SnappingDriver));
        let engine = Engine::with_registry(registry, None);
        let ds = engine.create_device_set("mock:snapping").unwrap();

        engine
            .patch_device(
                ds,
                DeviceSettings {
                    center_hz: Some(100_400_000.0),
                    gains: vec![sdrmm_wire::GainValue {
                        stage: "LNA".to_string(),
                        value_db: 13.0,
                    }],
                    ..DeviceSettings::default()
                },
            )
            .unwrap();

        let set = &engine.snapshot().device_sets[0];
        assert_eq!(set.settings.center_hz, Some(100_000_000.0));
        assert_eq!(
            set.settings
                .gains
                .iter()
                .find(|g| g.stage == "LNA")
                .map(|g| g.value_db),
            Some(16.0),
            "the request was echoed instead of the device's own value"
        );

        // A field the device reports nothing about must survive: the request is the base, and
        // only what the device actually speaks for is laid over it.
        engine
            .patch_device(
                ds,
                DeviceSettings {
                    antenna: Some("RX2".to_string()),
                    ..DeviceSettings::default()
                },
            )
            .unwrap();
        let set = &engine.snapshot().device_sets[0];
        assert_eq!(set.settings.antenna.as_deref(), Some("RX2"));
        assert_eq!(set.settings.center_hz, Some(100_000_000.0));
        engine.remove_device_set(ds).unwrap();
    }

    /// A faulted set must let go of its device. Every USB backend claims its interface for as
    /// long as the handle lives, so a set that kept it would make the replug recovery try to
    /// re-open a radio it is itself still holding — and fail, forever.
    #[tokio::test]
    async fn a_faulted_set_releases_its_device_so_the_replug_can_reopen_it() {
        let claimed = Arc::new(AtomicBool::new(false));
        let die = Arc::new(AtomicBool::new(false));
        let mut registry = DeviceRegistry::new();
        registry.register(
            50,
            Box::new(ExclusiveDriver {
                claimed: claimed.clone(),
                die: die.clone(),
            }),
        );
        let engine = Engine::with_registry(registry, None);
        let ds = engine.create_device_set("mock:exclusive").unwrap();
        assert!(claimed.load(Ordering::SeqCst), "the open must claim it");

        let mut events = engine.subscribe_events();
        die.store(true, Ordering::SeqCst);
        loop {
            wait_for_deviceset_event(&mut events, ds).await;
            if engine.snapshot().device_sets[0].status == DeviceSetStatus::Error {
                break;
            }
        }
        assert!(
            !claimed.load(Ordering::SeqCst),
            "the faulted set is still holding the device"
        );

        die.store(false, Ordering::SeqCst);
        let mut known = None;
        let mut missing_once = HashSet::new();
        engine.hotplug_tick(&mut known, &mut missing_once);
        let set = &engine.snapshot().device_sets[0];
        assert_eq!(set.status, DeviceSetStatus::Running, "{:?}", set.error);
        assert!(claimed.load(Ordering::SeqCst));
        engine.remove_device_set(ds).unwrap();
    }

    /// A device that stays unopenable must not thrash: the set keeps its live reason and the
    /// retry emits only when that reason changes (clients refetch on every emit).
    #[tokio::test]
    async fn reconnect_failure_reports_once_and_keeps_the_set_faulted() {
        let mut registry = DeviceRegistry::new();
        registry.register(
            50,
            Box::new(UnopenableDriver {
                opens: AtomicUsize::new(0),
            }),
        );
        let engine = Engine::with_registry(registry, None);
        let ds = engine.create_device_set("mock:refuse").unwrap();
        engine.mark_device_fault(ds, DeviceError::Io("unplugged".to_string()));
        let mut events = engine.subscribe_events();

        let mut known = None;
        let mut missing_once = HashSet::new();
        engine.hotplug_tick(&mut known, &mut missing_once);
        let set = &engine.snapshot().device_sets[0];
        assert_eq!(set.status, DeviceSetStatus::Error);
        let reported = set.error.clone().expect("reason");
        assert!(
            reported.contains("not reopenable") && reported.contains("still claimed"),
            "unhelpful reason: {reported}"
        );
        assert!(
            events.try_recv().is_ok(),
            "the first failure must reach clients"
        );

        // Second identical failure: same reason, so no further invalidation.
        while events.try_recv().is_ok() {}
        engine.hotplug_tick(&mut known, &mut missing_once);
        assert!(
            events.try_recv().is_err(),
            "an unchanged reason must not re-invalidate every client"
        );
        engine.remove_device_set(ds).unwrap();
    }

    /// End-to-end scan against a synthesized carrier: the sweep must find it, park on it,
    /// retune the hold channel onto it, and refuse client retunes while it owns the device.
    #[tokio::test]
    async fn scan_finds_a_carrier_holds_and_owns_the_tuning() {
        let mut registry = DeviceRegistry::new();
        registry.register(50, Box::new(SignalDriver));
        let engine = Engine::with_registry(registry, None);
        let ds = engine.create_device_set("mock:signal").unwrap();
        let ch = engine
            .add_channel(
                ds,
                0,
                ChannelSettings {
                    offset_hz: 0.0,
                    squelch_db: None,
                    params: ChannelParams::Nfm(NfmParams::default()),
                },
            )
            .unwrap();

        let settings = sdrmm_wire::ScanSettings {
            ranges: vec![sdrmm_wire::ScanRange {
                start_hz: 100_000_000.0,
                stop_hz: 100_200_000.0,
                step_hz: 25_000.0,
            }],
            threshold_db: -60.0,
            dwell_ms: 60,
            resume_ms: 60_000,
            hold_channel: Some(ch),
            ..sdrmm_wire::ScanSettings::default()
        };
        let status = engine.start_scan(ds, settings).unwrap();
        assert_eq!(status.targets, 9);

        // While a scan owns the tuning, a client retune is refused rather than fought over.
        let err = engine
            .patch_device(
                ds,
                DeviceSettings {
                    center_hz: Some(101_000_000.0),
                    ..DeviceSettings::default()
                },
            )
            .unwrap_err();
        assert!(err.is_bad_request(), "expected bad request, got {err}");
        assert!(
            engine
                .start_scan(ds, sdrmm_wire::ScanSettings::default())
                .is_err()
        );

        let deadline = Instant::now() + Duration::from_secs(20);
        let held = loop {
            let set = &engine.snapshot().device_sets[0];
            let scanner = set.scanner.clone().expect("scan listed on the set");
            assert_eq!(scanner.error, None, "scan failed");
            if scanner.state == ScanState::Holding {
                break (
                    scanner,
                    set.settings.center_hz.expect("center"),
                    set.channels[0].settings.offset_hz,
                );
            }
            assert!(Instant::now() < deadline, "scan never found the carrier");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        let (scanner, center_hz, offset_hz) = held;
        assert_eq!(scanner.current_hz, SIGNAL_HZ);
        assert!(scanner.hits >= 1);
        // The hold channel follows the hit, so its audio is the signal the scan stopped on.
        assert!(
            (center_hz + offset_hz - SIGNAL_HZ).abs() < 1.0,
            "hold channel parked at {} Hz, carrier at {SIGNAL_HZ} Hz",
            center_hz + offset_hz
        );

        let final_status = engine.stop_scan(ds).unwrap();
        assert_eq!(final_status.state, ScanState::Holding);
        assert!(
            engine.stop_scan(ds).is_err(),
            "double stop must be an error"
        );
        assert!(engine.snapshot().device_sets[0].scanner.is_none());
        // The tuning is the client's again once the scan lets go.
        engine
            .patch_device(
                ds,
                DeviceSettings {
                    center_hz: Some(101_000_000.0),
                    ..DeviceSettings::default()
                },
            )
            .unwrap();
        engine.remove_device_set(ds).unwrap();
    }

    /// Removing a set with a scan running must not hang: the scan thread takes the engine
    /// lock on every step, so teardown has to signal it and join outside that lock.
    #[tokio::test]
    async fn removing_a_scanning_set_tears_the_scan_down() {
        let mut registry = DeviceRegistry::new();
        registry.register(50, Box::new(SignalDriver));
        let engine = Engine::with_registry(registry, None);
        let ds = engine.create_device_set("mock:signal").unwrap();
        engine
            .start_scan(
                ds,
                sdrmm_wire::ScanSettings {
                    ranges: vec![sdrmm_wire::ScanRange {
                        start_hz: 100_000_000.0,
                        stop_hz: 100_400_000.0,
                        step_hz: 25_000.0,
                    }],
                    // Never trips, so the sweep keeps retuning for the whole test.
                    threshold_db: 100.0,
                    dwell_ms: 40,
                    ..sdrmm_wire::ScanSettings::default()
                },
            )
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        engine.remove_device_set(ds).unwrap();
        assert!(engine.snapshot().device_sets.is_empty());
    }

    #[tokio::test]
    async fn scan_rejects_targets_the_tuner_cannot_reach() {
        let mut registry = DeviceRegistry::new();
        registry.register(50, Box::new(SignalDriver));
        let engine = Engine::with_registry(registry, None);
        let ds = engine.create_device_set("mock:signal").unwrap();
        let err = engine
            .start_scan(
                ds,
                sdrmm_wire::ScanSettings {
                    frequencies: vec![2_400_000_000.0],
                    ..sdrmm_wire::ScanSettings::default()
                },
            )
            .unwrap_err();
        assert!(err.is_bad_request(), "expected bad request, got {err}");
        assert!(
            err.to_string().contains("tuning range"),
            "unhelpful message: {err}"
        );
        // A hold channel that does not exist is a not-found, not a silent scan without audio.
        let err = engine
            .start_scan(
                ds,
                sdrmm_wire::ScanSettings {
                    frequencies: vec![100_000_000.0],
                    hold_channel: Some(42),
                    ..sdrmm_wire::ScanSettings::default()
                },
            )
            .unwrap_err();
        assert!(err.is_not_found(), "expected not found, got {err}");
        engine.remove_device_set(ds).unwrap();
    }
}
