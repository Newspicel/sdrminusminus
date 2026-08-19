use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use sdrmm_channels::ChannelError;
use sdrmm_device::{DeviceError, DeviceRegistry, PlaybackShared};
use sdrmm_device_virtual::VirtualDriver;
use sdrmm_recorder::{data_path, meta_path};
use sdrmm_wire::{
    AudioRecordingStatus, Capabilities, ChannelInfo, ChannelSettings, DecodedRecord, DeviceFault,
    DeviceInfo, DeviceSet, DeviceSetStatus, DeviceSettings, NetworkExportSettings,
    NetworkExportStatus, PositionFix, RecordingStatus, ServerEvent, StateScope, StateSnapshot,
    TrunkSystemStatus,
};
use tokio::sync::broadcast;

pub mod audio;
pub mod audio_recording;
mod capture_ops;
mod channel_ops;
pub mod coherent;
mod coherent_ops;
mod device_ops;
mod discovery;
mod history;
mod hotplug;
mod hunt;
pub mod image;
pub mod iq;
mod network_export;
pub mod occupancy;
mod planning;
mod position;
pub mod recording;
pub mod runtime;
pub mod scanner;
mod sinks;
mod spectrum;
mod streams;
pub mod symbols;
mod time_machine;
pub mod trunking;
pub mod video;
pub use audio::{AudioPacket, PcmBlock, PcmPayload};
pub use image::ImageCapture;
pub use iq::{IQ_BLOCK_SAMPLES, IQ_BLOCKS_PER_SEC, IqBlock};
pub use planning::FrontEndPlan;
pub(crate) use planning::{channel_input_rate, descriptor_for, plan_front_end};
pub use recording::FinalizedRecording;
pub use runtime::SpectrumSnapshot;
pub use sdrmm_device_array::ArrayCatalog;
pub use symbols::{SYMBOL_BLOCKS_PER_SEC, SymbolBlock};
pub use trunking::TrunkSystem;
pub use video::{VideoPacket, VideoPicture};

use crate::{
    audio_recording::AudioRecordingShared,
    history::TimeMachineState,
    hunt::HuntState,
    network_export::{NetworkExportShared, NetworkExportTap},
    recording::RecordingShared,
    runtime::{CaptureRuntime, ChannelSinks, DecodedSink, DspCommand, RawDecoded, RawImage},
    scanner::{ScannerState, session::SessionState},
    sinks::ChannelBasebandRecording,
};

const VIRTUAL_PRIORITY: u8 = 10;
#[cfg(feature = "soapy")]
const SOAPY_PRIORITY: u8 = 20;
// Above Soapy so a host-installed SoapySDRPlay3 loses the dedup for a receiver this driver
// already speaks to directly.
#[cfg(feature = "sdrplay")]
const SDRPLAY_PRIORITY: u8 = 25;
// The USB backends speak to their radios directly and are hidden from Soapy's enumeration, so
// this rank only settles a tie against a driver that reports the same serial by another route.
#[cfg(any(feature = "rtlsdr", feature = "hackrf"))]
const NATIVE_PRIORITY: u8 = 25;
#[cfg(feature = "net-client")]
const NET_PRIORITY: u8 = 30;

/// A composite the operator described by hand beats anything discovered, because it is the only
/// thing that knows those radios belong together.
const ARRAY_PRIORITY: u8 = 40;
const EVENT_CHANNEL_CAP: usize = 256;
const DECODED_QUEUE_CAP: usize = 4096;
const DECODED_CHANNEL_CAP: usize = 1024;
const DEFAULT_CENTER_HZ: f64 = 100_000_000.0;
const DEFAULT_SAMPLE_RATE: f64 = 2_048_000.0;
const TIME_MACHINE_STOP_POLL: Duration = Duration::from_millis(10);
const TIME_MACHINE_STOP_POLLS: u32 = 200;

/// The SoapySDR driver names this build speaks to over its own USB stack, and therefore hides
/// from Soapy's enumeration so one radio is never listed twice.
#[must_use]
pub fn soapy_handled_natively() -> Vec<&'static str> {
    [
        #[cfg(feature = "rtlsdr")]
        "rtlsdr",
        #[cfg(feature = "hackrf")]
        "hackrf",
    ]
    .to_vec()
}

#[must_use]
pub fn builtin_registry(recordings_dir: Option<PathBuf>) -> DeviceRegistry {
    builtin_registry_accelerated(recordings_dir, 1.0)
}

