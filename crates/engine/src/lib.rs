use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration,
};

use sdrmm_channels::{ChannelCtx, ChannelError};
use sdrmm_device::{DeviceError, DeviceRegistry, PlaybackShared, check_stream_settings};
use sdrmm_device_virtual::VirtualDriver;
use sdrmm_recorder::{data_path, meta_path};
use sdrmm_wire::{
    Capabilities, ChannelDescriptor, ChannelInfo, ChannelLevel, ChannelParams, ChannelSettings,
    DecodedRecord, DeviceInfo, DeviceSet, DeviceSetStatus, DeviceSettings, PlaybackRequest,
    PlaybackStatus, PositionFix, RecordingStatus, ScanSettings, ScannerStatus, ServerEvent,
    StateScope, StateSnapshot, TrunkSystemStatus,
};
use tokio::sync::broadcast;

pub mod audio;
pub mod iq;
pub mod occupancy;
mod position;
pub mod recording;
pub mod runtime;
pub mod scanner;
mod spectrum;
pub mod trunking;
pub mod video;
pub use audio::{AudioPacket, PcmBlock, PcmPayload};
pub use iq::{IQ_BLOCK_SAMPLES, IQ_BLOCKS_PER_SEC, IqBlock};
pub use recording::FinalizedRecording;
pub use runtime::SpectrumSnapshot;
pub use trunking::TrunkSystem;
pub use video::VideoPacket;

use crate::{
    recording::RecordingShared,
    runtime::{CaptureRuntime, ChannelHost, ChannelSinks, DecodedSink, DspCommand, RawDecoded},
    scanner::{ScanPlan, ScannerState},
};

/// Merge priority for the built-in virtual driver.
const VIRTUAL_PRIORITY: u8 = 10;
/// Soapy sits above virtual.
#[cfg(feature = "soapy")]
const SOAPY_PRIORITY: u8 = 20;
/// The network clients sit beside the native backends, and the merge never reaches them: a remote
/// receiver reports no serial, because what identifies it is the endpoint it answers on and not
/// the hardware at the far end. Collapsing it into a local radio of the same serial would bind a
/// device node to a different antenna in a different room.
#[cfg(feature = "net-client")]
const NET_PRIORITY: u8 = 30;
const EVENT_CHANNEL_CAP: usize = 256;
const DECODED_QUEUE_CAP: usize = 4096;
const DECODED_CHANNEL_CAP: usize = 1024;
/// Fallbacks for devices that report no tuning/rate; mirrored wherever settings are read.
const DEFAULT_CENTER_HZ: f64 = 100_000_000.0;
const DEFAULT_SAMPLE_RATE: f64 = 2_048_000.0;

/// The driver registry every binary gets: recording playback, developer-only synthetic radios,
/// and whichever hardware backends this build compiled in. Split
/// out of [`Engine::new`] so `sdrmm --doctor` reports the same set the server would open,
/// rather than forking the registration policy.
#[must_use]
pub fn builtin_registry(recordings_dir: Option<PathBuf>) -> DeviceRegistry {
    let mut registry = DeviceRegistry::new();
    let virtual_driver = VirtualDriver::for_build(recordings_dir);
    registry.register(VIRTUAL_PRIORITY, Box::new(virtual_driver));
    #[cfg(feature = "soapy")]
    registry.register(
        SOAPY_PRIORITY,
        Box::new(sdrmm_device_soapy::SoapyDriver::new()),
    );
    #[cfg(feature = "net-client")]
    {
        registry.register(
            NET_PRIORITY,
            Box::new(sdrmm_device_net::RtlTcpDriver::new()),
        );
        registry.register(
            NET_PRIORITY,
            Box::new(sdrmm_device_net::SpyServerDriver::new()),
        );
    }
    registry
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("device set {0} not found")]
    DeviceSetNotFound(u32),
    #[error("channel {0} not found in device set {1}")]
    ChannelNotFound(u32, u32),
    #[error("stream {stream} is out of range: this device has {streams} rx streams")]
    StreamOutOfRange { stream: u32, streams: u32 },
    #[error("device {0} is already open in device set {1}")]
    DeviceAlreadyOpen(String, u32),
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
    #[error("occupancy: {0}")]
    Occupancy(String),
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
                | Self::DeviceAlreadyOpen(..)
        )
    }
}

