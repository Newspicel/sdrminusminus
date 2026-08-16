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

use sdrmm_channels::{ChannelCtx, ChannelError};
use sdrmm_device::{DeviceError, DeviceRegistry, PlaybackShared, check_stream_settings};
use sdrmm_device_virtual::VirtualDriver;
use sdrmm_recorder::{data_path, meta_path};
use sdrmm_wire::{
    AudioRecordingStatus, Capabilities, ChannelDescriptor, ChannelInfo, ChannelLevel,
    ChannelParams, ChannelSettings, DecodedRecord, DeviceFault, DeviceInfo, DeviceSet,
    DeviceSetStatus, DeviceSettings, NetworkExportSettings, NetworkExportStatus, PlaybackRequest,
    PlaybackStatus, PositionFix, RecordingStatus, ScanSettings, ScannerStatus, ServerEvent,
    StateScope, StateSnapshot, TrunkSystemStatus,
};
use tokio::sync::broadcast;

pub mod audio;
pub mod audio_recording;
mod discovery;
mod history;
mod hotplug;
pub mod image;
pub mod iq;
mod network_export;
pub mod occupancy;
mod position;
pub mod recording;
pub mod runtime;
pub mod scanner;
mod sinks;
mod spectrum;
mod time_machine;
pub mod trunking;
pub mod video;
pub use audio::{AudioPacket, PcmBlock, PcmPayload};
pub use image::ImageCapture;
pub use iq::{IQ_BLOCK_SAMPLES, IQ_BLOCKS_PER_SEC, IqBlock};
pub use recording::FinalizedRecording;
pub use runtime::SpectrumSnapshot;
pub use trunking::TrunkSystem;
pub use video::{VideoPacket, VideoPicture};