#[must_use]
pub fn builtin_registry_accelerated(
    recordings_dir: Option<PathBuf>,
    playback_speed: f64,
) -> DeviceRegistry {
    builtin_registry_with(recordings_dir, playback_speed, &ArrayCatalog::new())
}

/// The built-in drivers plus a composite driver over them, so a bank of radios the operator has
/// wired together opens like any other multi-lane radio.
#[must_use]
pub fn builtin_registry_with(
    recordings_dir: Option<PathBuf>,
    playback_speed: f64,
    arrays: &ArrayCatalog,
) -> DeviceRegistry {
    let mut registry = single_registry(recordings_dir.clone(), playback_speed);
    registry.register(
        ARRAY_PRIORITY,
        Box::new(sdrmm_device_array::ArrayDriver::new(
            arrays.clone(),
            std::sync::Arc::new(single_registry(recordings_dir, playback_speed)),
        )),
    );
    registry
}

fn single_registry(recordings_dir: Option<PathBuf>, playback_speed: f64) -> DeviceRegistry {
    let mut registry = DeviceRegistry::new();
    let virtual_driver = VirtualDriver::for_build_accelerated(recordings_dir, playback_speed);
    registry.register(VIRTUAL_PRIORITY, Box::new(virtual_driver));
    #[cfg(feature = "soapy")]
    registry.register(
        SOAPY_PRIORITY,
        Box::new(sdrmm_device_soapy::SoapyDriver::excluding(
            soapy_handled_natively(),
        )),
    );
    #[cfg(feature = "sdrplay")]
    registry.register(
        SDRPLAY_PRIORITY,
        Box::new(sdrmm_device_sdrplay::SdrplayDriver::new()),
    );
    #[cfg(feature = "rtlsdr")]
    registry.register(
        NATIVE_PRIORITY,
        Box::new(sdrmm_device_rtlsdr::RtlSdrDriver::new()),
    );
    #[cfg(feature = "hackrf")]
    registry.register(
        NATIVE_PRIORITY,
        Box::new(sdrmm_device_hackrf::HackRfDriver::new()),
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
    #[error("recording: {0}")]
    RecordingIo(String),
    #[error("network export: {0}")]
    NetworkExport(String),
    #[error("scan: {0}")]
    Scan(String),
    #[error("occupancy: {0}")]
    Occupancy(String),
    #[error("coherent: {0}")]
    Coherent(String),
}

impl EngineError {
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::DeviceSetNotFound(_) | Self::ChannelNotFound(..))
            || matches!(self, Self::Device(DeviceError::NotFound(_)))
    }

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
                | Self::NetworkExport(_)
                | Self::Scan(_)
                | Self::Coherent(_)
                | Self::StreamOutOfRange { .. }
        )
    }

    #[must_use]
    pub fn is_conflict(&self) -> bool {
        matches!(
            self,
            Self::DeviceAlreadyOpen(..) | Self::Device(DeviceError::InUse(_))
        )
    }
}

struct RebuildEntry {
    id: u32,
    stream: u32,
    settings: ChannelSettings,
    sinks: ChannelSinks,
}

fn sample_rate_of(settings: &DeviceSettings) -> f64 {
    settings.sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE)
}

fn ids_of(devices: &[DeviceInfo]) -> Vec<String> {
    devices.iter().map(DeviceInfo::id).collect()
}

/// Sorts a fault into what a reader can do about it, leaving the message itself for the details.
fn fault_kind(error: &DeviceError) -> DeviceFault {
    match error {
        DeviceError::Disconnected(_) => DeviceFault::Unplugged,
        DeviceError::InUse(_) => DeviceFault::InUse,
        _ => DeviceFault::Other,
    }
}

fn check_export_request(node: &str, settings: &NetworkExportSettings) -> Result<(), EngineError> {
    if node.is_empty() || node.len() > sdrmm_wire::patch::MAX_NODE_ID_LEN {
        return Err(EngineError::NetworkExport(
            "node id is empty or too long".to_owned(),
        ));
    }
    if settings.address.is_empty() || settings.address.len() > sdrmm_wire::MAX_NETWORK_ADDRESS_LEN {
        return Err(EngineError::NetworkExport(
            "destination address is empty or too long".to_owned(),
        ));
    }
    Ok(())
}