/// A hosted channel queued for a pipeline rebuild swap: the settings snapshot it was listed
/// under, plus the media identity (PCM and picture senders with their shared positions) the
/// replacement host must reuse, so a subscriber never notices the pipeline was replaced.
struct RebuildEntry {
    id: u32,
    /// The rx stream the channel taps; the replacement host must land on the same lane.
    stream: u32,
    settings: ChannelSettings,
    sinks: ChannelSinks,
}

/// [`CaptureRuntime::start`] refuses a device that reported no rate, so the fallback is
/// unreachable for a running set.
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
    // the Pi 4 is the budget floor.
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

/// A channel's media identity: the PCM fan-in, the Opus fan-out and the picture stream all
/// survive pipeline rebuilds (params type change, device rate change), so subscribers of either
/// kind never notice a swap.
struct ChannelMedia {
    /// What the DSP-side host writes into, handed to every host built for this channel.
    sinks: ChannelSinks,
    audio_tx: broadcast::Sender<AudioPacket>,
    encoder: Option<std::thread::JoinHandle<()>>,
    /// Latest transient station fix routed into this channel. It is deliberately absent from
    /// `ChannelInfo` (and therefore persistence), but every replacement DSP host inherits it.
    position: Option<PositionFix>,
}

impl ChannelMedia {
    /// `channels` is the layout the channel starts in; the encoder follows the PCM from there,
    /// so a later stereo toggle needs no new identity.
    fn new(channels: u8) -> Result<Self, EngineError> {
        let (pcm_tx, pcm_rx) = broadcast::channel(audio::PCM_CHANNEL_CAP);
        let (audio_tx, _) = broadcast::channel(audio::AUDIO_CHANNEL_CAP);
        let (video_tx, _) = broadcast::channel(video::VIDEO_CHANNEL_CAP);
        let (iq_tx, _) = broadcast::channel(iq::IQ_CHANNEL_CAP);
        let encoder = audio::spawn_encoder(channels, pcm_rx, audio_tx.clone())?;
        Ok(Self {
            sinks: ChannelSinks {
                pcm_tx,
                pcm_pos: Arc::new(AtomicU64::new(0)),
                video_tx,
                video_pos: Arc::new(AtomicU64::new(0)),
                iq_tx,
                level_db: Arc::new(AtomicU32::new(sdrmm_dsp::LEVEL_FLOOR_DB.to_bits())),
                peak_db: Arc::new(AtomicU32::new(sdrmm_dsp::LEVEL_FLOOR_DB.to_bits())),
            },
            audio_tx,
            encoder: Some(encoder),
            position: None,
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
    position: Option<recording::RecordingPosition>,
    writer: JoinHandle<()>,
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
    fn join(mut self) {
        drop(self.position.take());
        join_recording_writer(self.writer);
    }
}

struct DeviceSetState {
    info: DeviceInfo,
    capabilities: Capabilities,
    settings: DeviceSettings,
    status: DeviceSetStatus,
    channels: Vec<ChannelInfo>,
    /// Audio and video plumbing per channel id, kept beside the projected `channels` so
    /// rebuild swaps can preserve a channel's streams while replacing its DSP pipeline.
    media: HashMap<u32, ChannelMedia>,
    next_channel_id: u32,
    error: Option<String>,
    recording: Option<RecordingState>,
    /// Running frequency scan. The scan thread drives this set's centre frequency, so
    /// while it is present client retunes are refused rather than fought over.
    scanner: Option<ScannerState>,
    /// In-flight `patch_device` calls that will change the sample rate (pre-validated, device
    /// I/O or merge still pending). `start_recording`'s commit refuses while non-zero: a
    /// recording committed inside that window would pin a rate the patch is about to change —
    /// the reverse ordering of the recording guard in the patch pre-validation. Cleared by
    /// [`RatePatchGuard`] on every patch exit path.
    rate_patches: u32,
    cmd_txs: Vec<mpsc::Sender<DspCommand>>,
    /// Per-lane capture-ring drop counters shared with the runtime, index = stream;
    /// readable without its lock so snapshots never wait on a wedged device.
    overruns: Vec<Arc<AtomicU64>>,
    /// Overrun count already surfaced to clients; the hotplug tick diffs against it.
    overruns_seen: u64,
    /// Replay transport, on a set whose device is a recording; `None` for a radio. Held here
    /// rather than reached through the runtime for the same reason as `overruns`: a snapshot
    /// reads it on every emit and must never wait on a device lock, and a pause has to land
    /// while the capture thread is mid-block.
    playback: Option<Arc<PlaybackShared>>,
    runtime: Arc<Mutex<CaptureRuntime>>,
}

impl DeviceSetState {
    fn project(&self, id: u32) -> DeviceSet {
        let overruns = self.overruns_total();
        DeviceSet {
            id,
            device: self.info.identity(),
            capabilities: self.capabilities.clone(),
            settings: self.settings.clone(),
            status: self.status,
            channels: self.channels.clone(),
            overruns,
            error: self.error.clone(),
            recording: self.recording.as_ref().map(|r| r.status(overruns)),
            scanner: self.scanner.as_ref().map(ScannerState::status),
            playback: self.playback.as_deref().map(PlaybackShared::status),
        }
    }

    /// How many rx streams this set's runtime hosts — the bound every stream-taking call is
    /// checked against, and the count its refusal names.
    fn rx_streams(&self) -> u32 {
        self.cmd_txs.len() as u32
    }

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
    decoded_dropped: Arc<AtomicU64>,
    /// Stamped decoder records, fanned out to the WS hub and the decoder-log writer.
    decoded_tx_out: broadcast::Sender<DecodedRecord>,
    /// The trunk follower's inbox. Gated on the flag so an idle installation does not clone
    /// every decoded record.
    trunk_tx: mpsc::Sender<trunking::TrunkInput>,
    trunk_active: AtomicBool,
    trunk_status: Arc<Mutex<Vec<TrunkSystemStatus>>>,
    /// Band-occupancy statistics, shared across every set that is being collected from — the
    /// question they answer is about a frequency, not about which radio happened to hear it.
    occupancy: Mutex<occupancy::Occupancy>,
    /// The `(device set, stream)` lanes a collector thread is already running for.
    occupancy_sets: Mutex<HashSet<(u32, u32)>>,
    recordings_dir: Option<PathBuf>,
}

impl Engine {
    /// Build the engine with the built-in drivers registered (recording playback always,
    /// synthetic radios in debug builds, and native hardware backends). `recordings_dir` is both
    /// where `start_recording` writes and what the virtual driver scans for playback devices, so
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
        let (trunk_tx, trunk_rx) = mpsc::channel();
        let trunk_status = Arc::new(Mutex::new(Vec::new()));
        let engine = Arc::new(Self {
            registry,
            inner: Mutex::new(Inner::default()),
            event_tx,
            fault_tx,
            decoded_tx,
            decoded_dropped: Arc::new(AtomicU64::new(0)),
            decoded_tx_out,
            trunk_tx,
            trunk_active: AtomicBool::new(false),
            trunk_status: trunk_status.clone(),
            occupancy: Mutex::new(occupancy::Occupancy::new()),
            occupancy_sets: Mutex::new(HashSet::new()),
            recordings_dir,
        });
        engine.spawn_fault_drainer(fault_rx);
        engine.spawn_decoded_pump(decoded_rx);
        trunking::spawn(&engine, trunk_rx, trunk_status);
        engine
    }

    /// Which channels are trunk control channels. Only the patch knows, and it lives above the
    /// engine, so the answer is pushed down rather than polled for.
    pub fn configure_trunking(&self, systems: Vec<trunking::TrunkSystem>) {
        self.trunk_active
            .store(!systems.is_empty(), Ordering::Relaxed);
        if self
            .trunk_tx
            .send(trunking::TrunkInput::Configure(systems))
            .is_err()
        {
            tracing::error!("the trunk follower is gone: trunked systems will not be followed");
        }
    }

    fn spawn_decoded_pump(self: &Arc<Self>, decoded_rx: mpsc::Receiver<RawDecoded>) {
        let weak = Arc::downgrade(self);
        let spawned = std::thread::Builder::new()
            .name("sdrmm-decoded".to_string())
            .spawn(move || {
                let mut lost_seen = 0u64;
                while let Ok(raw) = decoded_rx.recv() {
                    let Some(engine) = weak.upgrade() else { return };
                    let at = format!("{:.9}", jiff::Timestamp::now());
                    let record = DecodedRecord {
                        device_set: raw.device_set,
                        channel: raw.channel,
                        at,
                        freq_hz: raw.freq_hz,
                        event: raw.event,
                    };
                    if engine.trunk_active.load(Ordering::Relaxed) {
                        let _ = engine
                            .trunk_tx
                            .send(trunking::TrunkInput::Record(Box::new(record.clone())));
                    }
                    let _ = engine.decoded_tx_out.send(record);
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

    #[must_use]
    pub fn subscribe_decoded(&self) -> broadcast::Receiver<DecodedRecord> {
        self.decoded_tx_out.subscribe()
    }

    /// Cumulative decoder frames dropped before reaching the pump (: surfaced loss).
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

    /// The directory `start_recording` writes into (: files on disk are the source
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
            tracing::error!("failed to spawn fault drainer: {e}");
        }
    }

    /// A capture thread died ( M1 hotplug robustness): keep the set listed but flag it
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

    /// Poll for attach/detach every `interval` ( M1): a changed probe result pushes
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

    /// Band-occupancy statistics gathered from the spectrum tap, shared across every reader.
    #[must_use]
    pub fn occupancy(&self) -> &Mutex<occupancy::Occupancy> {
        &self.occupancy
    }

    /// Start folding one device set's spectrum into the occupancy statistics.
    ///
    /// Runs off the same broadcast the waterfall is drawn from, so it costs no extra DSP; the
    /// thread ends when the stream does, when the engine is dropped, or when the set goes away.
    /// Subscribing twice to one lane is harmless — the second collector simply sees the same
    /// frames — but the caller is expected not to, and [`Engine::collect_occupancy_for`] is
    /// idempotent per set for exactly that reason.
    pub fn collect_occupancy_for(
        self: &Arc<Self>,
        ds: u32,
        stream: u32,
    ) -> Result<(), EngineError> {
        {
            let mut collecting = self
                .occupancy_sets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !collecting.insert((ds, stream)) {
                return Ok(());
            }
        }
        let mut rx = match self.subscribe_spectrum(ds, stream) {
            Ok(rx) => rx,
            Err(error) => {
                self.stop_collecting(ds, stream);
                return Err(error);
            }
        };
        let weak = Arc::downgrade(self);
        std::thread::Builder::new()
            .name(format!("sdrmm-occupancy-{ds}-{stream}"))
            .spawn(move || {
                loop {
                    let snapshot = match rx.blocking_recv() {
                        Ok(snapshot) => snapshot,
                        // A lagged reader is expected and fine: occupancy is a statistic over
                        // many frames, and missing some of them changes nothing it claims.
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    };
                    let Some(engine) = weak.upgrade() else { return };
                    let now_ms = jiff::Timestamp::now().as_millisecond();
                    engine
                        .occupancy
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .observe(&snapshot.db, snapshot.center_hz, snapshot.span_hz, now_ms);
                }
                if let Some(engine) = weak.upgrade() {
                    engine.stop_collecting(ds, stream);
                }
            })
            .map_err(|error| {
                self.stop_collecting(ds, stream);
                EngineError::Occupancy(error.to_string())
            })?;
        Ok(())
    }

    /// Keep an occupancy collector running on every lane of every running set, checking every
    /// `interval`. Opt-in per binary, like the hotplug prober.
    ///
    /// A poll rather than a hook on set creation: sets come and go, a replug rebuilds a runtime
    /// under a set that already existed, and a collector whose stream closed simply ends. One
    /// idempotent sweep covers all three without a lifecycle to keep in step.
    pub fn start_occupancy_collector(self: &Arc<Self>, interval: Duration) -> std::io::Result<()> {
        let weak = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("sdrmm-occupancy".to_string())
            .spawn(move || {
                loop {
                    let Some(engine) = weak.upgrade() else { return };
                    for (ds, streams) in engine.running_lanes() {
                        for stream in 0..streams {
                            // A set that cannot be subscribed to right now is one the next sweep
                            // will find again; there is nothing here worth failing over.
                            let _ = engine.collect_occupancy_for(ds, stream);
                        }
                    }
                    drop(engine);
                    std::thread::sleep(interval);
                }
            })?;
        Ok(())
    }

    /// Running device sets and how many receive streams each has.
    fn running_lanes(&self) -> Vec<(u32, u32)> {
        let inner = self.lock();
        inner
            .device_sets
            .iter()
            .filter(|(_, state)| state.status == DeviceSetStatus::Running)
            .map(|(id, state)| (*id, state.rx_streams()))
            .collect()
    }

    fn stop_collecting(&self, ds: u32, stream: u32) {
        self.occupancy_sets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&(ds, stream));
    }

    /// Push every set's channel levels every `interval` ( signal metering). Opt-in per
    /// binary, like the hotplug prober, and holding a `Weak` for the same reason: the thread
    /// exits at the first wake after the engine is dropped.
    ///
    /// Nothing accumulates here. A tick that finds no channels sends nothing, and a client that
    /// misses a tick simply draws the next — these are measurements, not state.
    pub fn start_level_meter(self: &Arc<Self>, interval: Duration) -> std::io::Result<()> {
        let weak = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("sdrmm-levels".to_string())
            .spawn(move || {
                loop {
                    let Some(engine) = weak.upgrade() else { return };
                    engine.level_tick();
                    drop(engine);
                    std::thread::sleep(interval);
                }
            })?;
        Ok(())
    }

    /// One metering step, split from the thread so tests drive it without sleeping.
    pub fn level_tick(&self) {
        for ds in self.device_sets_with_channels() {
            let levels = self.channel_levels(ds);
            if !levels.is_empty() {
                self.emit(ServerEvent::ChannelLevels {
                    device_set: ds,
                    levels,
                });
            }
        }
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
    /// its channels ( M5: auto-reconnect on replug). Driven by the hotplug tick, so an
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
        let playback = device.playback();
        let mut settings = stored_settings.clone();
        settings.merge_from(&device.settings().clone());
        let rate = sample_rate_of(&settings);
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
            state.playback = playback;
            let rebuilds: Vec<RebuildEntry> = state
                .channels
                .iter()
                .filter_map(|c| {
                    state.media.get(&c.id).map(|m| RebuildEntry {
                        id: c.id,
                        stream: c.stream,
                        settings: c.settings.clone(),
                        sinks: m.sinks.clone(),
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

        let mut dead: Vec<ChannelMedia> = Vec::new();
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

    /// Hand a `driver:key` to whichever backend can address it without opening anything, so a
    /// later probe reports it.
    ///
    /// Only a network receiver answers: it is named rather than discovered, and nothing else would
    /// put a stored endpoint back into the probe list after a restart — which is what a device node
    /// bound to one waits for. Every backend that enumerates real hardware returns `None`, where a
    /// key no probe found means the device is not attached.
    #[must_use]
    pub fn adopt_device(&self, device_id: &str) -> Option<DeviceInfo> {
        self.registry.resolve(device_id)
    }

    /// Discovered devices across all drivers ( `GET /api/devices`).
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

    /// Full authoritative snapshot ( `GET /api/state`).
    #[must_use]
    pub fn snapshot(&self) -> StateSnapshot {
        // Before the engine lock: the follower publishes without holding it, and this order is
        // what keeps the two from meeting head-on.
        let trunk_systems = self.trunk_systems();
        let inner = self.lock();
        StateSnapshot {
            device_sets: inner
                .device_sets
                .iter()
                .map(|(id, s)| s.project(*id))
                .collect(),
            trunk_systems,
            revision: inner.revision,
        }
    }

    #[must_use]
    pub fn trunk_systems(&self) -> Vec<TrunkSystemStatus> {
        self.trunk_status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Publish a server-owned event on the engine's bus: the hub is the one fan-out.
    pub fn emit_event(&self, event: ServerEvent) {
        self.emit(event);
    }

    /// Open a device into a new device set and start streaming ( POST devicesets).
    ///
    /// # Errors
    /// [`EngineError::DeviceAlreadyOpen`] when a set already holds this radio. One receiver is one
    /// set: a second open would either be refused by the driver holding the handle or — for a
    /// backend that allows it, a replayed file or a network endpoint — hand out two faces and two
    /// sets of settings for one stream, only one of which the radio is actually running.
    pub fn create_device_set(&self, device_id: &str) -> Result<u32, EngineError> {
        self.refuse_reopen(device_id)?;
        let (info, device) = self.registry.open(device_id)?;
        // The key a driver adopts is not always the one that was asked for — a network endpoint is
        // canonicalized — so the identity the open reports is checked as well, before the device
        // gets a capture thread.
        if let Err(already) = self.refuse_reopen(&info.id()) {
            drop(device);
            return Err(already);
        }
        let capabilities = device.capabilities().clone();
        let settings = device.settings().clone();
        // Taken before the device moves into its runtime — afterwards it is behind the capture
        // lock, which the snapshot path must never wait on.
        let playback = device.playback();

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
                    media: HashMap::new(),
                    next_channel_id: 1,
                    error: pending.as_ref().map(ToString::to_string),
                    recording: None,
                    scanner: None,
                    rate_patches: 0,
                    cmd_txs,
                    overruns,
                    overruns_seen: 0,
                    playback,
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

    fn refuse_reopen(&self, device_id: &str) -> Result<(), EngineError> {
        let inner = self.lock();
        match inner
            .device_sets
            .iter()
            .find(|(_, set)| set.info.id() == device_id)
        {
            Some((id, _)) => Err(EngineError::DeviceAlreadyOpen(device_id.to_owned(), *id)),
            None => Ok(()),
        }
    }

    /// Close a device set and stop its threads ( DELETE devicesets).
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

    /// Apply a device settings delta ( PATCH device). The device I/O runs under the
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
                        state.media.get(&c.id).map(|m| RebuildEntry {
                            id: c.id,
                            stream: c.stream,
                            settings: c.settings.clone(),
                            sinks: m.sinks.clone(),
                        })
                    })
                    .collect()
            };
            let settings = state.settings.clone();
            inner.revision += 1;
            (settings, rate, rebuilds)
        };
        lock_runtime(&runtime).set_meta(&settings);
        let mut dead: Vec<ChannelMedia> = Vec::new();
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
        dead: &mut Vec<ChannelMedia>,
    ) {
        let RebuildEntry {
            id,
            stream,
            mut settings,
            sinks,
        } = rebuild;
        let mut built_rate = rate;
        loop {
            let built = descriptor_for(&settings.params)
                .and_then(|d| validate_channel(&d, &settings, built_rate))
                .and_then(|()| {
                    ChannelHost::build(
                        built_rate,
                        &settings,
                        sinks.clone(),
                        self.decoded_sink(ds, id),
                    )
                    .map_err(EngineError::from)
                });
            let mut inner = self.lock();
            let Some(state) = inner.device_sets.get_mut(&ds) else {
                return;
            };
            let current_rate = sample_rate_of(&state.settings);
            let Some(info) = state.channels.iter().find(|c| c.id == id) else {
                return;
            };
            if current_rate != built_rate || info.settings != settings {
                settings = info.settings.clone();
                built_rate = current_rate;
                continue;
            }
            match built {
                Ok(mut host) => {
                    if let Some(media) = state.media.get(&id) {
                        host.position_changed(media.position.as_ref());
                    }
                    state.send_dsp(stream, DspCommand::RemoveChannel { id });
                    state.send_dsp(stream, DspCommand::AddChannel { id, host });
                }
                Err(e) => {
                    // The rate was pre-validated against every channel before the device
                    // I/O, so only a racing settings change lands here; drop the channel
                    // rather than leave a stale-rate pipeline running.
                    tracing::error!(ds, channel = id, error = %e, "channel rebuild failed after rate change; removing channel");
                    state.channels.retain(|c| c.id != id);
                    dead.extend(state.media.remove(&id));
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

    /// Add a channel to a device set ( POST channels), tapping the rx stream
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
        let created = ChannelMedia::new(sdrmm_channels::audio_channels(&settings.params))?;
        let sinks = created.sinks.clone();
        let mut media = Some(created);

        // A rate patch racing between build and insert would leave a wrong-rate DDC, so the
        // rate is re-checked under the lock and the pipeline rebuilt if it moved. The
        // AddChannel goes out in the same critical section as the state commit, so no
        // concurrent removal or swap can interleave between them.
        let staged = loop {
            let built = validate_channel(&descriptor, &settings, device_rate).and_then(|()| {
                ChannelHost::build(
                    device_rate,
                    &settings,
                    sinks.clone(),
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
            if let Some(handle) = media.take() {
                state.media.insert(id, handle);
            }
            state.send_dsp(stream, DspCommand::AddChannel { id, host });
            inner.revision += 1;
            break Ok(id);
        };
        let id = match staged {
            Ok(id) => id,
            Err(e) => {
                // The local sender clones must go first or the encoder join would wait on them.
                drop(sinks);
                if let Some(handle) = media.take() {
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

    /// Apply a channel settings delta ( PATCH channels). Retunes and param tweaks
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
        let (old, sinks, mut device_rate) = {
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
                .media
                .get(&ch)
                .ok_or(EngineError::ChannelNotFound(ch, ds))?;
            (
                info.settings.clone(),
                handle.sinks.clone(),
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
                    sinks.clone(),
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
            let stream = info.stream;
            let prev = std::mem::replace(&mut info.settings, settings.clone());
            match host {
                Some(mut host) => {
                    if let Some(media) = state.media.get(&ch) {
                        host.position_changed(media.position.as_ref());
                    }
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
                        // ADS-B settings carry a persisted fallback reference. Its `apply`
                        // updates that reference, so immediately restore the live wire value
                        // (including `None`) after any same-type settings patch.
                        let fix = state
                            .media
                            .get(&ch)
                            .and_then(|media| media.position.clone());
                        state.send_dsp(stream, DspCommand::PositionChanged { id: ch, fix });
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

    /// Remove a channel from a device set ( DELETE channels), tearing down its DSP
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
            let handle = state.media.remove(&ch);
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
    /// recordings dir (; the path is lossless — see [`recording`]). One recording per
    /// set, on the named stream (b); the SigMF meta records which. Writer, files,
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
            let (tap, position, messages, shared) = recording::create_tap();
            let writer = recording::spawn_writer(sigmf, messages, shared.clone())?;

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
                            position: Some(position.clone()),
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
            drop(tap);
            drop(position);
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
            mut position,
            writer,
            overruns_at_start,
            ..
        } = recording;
        drop(position.take());
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

    /// Subscribe to a channel's Opus packet stream ( SubscribeAudio).
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
            .media
            .get(&ch)
            .ok_or(EngineError::ChannelNotFound(ch, ds))?;
        Ok(handle.audio_tx.subscribe())
    }

    pub fn subscribe_pcm(
        &self,
        ds: u32,
        ch: u32,
    ) -> Result<broadcast::Receiver<PcmBlock>, EngineError> {
        let inner = self.lock();
        let state = inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let handle = state
            .media
            .get(&ch)
            .ok_or(EngineError::ChannelNotFound(ch, ds))?;
        Ok(handle.sinks.pcm_tx.subscribe())
    }

    /// Subscribe to a channel's picture stream ( SubscribeVideo). A channel whose type
    /// scans out nothing is refused rather than handed a stream that would stay silent: a panel
    /// waiting forever on a mode that has no video looks exactly like a broken receiver.
    pub fn subscribe_video(
        &self,
        ds: u32,
        ch: u32,
    ) -> Result<broadcast::Receiver<VideoPacket>, EngineError> {
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
        let descriptor = descriptor_for(&info.settings.params)?;
        if !descriptor.has_video {
            return Err(ChannelError::InvalidSettings(format!(
                "{} produces no video",
                descriptor.name
            ))
            .into());
        }
        let handle = state
            .media
            .get(&ch)
            .ok_or(EngineError::ChannelNotFound(ch, ds))?;
        Ok(handle.sinks.video_tx.subscribe())
    }

    /// Every channel's current signal level on one device set, newest reading of each.
    ///
    /// Reads two atomics per channel behind the state lock and touches no DSP thread, so a poller
    /// can call it as often as it likes. Levels are deliberately *not* part of
    /// [`sdrmm_wire::StateSnapshot`]: they change continuously, and a `StateChanged` per reading
    /// would have every client refetch the whole world ten times a second.
    #[must_use]
    pub fn channel_levels(&self, ds: u32) -> Vec<ChannelLevel> {
        let inner = self.lock();
        let Some(state) = inner.device_sets.get(&ds) else {
            return Vec::new();
        };
        state
            .channels
            .iter()
            .filter_map(|channel| {
                let media = state.media.get(&channel.id)?;
                Some(ChannelLevel {
                    channel: channel.id,
                    level_db: f32::from_bits(media.sinks.level_db.load(Ordering::Relaxed)),
                    peak_db: f32::from_bits(media.sinks.peak_db.load(Ordering::Relaxed)),
                })
            })
            .collect()
    }

    /// Device sets that currently host at least one channel — what a level poller iterates,
    /// rather than reaching into the state map itself.
    #[must_use]
    pub fn device_sets_with_channels(&self) -> Vec<u32> {
        let inner = self.lock();
        inner
            .device_sets
            .iter()
            .filter(|(_, state)| !state.channels.is_empty())
            .map(|(id, _)| *id)
            .collect()
    }

    /// Subscribe to a channel's baseband tap ( SubscribeIq).
    ///
    /// Unlike video there is nothing to refuse: every channel has a passband, whatever it
    /// demodulates from it, and the tap runs above the squelch so a quiet channel still carries
    /// one. The engine starts sending only once this receiver exists — the DSP-side tap checks
    /// the subscriber count and does nothing at all until then.
    pub fn subscribe_iq(
        &self,
        ds: u32,
        ch: u32,
    ) -> Result<broadcast::Receiver<IqBlock>, EngineError> {
        let inner = self.lock();
        let state = inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let handle = state
            .media
            .get(&ch)
            .ok_or(EngineError::ChannelNotFound(ch, ds))?;
        Ok(handle.sinks.iq_tx.subscribe())
    }

    /// Every compiled-in channel type ( GET channel types). The registry lives in
    /// `sdrmm-channels`; the server reaches it through here and never depends on it directly.
    #[must_use]
    pub fn channel_types(&self) -> Vec<ChannelDescriptor> {
        sdrmm_channels::descriptors()
    }

    /// Start a frequency scan on a device set. The scan owns the set's
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

    /// Drive a replaying set's transport (play, pause, stop, seek) and return the state it
    /// leaves behind.
    ///
    /// The whole call is atomic stores on a shared handle, so unlike `apply` it never touches
    /// the device or its lock: a pause lands while the capture thread is mid-block, and it
    /// cannot be stalled by a wedged device.
    pub fn control_playback(
        &self,
        ds: u32,
        request: &PlaybackRequest,
    ) -> Result<PlaybackStatus, EngineError> {
        let status = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let playback = state.playback.as_deref().ok_or_else(|| {
                EngineError::Device(DeviceError::Unsupported(
                    "this device is a radio, not a recording: there is nothing to seek in a \
                     signal that is still arriving"
                        .to_string(),
                ))
            })?;
            playback.control(request);
            let status = playback.status();
            inner.revision += 1;
            status
        };
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
    for (_, handle) in removed.media.drain() {
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
mod tests;