use crate::{
    audio_recording::AudioRecordingShared,
    history::TimeMachineState,
    network_export::{NetworkExportShared, NetworkExportTap},
    recording::RecordingShared,
    runtime::{
        CaptureRuntime, ChannelHost, ChannelSinks, DecodedSink, DspCommand, RawDecoded, RawImage,
    },
    scanner::{ScanPlan, ScannerState},
    sinks::{BasebandSinks, ChannelBasebandRecording},
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

pub(crate) fn channel_input_rate(descriptor: &ChannelDescriptor, device_rate: f64) -> f64 {
    match descriptor.native_rate_range() {
        Some(_) => device_rate,
        None => descriptor.input_rate_hz,
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
    if let Some(margin) = settings.squelch_auto_db
        && (!margin.is_finite()
            || !(sdrmm_wire::MIN_SQUELCH_AUTO_MARGIN_DB..=sdrmm_wire::MAX_SQUELCH_AUTO_MARGIN_DB)
                .contains(&margin))
    {
        return Err(ChannelError::InvalidSettings(format!(
            "squelch_auto_db must be in {}..={} dB above the noise floor, got {margin}",
            sdrmm_wire::MIN_SQUELCH_AUTO_MARGIN_DB,
            sdrmm_wire::MAX_SQUELCH_AUTO_MARGIN_DB
        ))
        .into());
    }
    if let Err(reason) = settings.audio.validate() {
        return Err(ChannelError::InvalidSettings(reason).into());
    }
    if settings.audio.is_active() && !descriptor.has_audio {
        return Err(ChannelError::InvalidSettings(format!(
            "{} produces no audio, so it has nothing for the audio chain to process",
            descriptor.type_id
        ))
        .into());
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
    rate_patches: u32,
    cmd_txs: Vec<mpsc::Sender<DspCommand>>,
    overruns: Vec<Arc<AtomicU64>>,
    overruns_seen: u64,
    stalls: Vec<Arc<AtomicU64>>,
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
    creating: HashSet<u32>,
    pending_faults: HashMap<u32, DeviceError>,
    next_ds_id: u32,
    revision: u64,
}

pub struct Engine {
    registry: DeviceRegistry,
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
        let registry = builtin_registry(recordings_dir.clone());
        Self::with_registry(registry, recordings_dir)
    }

    #[must_use]
    pub fn with_registry(registry: DeviceRegistry, recordings_dir: Option<PathBuf>) -> Arc<Self> {
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
        trunking::spawn(&engine, trunk_rx, trunk_status);
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
            let runtime = state.runtime.clone();
            inner.revision += 1;
            drop(inner);
            if let Some(scanner) = scanner {
                scanner.stop_and_join();
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

    fn hotplug_tick(
        &self,
        known: &mut Option<Vec<String>>,
        missing_once: &mut HashSet<u32>,
        gate: &mut hotplug::ProbeGate,
        woken: bool,
    ) -> bool {
        let (grown, rec_faults, audio_rec_faults, export_faults, sink_faults, changed) = {
            let mut inner = self.lock();
            let mut grown: Vec<(u32, u64, u64)> = Vec::new();
            let mut rec_faults: Vec<(u32, String)> = Vec::new();
            let mut audio_rec_faults: Vec<(u32, u32, String)> = Vec::new();
            let mut export_faults: Vec<(u32, String)> = Vec::new();
            let mut baseband_faults: Vec<(u32, u32, String)> = Vec::new();
            let mut history_faults: Vec<(u32, String)> = Vec::new();
            let mut changed: Vec<u32> = Vec::new();
            for (id, s) in inner.device_sets.iter_mut() {
                let now = s.overruns_total();
                let delta = now - s.overruns_seen;
                s.overruns_seen = now;
                let mut dirty = delta > 0;
                if delta > 0 {
                    grown.push((*id, delta, s.take_worst_stall_ms()));
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
                for (ch, recording) in &mut s.audio_recordings {
                    let frames = recording.shared.frames();
                    if frames != recording.frames_seen {
                        recording.frames_seen = frames;
                        dirty = true;
                    }
                    if let Some(error) = recording.shared.error()
                        && !recording.error_seen
                    {
                        recording.error_seen = true;
                        audio_rec_faults.push((*id, *ch, error));
                        dirty = true;
                    }
                }
                for (ch, recording) in &mut s.baseband_recordings {
                    let samples = recording.shared.samples();
                    if samples != recording.samples_seen {
                        recording.samples_seen = samples;
                        dirty = true;
                    }
                    if let Some(error) = recording.shared.error()
                        && !recording.error_seen
                    {
                        recording.error_seen = true;
                        baseband_faults.push((*id, *ch, error));
                        dirty = true;
                    }
                }
                for (ch, export) in &mut s.channel_exports {
                    let samples = export.shared.samples();
                    if samples != export.samples_seen {
                        export.samples_seen = samples;
                        dirty = true;
                    }
                    if let Some(error) = export.shared.error()
                        && !export.error_seen
                    {
                        export.error_seen = true;
                        baseband_faults.push((*id, *ch, error));
                        dirty = true;
                    }
                }
                if let Some(history) = &mut s.time_machine {
                    let held = history.handle.shared().held();
                    if held != history.held_seen {
                        history.held_seen = held;
                        dirty = true;
                    }
                    if let Some(error) = history.handle.shared().error()
                        && !history.error_seen
                    {
                        history.error_seen = true;
                        history_faults.push((*id, error));
                        dirty = true;
                    }
                    if history.capture.is_some() && !history.handle.shared().capturing() {
                        history.capture = None;
                        dirty = true;
                    }
                }
                if let Some(export) = &mut s.network_export {
                    let samples = export.shared.samples();
                    if samples != export.samples_seen {
                        export.samples_seen = samples;
                        dirty = true;
                    }
                    if let Some(error) = export.shared.error()
                        && !export.error_seen
                    {
                        export.error_seen = true;
                        export_faults.push((*id, error));
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
            (
                grown,
                rec_faults,
                audio_rec_faults,
                export_faults,
                (baseband_faults, history_faults),
                changed,
            )
        };
        for (ds, dropped, stalled_ms) in grown {
            tracing::warn!(
                ds,
                dropped,
                stalled_ms,
                "capture ring overrun: device samples dropped while the dsp thread was held off"
            );
        }
        for (ds, error) in rec_faults {
            tracing::warn!(ds, error = %error, "recording fault");
        }
        for (ds, channel, error) in audio_rec_faults {
            tracing::warn!(ds, channel, error = %error, "audio recording fault");
        }
        for (ds, error) in export_faults {
            tracing::warn!(ds, error = %error, "network export fault");
        }
        let (baseband_faults, history_faults) = sink_faults;
        for (ds, channel, error) in baseband_faults {
            tracing::warn!(ds, channel, error = %error, "channel baseband sink fault");
        }
        for (ds, error) in history_faults {
            tracing::warn!(ds, error = %error, "time machine fault");
        }
        for ds in changed {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(ds),
            });
        }

        let Some(reason) = gate.should_probe(sdrmm_device::usb::fingerprint(), woken) else {
            return false;
        };
        if reason == hotplug::Probe::BusChanged {
            self.lock_discovery().expire();
        }

        let mut ids = ids_of(&self.registry.probe_all());
        if self.wants_a_deeper_look(&ids) {
            ids = ids_of(&self.registry.probe_all_deep());
        }

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
        // A radio the quick search cannot name — one that answers over the network, or one whose
        // vendor module only the deep search loads — still moved on the bus, and whoever has the
        // device list open is the one who should find out.
        if changed || reason == hotplug::Probe::BusChanged {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::Devices,
            });
        }
        changed
    }

    /// Whether the cheap search left a question only a full one can answer: a radio that is
    /// streaming but nothing found, or a faulted one that may have come back. Both are worth
    /// seconds; a healthy machine never gets here.
    fn wants_a_deeper_look(&self, ids: &[String]) -> bool {
        let inner = self.lock();
        inner.device_sets.values().any(|s| match s.status {
            DeviceSetStatus::Running => !ids.contains(&s.info.id()),
            DeviceSetStatus::Error => true,
            DeviceSetStatus::Idle => false,
        })
    }

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

        let opened = self
            .registry
            .open(&device_id)
            .and_then(|(info, mut device)| {
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
        let stalls = runtime.stall_counters();
        let runtime = Arc::new(Mutex::new(runtime));

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
            state.overruns = overruns;
            state.overruns_seen = 0;
            state.stalls = stalls;
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
            tracing::warn!(ds, error = %err, "reconnected capture died immediately");
            self.mark_device_fault(ds, err);
            return;
        }
        tracing::info!(ds, device = %device_id, "device set reconnected after replug");
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
    }

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

    pub fn create_device_set(&self, device_id: &str) -> Result<u32, EngineError> {
        self.refuse_reopen(device_id)?;
        let (info, device) = self.registry.open(device_id)?;
        if let Err(already) = self.refuse_reopen(&info.id()) {
            drop(device);
            return Err(already);
        }
        let capabilities = device.capabilities().clone();
        let settings = device.settings().clone();
        let playback = device.playback();

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
        let stalls = runtime.stall_counters();
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
                    fault: pending.as_ref().map(fault_kind),
                    recording: None,
                    audio_recordings: HashMap::new(),
                    baseband_recordings: HashMap::new(),
                    channel_exports: HashMap::new(),
                    network_export: None,
                    time_machine: None,
                    scanner: None,
                    rate_patches: 0,
                    cmd_txs,
                    overruns,
                    overruns_seen: 0,
                    stalls,
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

    pub fn remove_device_set(&self, ds: u32) -> Result<(), EngineError> {
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
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::Recordings,
            });
        }
        Ok(())
    }

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

    pub fn patch_device(&self, ds: u32, delta: DeviceSettings) -> Result<(), EngineError> {
        self.patch_device_from(ds, delta, PatchOrigin::Client)
    }

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
            let mut rate_change = false;
            if let Some(new_rate) = delta.sample_rate
                && new_rate != sample_rate_of(&state.settings)
            {
                if state.network_export.is_some() {
                    return Err(EngineError::NetworkExport(
                        "sample rate is locked while exporting; stop the export first".to_string(),
                    ));
                }
                if state.recording.is_some() {
                    return Err(EngineError::Recording(
                        "sample rate is locked while recording; stop the recording first"
                            .to_string(),
                    ));
                }
                if state.time_machine.is_some() {
                    return Err(EngineError::Recording(
                        "sample rate is locked while the time machine holds history; disarm it \
                         first"
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
            let guard = rate_change.then(|| {
                state.rate_patches += 1;
                RatePatchGuard { engine: self, ds }
            });
            (runtime, guard)
        };
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
            let locked_by_export = state.network_export.is_some();
            let owner = if locked_by_export {
                Some(("exporting", "stop the export first"))
            } else if state.recording.is_some() {
                Some(("recording", "stop the recording first"))
            } else if state.time_machine.is_some() {
                Some(("holding history", "disarm the time machine first"))
            } else {
                None
            };
            if let Some((owner, remedy)) = owner
                && delta.sample_rate.is_some_and(|r| r != old_rate)
            {
                drop(inner);
                let revert = DeviceSettings {
                    sample_rate: Some(old_rate),
                    ..DeviceSettings::default()
                };
                if let Err(e) = lock_runtime(&runtime).apply(&revert) {
                    let message = format!(
                        "sample rate is locked while {owner}, and reverting the device to \
                         {old_rate} Hz failed: {e}"
                    );
                    return Err(if locked_by_export {
                        EngineError::NetworkExport(message)
                    } else {
                        EngineError::Recording(message)
                    });
                }
                let message = format!("sample rate is locked while {owner}; {remedy}");
                return Err(if locked_by_export {
                    EngineError::NetworkExport(message)
                } else {
                    EngineError::Recording(message)
                });
            }
            state.settings.merge_from(&delta);
            if let Some(actual) = &actual {
                state.settings.merge_from(actual);
            }
            let export_center = state.network_export.as_ref().map(|export| {
                state
                    .settings
                    .for_stream(export.stream, &state.capabilities.per_stream)
                    .center_hz
                    .unwrap_or(DEFAULT_CENTER_HZ)
                    .round() as i64
            });
            if let (Some(export), Some(center_hz)) = (state.network_export.as_mut(), export_center)
            {
                export.center_hz = center_hz;
            }
            let history_center = state.time_machine.as_ref().map(|history| {
                state
                    .settings
                    .for_stream(history.stream, &state.capabilities.per_stream)
                    .center_hz
                    .unwrap_or(DEFAULT_CENTER_HZ)
                    .round() as i64
            });
            if let (Some(history), Some(center_hz)) = (state.time_machine.as_mut(), history_center)
            {
                history.center_hz = center_hz;
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
            let orphaned = state.release_baseband_sinks(id, stream);
            match built {
                Ok(mut host) => {
                    if let Some(media) = state.media.get(&id) {
                        host.position_changed(media.position.as_ref());
                    }
                    state.send_dsp(stream, DspCommand::RemoveChannel { id });
                    state.send_dsp(stream, DspCommand::AddChannel { id, host });
                    state.rearm_audio_recording(id, stream);
                    inner.revision += 1;
                    drop(inner);
                    self.close_baseband_sinks(ds, id, orphaned, "the channel was rebuilt");
                }
                Err(e) => {
                    tracing::error!(ds, channel = id, error = %e, "channel rebuild failed after rate change; removing channel");
                    state.channels.retain(|c| c.id != id);
                    dead.extend(state.media.remove(&id));
                    let recording = state.audio_recordings.remove(&id);
                    state.send_dsp(stream, DspCommand::RemoveChannel { id });
                    inner.revision += 1;
                    drop(inner);
                    if let Some(recording) = recording {
                        recording.join();
                    }
                    self.close_baseband_sinks(ds, id, orphaned, "the channel was removed");
                }
            }
            return;
        }
    }

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

    pub fn add_channel(
        &self,
        ds: u32,
        stream: u32,
        settings: ChannelSettings,
    ) -> Result<u32, EngineError> {
        let descriptor = descriptor_for(&settings.params)?;
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
                audio_recording: None,
                baseband_recording: None,
                network_export: None,
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
            drop(sdrmm_channels::create(
                ChannelCtx {
                    input_rate: descriptor.input_rate_hz,
                },
                &settings,
            )?);
        }
        let mut orphaned: Option<ChannelAudioRecording> = None;
        let mut orphaned_baseband = BasebandSinks::default();
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
                    orphaned_baseband = state.release_baseband_sinks(ch, stream);
                    state.send_dsp(stream, DspCommand::RemoveChannel { id: ch });
                    state.send_dsp(stream, DspCommand::AddChannel { id: ch, host });
                    if descriptor.has_audio {
                        state.rearm_audio_recording(ch, stream);
                    } else {
                        orphaned = state.audio_recordings.remove(&ch);
                    }
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
                    if prev.params != settings.params
                        || prev.squelch_db != settings.squelch_db
                        || prev.squelch_auto_db != settings.squelch_auto_db
                        || prev.audio != settings.audio
                    {
                        state.send_dsp(
                            stream,
                            DspCommand::ApplySettings {
                                id: ch,
                                settings: settings.clone(),
                            },
                        );
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
        if let Some(recording) = orphaned {
            tracing::info!(
                ds,
                channel = ch,
                "channel audio recording finished: the channel no longer produces audio"
            );
            recording.join();
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::Recordings,
            });
        }
        self.close_baseband_sinks(ds, ch, orphaned_baseband, "the channel was rebuilt");
        staged?;
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(())
    }

    pub fn remove_channel(&self, ds: u32, ch: u32) -> Result<(), EngineError> {
        let (handle, recording, baseband) = {
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
            let recording = state.audio_recordings.remove(&ch);
            let baseband = state.release_baseband_sinks(ch, stream);
            state.send_dsp(stream, DspCommand::RemoveChannel { id: ch });
            inner.revision += 1;
            (handle, recording, baseband)
        };
        if let Some(recording) = recording {
            recording.join();
        }
        self.close_baseband_sinks(ds, ch, baseband, "the channel was removed");
        if let Some(handle) = handle {
            handle.shutdown();
        }
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(())
    }

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
            let Some(dir) = self.recordings_dir.clone() else {
                return Err(EngineError::Recording(
                    "no recordings directory configured".to_string(),
                ));
            };
            std::fs::create_dir_all(&dir)
                .map_err(|e| EngineError::RecordingIo(format!("create {}: {e}", dir.display())))?;
            let started_at = jiff::Timestamp::now();
            let (sigmf, file) = recording::create_writer(
                &dir,
                &format!("rec_{ds}"),
                stream,
                started_at,
                rate,
                center,
                &hw,
            )?;
            let stem = sigmf.stem().to_path_buf();
            let (tap, position, messages, shared) = recording::create_tap();
            let writer = recording::spawn_writer(sigmf, messages, shared.clone())?;

            let (aborted, patch_in_flight) = {
                let mut inner = self.lock();
                match inner.device_sets.get_mut(&ds) {
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
            remove_recording_files(&stem);
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

    #[must_use]
    pub fn audio_recordings_dir(&self) -> Option<PathBuf> {
        self.recordings_dir
            .as_deref()
            .map(audio_recording::audio_dir)
    }

    pub fn start_channel_recording(
        &self,
        ds: u32,
        ch: u32,
    ) -> Result<AudioRecordingStatus, EngineError> {
        loop {
            let (stream, channels) = {
                let inner = self.lock();
                let state = inner
                    .device_sets
                    .get(&ds)
                    .ok_or(EngineError::DeviceSetNotFound(ds))?;
                let channel = state
                    .channels
                    .iter()
                    .find(|c| c.id == ch)
                    .ok_or(EngineError::ChannelNotFound(ch, ds))?;
                if state.audio_recordings.contains_key(&ch) {
                    return Err(EngineError::Recording(
                        "this channel is already recording".to_string(),
                    ));
                }
                if state.status != DeviceSetStatus::Running {
                    return Err(EngineError::Recording(
                        "device set is not running".to_string(),
                    ));
                }
                if !descriptor_for(&channel.settings.params)?.has_audio {
                    return Err(EngineError::Recording(format!(
                        "`{}` channels produce no audio to record",
                        channel.settings.params.type_id()
                    )));
                }
                (
                    channel.stream,
                    sdrmm_channels::audio_channels(&channel.settings.params),
                )
            };
            let Some(dir) = self.audio_recordings_dir() else {
                return Err(EngineError::Recording(
                    "no recordings directory configured".to_string(),
                ));
            };
            std::fs::create_dir_all(&dir)
                .map_err(|e| EngineError::RecordingIo(format!("create {}: {e}", dir.display())))?;
            let started_at = jiff::Timestamp::now();
            let writer = audio_recording::create_writer(
                &dir,
                ds,
                ch,
                started_at,
                sdrmm_channels::AUDIO_RATE,
                channels,
            )?;
            let path = writer.path().to_path_buf();
            let file = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned();
            let (tap, blocks, shared) = audio_recording::create_tap();
            let thread = audio_recording::spawn_writer(writer, blocks, shared.clone())?;

            let committed = {
                let mut inner = self.lock();
                match inner.device_sets.get_mut(&ds) {
                    Some(state)
                        if state.status == DeviceSetStatus::Running
                            && !state.audio_recordings.contains_key(&ch)
                            && state.channels.iter().any(|c| {
                                c.id == ch
                                    && c.stream == stream
                                    && sdrmm_channels::audio_channels(&c.settings.params)
                                        == channels
                            }) =>
                    {
                        let recording = ChannelAudioRecording {
                            file,
                            stream,
                            started_at: started_at.to_string(),
                            channels,
                            tap: tap.clone(),
                            shared,
                            writer: thread,
                            frames_seen: 0,
                            error_seen: false,
                        };
                        let status = recording.status();
                        state.audio_recordings.insert(ch, recording);
                        state.send_dsp(stream, DspCommand::StartChannelRecording { id: ch, tap });
                        inner.revision += 1;
                        Ok(status)
                    }
                    _ => Err((tap, thread, path)),
                }
            };
            match committed {
                Ok(status) => {
                    self.emit(ServerEvent::StateChanged {
                        scope: StateScope::DeviceSet(ds),
                    });
                    return Ok(status);
                }
                Err((tap, thread, path)) => {
                    drop(tap);
                    if thread.join().is_err() {
                        tracing::error!("audio recording writer thread panicked");
                    }
                    if let Err(e) = std::fs::remove_file(&path)
                        && e.kind() != std::io::ErrorKind::NotFound
                    {
                        tracing::warn!(path = %path.display(), error = %e, "aborted audio recording left a file behind");
                    }
                }
            }
        }
    }

    pub fn stop_channel_recording(
        &self,
        ds: u32,
        ch: u32,
    ) -> Result<AudioRecordingStatus, EngineError> {
        let recording = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let Some(recording) = state.audio_recordings.remove(&ch) else {
                return Err(EngineError::Recording(
                    "this channel is not recording".to_string(),
                ));
            };
            state.send_dsp(
                recording.stream,
                DspCommand::StopChannelRecording { id: ch },
            );
            inner.revision += 1;
            recording
        };
        let (file, started_at, channels, shared) = (
            recording.file.clone(),
            recording.started_at.clone(),
            recording.channels,
            recording.shared.clone(),
        );
        recording.join();
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::Recordings,
        });
        Ok(AudioRecordingStatus {
            file,
            started_at,
            channels,
            frames: shared.frames(),
            bytes: shared.bytes(),
            error: shared.error(),
        })
    }

    pub fn start_network_export(
        &self,
        ds: u32,
        node: String,
        stream: u32,
        settings: NetworkExportSettings,
    ) -> Result<NetworkExportStatus, EngineError> {
        check_export_request(&node, &settings)?;
        loop {
            let rate = {
                let inner = self.lock();
                let state = inner
                    .device_sets
                    .get(&ds)
                    .ok_or(EngineError::DeviceSetNotFound(ds))?;
                state.check_stream(stream)?;
                if state.network_export.is_some() {
                    return Err(EngineError::NetworkExport(
                        "another network export is already active".to_owned(),
                    ));
                }
                if state.status != DeviceSetStatus::Running {
                    return Err(EngineError::NetworkExport(
                        "device set is not running".to_owned(),
                    ));
                }
                sample_rate_of(&state.settings)
            };
            let (tap, shared, writer) = network_export::start(&settings)?;
            let commit = {
                let mut inner = self.lock();
                match inner.device_sets.get_mut(&ds) {
                    Some(state)
                        if state.status == DeviceSetStatus::Running
                            && state.network_export.is_none()
                            && state.rate_patches == 0
                            && state.check_stream(stream).is_ok()
                            && sample_rate_of(&state.settings) == rate =>
                    {
                        let center = state
                            .settings
                            .for_stream(stream, &state.capabilities.per_stream)
                            .center_hz
                            .unwrap_or(DEFAULT_CENTER_HZ);
                        let export = NetworkExportState {
                            node: node.clone(),
                            stream,
                            settings: settings.clone(),
                            sample_rate: rate.round() as u64,
                            center_hz: center.round() as i64,
                            shared,
                            writer: Some(writer),
                            overruns_at_start: state.overruns_total(),
                            samples_seen: 0,
                            error_seen: false,
                        };
                        let status = export.status(state.overruns_total());
                        state.network_export = Some(export);
                        state.send_dsp(stream, DspCommand::StartNetworkExport { tap });
                        inner.revision += 1;
                        NetworkExportCommit::Started(status)
                    }
                    Some(state) if state.rate_patches > 0 => NetworkExportCommit::Aborted {
                        tap,
                        writer,
                        patch_in_flight: true,
                    },
                    _ => NetworkExportCommit::Aborted {
                        tap,
                        writer,
                        patch_in_flight: false,
                    },
                }
            };
            match commit {
                NetworkExportCommit::Started(status) => {
                    self.emit(ServerEvent::StateChanged {
                        scope: StateScope::DeviceSet(ds),
                    });
                    return Ok(status);
                }
                NetworkExportCommit::Aborted {
                    tap,
                    writer,
                    patch_in_flight,
                } => {
                    drop(tap);
                    join_network_writer(writer);
                    if patch_in_flight {
                        return Err(EngineError::NetworkExport(
                            "a sample-rate change is in flight; retry once it completes".to_owned(),
                        ));
                    }
                }
            }
        }
    }

    pub fn stop_network_export(
        &self,
        ds: u32,
        node: &str,
    ) -> Result<NetworkExportStatus, EngineError> {
        let (export, overruns) = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let Some(active) = state.network_export.as_ref() else {
                return Err(EngineError::NetworkExport(
                    "network export is not active".to_owned(),
                ));
            };
            if active.node != node {
                return Err(EngineError::NetworkExport(format!(
                    "network export belongs to node `{}`",
                    active.node
                )));
            }
            let Some(export) = state.network_export.take() else {
                return Err(EngineError::NetworkExport(
                    "network export vanished while stopping".to_owned(),
                ));
            };
            state.send_dsp(export.stream, DspCommand::StopNetworkExport);
            let overruns = state.overruns.clone();
            inner.revision += 1;
            (export, overruns)
        };
        let overruns_now: u64 = overruns
            .iter()
            .map(|counter| counter.load(Ordering::Relaxed))
            .sum();
        let mut export = export;
        export.join();
        let status = export.status(overruns_now);
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(status)
    }

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
                    squelch_db: Some(f32::from_bits(
                        media.sinks.squelch_db.load(Ordering::Relaxed),
                    ))
                    .filter(|db| db.is_finite()),
                })
            })
            .collect()
    }

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

    #[must_use]
    pub fn channel_types(&self) -> Vec<ChannelDescriptor> {
        sdrmm_channels::descriptors()
    }

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
        let status = scanner.stop_and_join();
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(status)
    }

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

    pub(crate) fn scan_sample_rate(&self, ds: u32) -> Option<f64> {
        let inner = self.lock();
        let state = inner.device_sets.get(&ds)?;
        (state.status == DeviceSetStatus::Running).then(|| sample_rate_of(&state.settings))
    }

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
        self.subscribe_spectrum(ds, 0)
    }

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

fn teardown_set(mut removed: DeviceSetState) -> bool {
    if let Some(scanner) = removed.scanner.take() {
        scanner.stop_and_join();
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