fn remove_recording_files(stem: &Path) {
    for path in [meta_path(stem), data_path(stem)] {
        if let Err(e) = std::fs::remove_file(&path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %path.display(), error = %e, "aborted recording attempt left a file behind");
        }
    }
}

struct ChannelMedia {
    sinks: ChannelSinks,
    audio_tx: broadcast::Sender<AudioPacket>,
    encoder: Option<std::thread::JoinHandle<()>>,
    position: Option<PositionFix>,
}

impl ChannelMedia {
    fn new(channels: u8) -> Result<Self, EngineError> {
        let (pcm_tx, pcm_rx) = broadcast::channel(audio::PCM_CHANNEL_CAP);
        let (audio_tx, _) = broadcast::channel(audio::AUDIO_CHANNEL_CAP);
        let (video_tx, _) = broadcast::channel(video::VIDEO_CHANNEL_CAP);
        let (iq_tx, _) = broadcast::channel(iq::IQ_CHANNEL_CAP);
        let (symbol_tx, _) = broadcast::channel(symbols::SYMBOL_CHANNEL_CAP);
        let encoder = audio::spawn_encoder(channels, pcm_rx, audio_tx.clone())?;
        Ok(Self {
            sinks: ChannelSinks {
                pcm_tx,
                pcm_pos: Arc::new(AtomicU64::new(0)),
                video_tx,
                video_pos: Arc::new(AtomicU64::new(0)),
                iq_tx,
                symbol_tx,
                level_db: Arc::new(AtomicU32::new(sdrmm_dsp::LEVEL_FLOOR_DB.to_bits())),
                peak_db: Arc::new(AtomicU32::new(sdrmm_dsp::LEVEL_FLOOR_DB.to_bits())),
                squelch_db: Arc::new(AtomicU32::new(f32::NAN.to_bits())),
            },
            audio_tx,
            encoder: Some(encoder),
            position: None,
        })
    }

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
    file: String,
    stream: u32,
    started_at: String,
    stem: PathBuf,
    shared: Arc<RecordingShared>,
    position: Option<recording::RecordingPosition>,
    writer: JoinHandle<()>,
    overruns_at_start: u64,
    samples_seen: u64,
    error_seen: bool,
}

struct ChannelAudioRecording {
    file: String,
    stream: u32,
    started_at: String,
    channels: u8,
    tap: audio_recording::AudioRecorderTap,
    shared: Arc<AudioRecordingShared>,
    writer: JoinHandle<()>,
    frames_seen: u64,
    error_seen: bool,
}

impl ChannelAudioRecording {
    fn status(&self) -> AudioRecordingStatus {
        AudioRecordingStatus {
            file: self.file.clone(),
            started_at: self.started_at.clone(),
            channels: self.channels,
            frames: self.shared.frames(),
            bytes: self.shared.bytes(),
            error: self.shared.error(),
        }
    }

    fn join(self) {
        let Self { tap, writer, .. } = self;
        drop(tap);
        if writer.join().is_err() {
            tracing::error!("audio recording writer thread panicked");
        }
    }
}

struct NetworkExportState {
    node: String,
    stream: u32,
    settings: NetworkExportSettings,
    sample_rate: u64,
    center_hz: i64,
    shared: Arc<NetworkExportShared>,
    writer: Option<JoinHandle<()>>,
    overruns_at_start: u64,
    samples_seen: u64,
    error_seen: bool,
}

enum NetworkExportCommit {
    Started(NetworkExportStatus),
    Aborted {
        tap: NetworkExportTap,
        writer: JoinHandle<()>,
        patch_in_flight: bool,
    },
}

impl NetworkExportState {
    fn status(&self, overruns_now: u64) -> NetworkExportStatus {
        NetworkExportStatus {
            node: self.node.clone(),
            stream: self.stream,
            settings: self.settings.clone(),
            sample_rate: self.sample_rate,
            center_hz: self.center_hz,
            samples: self.shared.samples(),
            bytes: self.shared.bytes(),
            packets: self.shared.packets(),
            overruns: overruns_now - self.overruns_at_start,
            error: self.shared.error(),
        }
    }

    fn join(&mut self) {
        if let Some(writer) = self.writer.take() {
            join_network_writer(writer);
        }
    }
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

    fn join(mut self) {
        drop(self.position.take());
        join_recording_writer(self.writer);
    }
}

struct DeviceSetState {
    info: DeviceInfo,
    capabilities: Capabilities,
    settings: DeviceSettings,
    /// Where the front end's DC artifact is parked and whether it is being removed there. The
    /// displacement is mixed back out downstream, so nothing the operator sees moves with it.
    front_end: FrontEndPlan,
    status: DeviceSetStatus,
    channels: Vec<ChannelInfo>,
    media: HashMap<u32, ChannelMedia>,
    next_channel_id: u32,
    error: Option<String>,
    fault: Option<DeviceFault>,
    recording: Option<RecordingState>,
    audio_recordings: HashMap<u32, ChannelAudioRecording>,
    baseband_recordings: HashMap<u32, ChannelBasebandRecording>,
    channel_exports: HashMap<u32, NetworkExportState>,
    network_export: Option<NetworkExportState>,
    time_machine: Option<TimeMachineState>,
    scanner: Option<ScannerState>,
    hunt: Option<HuntState>,
    rate_patches: u32,
    cmd_txs: Vec<mpsc::Sender<DspCommand>>,
    overruns: Vec<Arc<AtomicU64>>,
    overruns_seen: u64,
    stalls: Vec<Arc<AtomicU64>>,
    playback: Option<Arc<PlaybackShared>>,
    coherent: Option<crate::coherent_ops::CoherentState>,
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
            lo_offset_in_force_hz: self.front_end.lo_offset_hz,
            channels: self
                .channels
                .iter()
                .map(|channel| ChannelInfo {
                    audio_recording: self
                        .audio_recordings
                        .get(&channel.id)
                        .map(ChannelAudioRecording::status),
                    baseband_recording: self
                        .baseband_recordings
                        .get(&channel.id)
                        .map(|recording| recording.status(overruns)),
                    network_export: self
                        .channel_exports
                        .get(&channel.id)
                        .map(|export| export.status(overruns)),
                    ..channel.clone()
                })
                .collect(),
            overruns,
            error: self.error.clone(),
            fault: self.fault,
            recording: self.recording.as_ref().map(|r| r.status(overruns)),
            network_export: self
                .network_export
                .as_ref()
                .map(|export| export.status(overruns)),
            time_machine: self
                .time_machine
                .as_ref()
                .map(|history| history.status(overruns)),
            scanner: self.scanner.as_ref().map(ScannerState::status),
            hunt: self.hunt.as_ref().map(HuntState::status),
            playback: self.playback.as_deref().map(PlaybackShared::status),
        }
    }

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

    fn overruns_total(&self) -> u64 {
        self.overruns
            .iter()
            .map(|counter| counter.load(Ordering::Relaxed))
            .sum()
    }

    /// Longest gap any lane went without touching its capture ring since the last read, and
    /// clears it so the next report covers the next window.
    fn take_worst_stall_ms(&self) -> u64 {
        self.stalls
            .iter()
            .map(|counter| counter.swap(0, Ordering::Relaxed))
            .max()
            .unwrap_or(0)
            / 1_000
    }

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

    fn rearm_audio_recording(&self, ch: u32, stream: u32) {
        if let Some(recording) = self.audio_recordings.get(&ch) {
            self.send_dsp(
                stream,
                DspCommand::StartChannelRecording {
                    id: ch,
                    tap: recording.tap.clone(),
                },
            );
        }
    }
}

enum FaultGate {
    Pending(Option<DeviceError>),
    Armed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PatchOrigin {
    Client,
    Scan,
}

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
    scan_session: Option<SessionState>,
    creating: HashSet<u32>,
    pending_faults: HashMap<u32, DeviceError>,
    next_ds_id: u32,
    revision: u64,
}

impl Inner {
    fn leave_scan_session(&mut self, ds: u32) {
        let Some(session) = self.scan_session.as_mut() else {
            return;
        };
        session.device_sets.retain(|&id| id != ds);
        if session.device_sets.is_empty() {
            self.scan_session = None;
        }
    }
}

pub struct Engine {
    registry: DeviceRegistry,
    arrays: ArrayCatalog,
    inner: Mutex<Inner>,
    event_tx: broadcast::Sender<ServerEvent>,
    fault_tx: mpsc::Sender<(u32, DeviceError)>,
    decoded_tx: mpsc::SyncSender<RawDecoded>,
    image_queue_tx: mpsc::SyncSender<RawImage>,
    decoded_dropped: Arc<AtomicU64>,
    decoded_tx_out: broadcast::Sender<DecodedRecord>,
    image_tx: broadcast::Sender<ImageCapture>,
    trunk_tx: mpsc::Sender<trunking::TrunkInput>,
    trunk_active: AtomicBool,
    trunk_status: Arc<Mutex<Vec<TrunkSystemStatus>>>,
    occupancy: Mutex<occupancy::Occupancy>,
    occupancy_sets: Mutex<HashSet<(u32, u32)>>,
    discovery: Mutex<discovery::Discovery>,
    recordings_dir: Option<PathBuf>,
}

impl Engine {
    #[must_use]
    pub fn new(recordings_dir: Option<PathBuf>) -> Arc<Self> {
        let arrays = ArrayCatalog::new();
        let registry = builtin_registry_with(recordings_dir.clone(), 1.0, &arrays);
        Self::with_arrays(registry, recordings_dir, arrays)
    }

    #[must_use]
    pub fn with_registry(registry: DeviceRegistry, recordings_dir: Option<PathBuf>) -> Arc<Self> {
        Self::with_arrays(registry, recordings_dir, ArrayCatalog::new())
    }

    /// The arrays this engine's composite driver opens. Written by whoever the operator edits
    /// them through, read by the driver on every probe.
    #[must_use]
    pub fn arrays(&self) -> &ArrayCatalog {
        &self.arrays
    }

    #[must_use]
    pub fn with_arrays(
        registry: DeviceRegistry,
        recordings_dir: Option<PathBuf>,
        arrays: ArrayCatalog,
    ) -> Arc<Self> {
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAP);
        let (fault_tx, fault_rx) = mpsc::channel();
        let (decoded_tx, decoded_rx) = mpsc::sync_channel(DECODED_QUEUE_CAP);
        let (image_queue_tx, image_queue_rx) = mpsc::sync_channel(image::IMAGE_QUEUE_CAP);
        let (decoded_tx_out, _) = broadcast::channel(DECODED_CHANNEL_CAP);
        let (image_tx, _) = broadcast::channel(image::IMAGE_CHANNEL_CAP);
        let (trunk_tx, trunk_rx) = mpsc::channel();
        let trunk_status = Arc::new(Mutex::new(Vec::new()));
        let engine = Arc::new(Self {
            registry,
            arrays,
            inner: Mutex::new(Inner::default()),
            event_tx,
            fault_tx,
            decoded_tx,
            image_queue_tx,
            decoded_dropped: Arc::new(AtomicU64::new(0)),
            decoded_tx_out,
            image_tx,
            trunk_tx,
            trunk_active: AtomicBool::new(false),
            trunk_status: trunk_status.clone(),
            occupancy: Mutex::new(occupancy::Occupancy::new()),
            occupancy_sets: Mutex::new(HashSet::new()),
            discovery: Mutex::new(discovery::Discovery::default()),
            recordings_dir,
        });
        engine.spawn_fault_drainer(fault_rx);
        engine.spawn_decoded_pump(decoded_rx);
        engine.spawn_image_pump(image_queue_rx);
        trunking::spawn(&engine, engine.trunk_tx.clone(), trunk_rx, trunk_status);
        engine
    }

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

    fn spawn_image_pump(self: &Arc<Self>, image_rx: mpsc::Receiver<RawImage>) {
        let weak = Arc::downgrade(self);
        let spawned = std::thread::Builder::new()
            .name("sdrmm-images".to_string())
            .spawn(move || {
                while let Ok(raw) = image_rx.recv() {
                    let Some(engine) = weak.upgrade() else { return };
                    let _ = engine.image_tx.send(ImageCapture {
                        device_set: raw.device_set,
                        channel: raw.channel,
                        at: format!("{:.9}", jiff::Timestamp::now()),
                        freq_hz: raw.freq_hz,
                        source: raw.image.source,
                        mode: raw.image.mode,
                        complete: raw.image.complete,
                        lines: raw.image.lines,
                        picture: Arc::new(raw.image.picture),
                    });
                }
            });
        if let Err(e) = spawned {
            tracing::error!("failed to spawn image pump: {e}");
        }
    }

    #[must_use]
    pub fn subscribe_decoded(&self) -> broadcast::Receiver<DecodedRecord> {
        self.decoded_tx_out.subscribe()
    }

    pub fn publish_decoded(&self, record: DecodedRecord) {
        let _ = self.decoded_tx_out.send(record);
    }

    #[must_use]
    pub fn subscribe_images(&self) -> broadcast::Receiver<ImageCapture> {
        self.image_tx.subscribe()
    }

    #[must_use]
    pub fn decoded_dropped(&self) -> u64 {
        self.decoded_dropped.load(Ordering::Relaxed)
    }

    fn decoded_sink(&self, ds: u32, channel: u32) -> DecodedSink {
        DecodedSink::new(
            self.decoded_tx.clone(),
            self.image_queue_tx.clone(),
            self.decoded_dropped.clone(),
            ds,
            channel,
        )
    }

    #[must_use]
    pub fn recordings_dir(&self) -> Option<&Path> {
        self.recordings_dir.as_deref()
    }

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

    fn mark_device_fault(&self, ds: u32, err: DeviceError) {
        let mut inner = self.lock();
        if let Some(state) = inner.device_sets.get_mut(&ds) {
            state.status = DeviceSetStatus::Error;
            state.error = Some(err.to_string());
            state.fault = Some(fault_kind(&err));
            let recording = state.recording.take();
            if let Some(recording) = &recording {
                state.send_dsp(recording.stream, DspCommand::StopRecording);
            }
            let audio_recordings: Vec<ChannelAudioRecording> =
                state.audio_recordings.drain().map(|(_, rec)| rec).collect();
            let baseband_recordings: Vec<ChannelBasebandRecording> = state
                .baseband_recordings
                .drain()
                .map(|(_, rec)| rec)
                .collect();
            let channel_exports: Vec<NetworkExportState> =
                state.channel_exports.drain().map(|(_, rec)| rec).collect();
            let network_export = state.network_export.take();
            if let Some(export) = &network_export {
                state.send_dsp(export.stream, DspCommand::StopNetworkExport);
            }
            let history = state.time_machine.take();
            if let Some(history) = &history {
                state.send_dsp(history.stream, DspCommand::StopTimeMachine);
            }
            let scanner = state.scanner.take();
            let hunt = state.hunt.take();
            let runtime = state.runtime.clone();
            inner.leave_scan_session(ds);
            inner.revision += 1;
            drop(inner);
            if let Some(scanner) = scanner {
                scanner.stop_and_join();
            }
            if let Some(hunt) = hunt {
                hunt.stop_and_join();
            }
            lock_runtime(&runtime).stop();
            if let Some(recording) = recording {
                recording.join();
                self.emit(ServerEvent::StateChanged {
                    scope: StateScope::Recordings,
                });
            }
            for recording in audio_recordings {
                recording.join();
            }
            let mut wrote_files = false;
            for recording in baseband_recordings {
                recording.join();
                wrote_files = true;
            }
            for mut export in channel_exports {
                export.join();
            }
            if let Some(mut export) = network_export {
                export.join();
            }
            if let Some(history) = history {
                wrote_files |= history.capture.is_some();
                history.handle.join();
            }
            if wrote_files {
                self.emit(ServerEvent::StateChanged {
                    scope: StateScope::Recordings,
                });
            }
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(ds),
            });
        } else if inner.creating.contains(&ds) {
            inner.pending_faults.insert(ds, err);
        } else {
            drop(inner);
            tracing::warn!(ds, error = %err, "fault for removed device set");
        }
    }

    pub fn start_hotplug_prober(self: &Arc<Self>, interval: Duration) -> std::io::Result<()> {
        let weak = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("sdrmm-hotplug".to_string())
            .spawn(move || {
                let mut known = None;
                let mut missing_once = HashSet::new();
                let mut gate = hotplug::ProbeGate::default();
                let pace = hotplug::Pace::start();
                let mut woken = false;
                loop {
                    let Some(engine) = weak.upgrade() else { return };
                    engine.hotplug_tick(&mut known, &mut missing_once, &mut gate, woken);
                    drop(engine);
                    woken = pace.wait(interval);
                }
            })?;
        Ok(())
    }

    #[must_use]
    pub fn occupancy(&self) -> &Mutex<occupancy::Occupancy> {
        &self.occupancy
    }

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
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    };
                    let Some(engine) = weak.upgrade() else { return };
                    let now_ms = jiff::Timestamp::now().as_millisecond();
                    engine
                        .occupancy
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .observe(
                            &snapshot.db,
                            snapshot.center_hz,
                            snapshot.span_hz,
                            snapshot.lo_guard(),
                            now_ms,
                        );
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

    pub fn start_occupancy_collector(self: &Arc<Self>, interval: Duration) -> std::io::Result<()> {
        let weak = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("sdrmm-occupancy".to_string())
            .spawn(move || {
                loop {
                    let Some(engine) = weak.upgrade() else { return };
                    for (ds, streams) in engine.running_lanes() {
                        for stream in 0..streams {
                            let _ = engine.collect_occupancy_for(ds, stream);
                        }
                    }
                    drop(engine);
                    std::thread::sleep(interval);
                }
            })?;
        Ok(())
    }

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

    pub fn hotplug_tick_for_test(
        &self,
        known: &mut Option<Vec<String>>,
        missing_once: &mut HashSet<u32>,
    ) -> bool {
        self.hotplug_tick(
            known,
            missing_once,
            &mut hotplug::ProbeGate::default(),
            false,
        )
    }

    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<ServerEvent> {
        self.event_tx.subscribe()
    }

    pub fn emit_scope(&self, scope: StateScope) {
        self.emit(ServerEvent::StateChanged { scope });
    }

    #[must_use]
    pub fn adopt_device(&self, device_id: &str) -> Option<DeviceInfo> {
        self.registry.resolve(device_id)
    }

    /// The radios to choose from: what is attached to this machine right now, plus what the last
    /// network search found. A fresh network search runs behind the answer and announces itself
    /// when it changes the list, so nobody waits seconds for a list that is mostly already known.
    #[must_use]
    pub fn probe_devices(self: &Arc<Self>) -> Vec<DeviceInfo> {
        let attached = self.registry.probe_all();
        self.search_the_network();
        self.lock_discovery().merge(attached)
    }

    fn lock_discovery(&self) -> std::sync::MutexGuard<'_, discovery::Discovery> {
        self.discovery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn search_the_network(self: &Arc<Self>) {
        if !self.lock_discovery().claim(Instant::now()) {
            return;
        }
        let weak = Arc::downgrade(self);
        let spawned = std::thread::Builder::new()
            .name("sdrmm-discovery".to_string())
            .spawn(move || {
                let Some(engine) = weak.upgrade() else { return };
                let found = engine.registry.probe_all_deep();
                let attached = ids_of(&engine.registry.probe_all());
                let extras = found
                    .into_iter()
                    .filter(|device| !attached.contains(&device.id()))
                    .collect();
                let changed = engine.lock_discovery().searched(extras, Instant::now());
                if changed {
                    engine.emit(ServerEvent::StateChanged {
                        scope: StateScope::Devices,
                    });
                }
            });
        if let Err(error) = spawned {
            tracing::warn!("cannot search for network radios: {error}");
            self.lock_discovery().searched(Vec::new(), Instant::now());
        }
    }

    #[must_use]
    pub fn registry(&self) -> &DeviceRegistry {
        &self.registry
    }

    #[must_use]
    pub fn snapshot(&self) -> StateSnapshot {
        let trunk_systems = self.trunk_systems();
        let inner = self.lock();
        StateSnapshot {
            device_sets: inner
                .device_sets
                .iter()
                .map(|(id, s)| s.project(*id))
                .collect(),
            scan_session: inner.scan_session.as_ref().map(SessionState::project),
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

    pub fn emit_event(&self, event: ServerEvent) {
        self.emit(event);
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

fn teardown_set(mut removed: DeviceSetState) -> bool {
    if let Some(scanner) = removed.scanner.take() {
        scanner.stop_and_join();
    }
    if let Some(hunt) = removed.hunt.take() {
        hunt.stop_and_join();
    }
    lock_runtime(&removed.runtime).stop();
    let mut finalized = removed.recording.take().map(RecordingState::join).is_some();
    for (_, recording) in removed.audio_recordings.drain() {
        recording.join();
    }
    for (_, recording) in removed.baseband_recordings.drain() {
        recording.join();
        finalized = true;
    }
    for (_, mut export) in removed.channel_exports.drain() {
        export.join();
    }
    if let Some(mut export) = removed.network_export.take() {
        export.join();
    }
    if let Some(history) = removed.time_machine.take() {
        finalized |= history.capture.is_some();
        history.handle.join();
    }
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

fn join_network_writer(writer: JoinHandle<()>) {
    if writer.join().is_err() {
        tracing::error!("network export writer thread panicked");
    }
}

#[cfg(test)]
mod tests;
