use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration,
};

use arc_swap::ArcSwap;
use num_complex::Complex;
use rtrb::RingBuffer;
use sdrmm_channels::{
    AUDIO_RATE, AudioChain, ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx,
    ClickProfile,
};
use sdrmm_device::{DeviceError, RxSink, SdrDevice};
use sdrmm_dsp::{Ddc, LevelMeter, Squelch};
use sdrmm_wire::{
    ChannelParams, ChannelSettings, DecoderEvent, DeviceSettings, MAX_STREAMS, PositionFix,
    StreamScope,
};
use tokio::sync::broadcast;

use crate::{
    audio::{PcmBlock, PcmPayload},
    audio_recording::AudioRecorderTap,
    iq::{IqBlock, IqTap},
    network_export::NetworkExportTap,
    recording::RecorderTap,
    spectrum::{SpectrumAnalyzer, SpectrumFrame, SpectrumPlan},
    time_machine::TimeMachineTap,
    video::VideoPacket,
};

const FFT_SIZE: usize = 4096;
const TARGET_FPS: f64 = 30.0;
pub(crate) const RING_CAPACITY: usize = 1 << 20;
const SQUELCH_HYSTERESIS_DB: f32 = 6.0;
const SQUELCH_HOLD_S: f32 = 0.1;

#[derive(Clone, Debug)]
pub struct SpectrumSnapshot {
    pub seq: u32,
    pub timestamp: u64,
    pub center_hz: f64,
    pub span_hz: f32,
    pub db: Arc<[f32]>,
}

#[derive(Clone, Copy)]
pub struct DspMeta {
    pub center_hz: f64,
    pub sample_rate: f64,
}

pub(crate) struct RawDecoded {
    pub(crate) device_set: u32,
    pub(crate) channel: u32,
    pub(crate) freq_hz: f64,
    pub(crate) event: DecoderEvent,
}

#[derive(Clone)]
pub(crate) struct DecodedSink {
    tx: mpsc::SyncSender<RawDecoded>,
    dropped: Arc<AtomicU64>,
    device_set: u32,
    channel: u32,
}

impl DecodedSink {
    pub(crate) fn new(
        tx: mpsc::SyncSender<RawDecoded>,
        dropped: Arc<AtomicU64>,
        device_set: u32,
        channel: u32,
    ) -> Self {
        Self {
            tx,
            dropped,
            device_set,
            channel,
        }
    }

    #[cfg(test)]
    pub(crate) fn null() -> Self {
        let (tx, rx) = mpsc::sync_channel(1);
        drop(rx);
        Self::new(tx, Arc::new(AtomicU64::new(0)), 0, 0)
    }

    fn publish(&self, freq_hz: f64, event: DecoderEvent) {
        let record = RawDecoded {
            device_set: self.device_set,
            channel: self.channel,
            freq_hz,
            event,
        };
        if self.tx.try_send(record).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Clone)]
pub(crate) struct ChannelSinks {
    pub(crate) pcm_tx: broadcast::Sender<PcmBlock>,
    pub(crate) pcm_pos: Arc<AtomicU64>,
    pub(crate) video_tx: broadcast::Sender<VideoPacket>,
    pub(crate) video_pos: Arc<AtomicU64>,
    pub(crate) iq_tx: broadcast::Sender<IqBlock>,
    pub(crate) level_db: Arc<AtomicU32>,
    pub(crate) peak_db: Arc<AtomicU32>,
    pub(crate) squelch_db: Arc<AtomicU32>,
}

pub(crate) struct ChannelHost {
    ddc: Ddc,
    filter: ChannelFilter,
    params: ChannelParams,
    squelch: Squelch,
    threshold_db: Option<f32>,
    audio_rec: Option<AudioRecorderTap>,
    rx: Box<dyn ChannelRx>,
    audio: AudioChain,
    has_audio: bool,
    outputs: ChannelOutputs,
    scratch: Vec<Complex<f32>>,
    filtered: Vec<Complex<f32>>,
    pcm_per_input: f64,
    zero_carry: f64,
    audio_channels: u8,
    sinks: ChannelSinks,
    produces_video: bool,
    video_seq: u32,
    offset_hz: f64,
    decoded: DecodedSink,
    decodes: bool,
    emits_events: bool,
    gated: Vec<Complex<f32>>,
    iq_tap: IqTap,
    baseband_rec: Option<RecorderTap>,
    baseband_export: Option<NetworkExportTap>,
    baseband_pos: u64,
    input_rate: f64,
    meter: LevelMeter,
}

impl ChannelHost {
    pub(crate) fn build(
        device_rate: f64,
        settings: &ChannelSettings,
        sinks: ChannelSinks,
        decoded: DecodedSink,
    ) -> Result<Box<Self>, ChannelError> {
        let type_id = settings.params.type_id();
        let descriptor = sdrmm_channels::descriptors()
            .into_iter()
            .find(|d| d.type_id == type_id)
            .ok_or_else(|| ChannelError::UnknownType(type_id.to_owned()))?;
        let input_rate = match descriptor.native_rate_range() {
            Some(_) => device_rate,
            None => descriptor.input_rate_hz,
        };
        let ddc = Ddc::new(device_rate, input_rate, settings.offset_hz)
            .map_err(|e| ChannelError::InvalidSettings(e.to_string()))?;
        let filter = sdrmm_channels::channel_filter(&settings.params)?;
        let rx = sdrmm_channels::create(ChannelCtx { input_rate }, settings)?;
        let audio_channels = sdrmm_channels::audio_channels(&settings.params);
        let decodes = descriptor.decoder_kind.is_some();
        let emits_events = decodes && rx.needs_gated_input();
        let mut squelch = Squelch::new(
            input_rate,
            settings.squelch_db.unwrap_or(0.0),
            SQUELCH_HYSTERESIS_DB,
            SQUELCH_HOLD_S,
        );
        squelch.set_auto_margin_db(settings.squelch_auto_db);
        Ok(Box::new(Self {
            ddc,
            filter,
            params: settings.params.clone(),
            squelch,
            threshold_db: settings.squelch_db,
            audio_rec: None,
            rx,
            audio: AudioChain::new(
                input_rate,
                audio_channels,
                &settings.audio,
                ClickProfile::for_params(&settings.params),
            ),
            has_audio: descriptor.has_audio,
            outputs: ChannelOutputs::default(),
            scratch: Vec::new(),
            filtered: Vec::new(),
            pcm_per_input: f64::from(AUDIO_RATE) / input_rate,
            zero_carry: 0.0,
            audio_channels,
            sinks,
            produces_video: descriptor.has_video,
            video_seq: 0,
            offset_hz: settings.offset_hz,
            decoded,
            decodes,
            emits_events,
            gated: Vec::new(),
            iq_tap: IqTap::new(input_rate),
            baseband_rec: None,
            baseband_export: None,
            baseband_pos: 0,
            input_rate,
            meter: LevelMeter::new(input_rate),
        }))
    }

    fn process(&mut self, input: &[Complex<f32>], center_hz: f64) {
        self.ddc.process(input, &mut self.scratch);
        if self.scratch.is_empty() {
            return;
        }
        if self.has_audio {
            self.audio.process_iq(&mut self.scratch);
        }
        self.filter.process(&self.scratch, &mut self.filtered);
        self.meter.process(&self.filtered);
        self.sinks
            .level_db
            .store(self.meter.level_db().to_bits(), Ordering::Relaxed);
        self.sinks
            .peak_db
            .store(self.meter.peak_db().to_bits(), Ordering::Relaxed);
        self.tap_baseband(center_hz);
        self.sink_baseband(center_hz);
        let open = match self.threshold_db {
            Some(_) => self.squelch.process(&self.filtered),
            None => true,
        };
        self.sinks.squelch_db.store(
            match self.threshold_db {
                Some(_) => self.squelch.threshold_db().to_bits(),
                None => f32::NAN.to_bits(),
            },
            Ordering::Relaxed,
        );
        let video_pos = if self.produces_video {
            self.sinks
                .video_pos
                .fetch_add(self.filtered.len() as u64, Ordering::Relaxed)
                + self.filtered.len() as u64
        } else {
            0
        };
        if open {
            self.outputs.reset();
            self.rx.process(&self.filtered, &mut self.outputs);
            self.publish_frames(center_hz, video_pos);
            if self.has_audio {
                self.audio.process_audio(&mut self.outputs.audio_pcm);
            }
            if !self.outputs.audio_pcm.is_empty() {
                let frames = self.outputs.audio_pcm.len() / usize::from(self.audio_channels);
                let stamp = self
                    .sinks
                    .pcm_pos
                    .fetch_add(frames as u64, Ordering::Relaxed);
                let block = PcmBlock {
                    start_frame: stamp,
                    channels: self.audio_channels,
                    payload: PcmPayload::Samples(Arc::from(self.outputs.audio_pcm.as_slice())),
                };
                self.record_audio(&block);
                let _ = self.sinks.pcm_tx.send(block);
            }
        } else {
            if self.emits_events {
                self.gated.clear();
                self.gated
                    .resize(self.filtered.len(), Complex::new(0.0, 0.0));
                self.outputs.reset();
                self.rx.process(&self.gated, &mut self.outputs);
                self.publish_frames(center_hz, video_pos);
            }
            self.zero_carry += self.filtered.len() as f64 * self.pcm_per_input;
            let zeros = self.zero_carry as usize;
            if zeros > 0 {
                self.zero_carry -= zeros as f64;
                let stamp = self
                    .sinks
                    .pcm_pos
                    .fetch_add(zeros as u64, Ordering::Relaxed);
                let block = PcmBlock {
                    start_frame: stamp,
                    channels: self.audio_channels,
                    payload: PcmPayload::Silence(zeros),
                };
                self.record_audio(&block);
                let _ = self.sinks.pcm_tx.send(block);
            }
        }
    }

    fn record_audio(&mut self, block: &PcmBlock) {
        if let Some(tap) = &self.audio_rec
            && !tap.push(block.clone())
        {
            self.audio_rec = None;
        }
    }

    fn tap_baseband(&mut self, center_hz: f64) {
        if self.sinks.iq_tx.receiver_count() == 0 {
            self.iq_tap.reset();
            return;
        }
        let rate = self.input_rate as f32;
        let center = center_hz + self.offset_hz;
        let sinks = &self.sinks;
        self.iq_tap.push(&self.filtered, rate, center, |block| {
            let _ = sinks.iq_tx.send(block);
        });
    }

    fn sink_baseband(&mut self, center_hz: f64) {
        let start = self.baseband_pos;
        self.baseband_pos += self.filtered.len() as u64;
        if self.baseband_rec.is_none() && self.baseband_export.is_none() {
            return;
        }
        let center = center_hz + self.offset_hz;
        if self
            .baseband_rec
            .as_ref()
            .is_some_and(|tap| !tap.push(&self.filtered, start, center))
        {
            self.baseband_rec = None;
        }
        if self
            .baseband_export
            .as_mut()
            .is_some_and(|tap| !tap.push(&self.filtered))
        {
            self.baseband_export = None;
        }
    }

    fn publish_frames(&mut self, center_hz: f64, video_pos: u64) {
        if !self.outputs.events.is_empty() {
            let freq_hz = center_hz + self.offset_hz;
            for event in self.outputs.events.drain(..) {
                self.decoded.publish(freq_hz, event);
            }
        }
        for picture in self.outputs.video.drain(..) {
            let _ = self.sinks.video_tx.send(VideoPacket {
                seq: self.video_seq,
                timestamp: video_pos,
                picture: Arc::new(picture),
            });
            self.video_seq = self.video_seq.wrapping_add(1);
        }
    }

    fn apply(&mut self, settings: ChannelSettings) {
        self.offset_hz = settings.offset_hz;
        self.threshold_db = settings.squelch_db;
        if let Some(db) = settings.squelch_db {
            self.squelch.set_threshold_db(db);
        }
        self.squelch.set_auto_margin_db(settings.squelch_auto_db);
        if settings.params != self.params {
            match sdrmm_channels::channel_filter(&settings.params) {
                Ok(filter) => {
                    self.filter = filter;
                    self.params = settings.params.clone();
                }
                Err(e) => {
                    tracing::error!(error = %e, "validated channel filter rejected on dsp thread");
                }
            }
        }
        let channels = sdrmm_channels::audio_channels(&settings.params);
        self.audio.configure(
            channels,
            &settings.audio,
            ClickProfile::for_params(&settings.params),
        );
        if let Err(e) = self.rx.apply(settings) {
            tracing::error!(error = %e, "validated channel settings rejected on dsp thread");
        } else {
            self.audio_channels = channels;
        }
        self.emits_events = self.decodes && self.rx.needs_gated_input();
    }

    pub(crate) fn position_changed(&mut self, fix: Option<&PositionFix>) {
        self.rx.position_changed(fix);
    }

    pub(crate) fn retuned(&mut self) {
        self.audio.reset();
        self.squelch.reset();
    }

    fn set_audio_recording(&mut self, tap: Option<AudioRecorderTap>) {
        self.audio_rec = tap;
    }

    fn set_baseband_recording(&mut self, tap: Option<RecorderTap>) {
        self.baseband_rec = tap;
    }

    fn set_baseband_export(&mut self, tap: Option<NetworkExportTap>) {
        self.baseband_export = tap;
    }
}

pub(crate) enum DspCommand {
    AddChannel { id: u32, host: Box<ChannelHost> },
    RemoveChannel { id: u32 },
    Retune { id: u32, offset_hz: f64 },
    ApplySettings { id: u32, settings: ChannelSettings },
    PositionChanged { id: u32, fix: Option<PositionFix> },
    StartRecording { tap: RecorderTap },
    StopRecording,
    StartChannelRecording { id: u32, tap: AudioRecorderTap },
    StopChannelRecording { id: u32 },
    StartBasebandRecording { id: u32, tap: RecorderTap },
    StopBasebandRecording { id: u32 },
    StartBasebandExport { id: u32, tap: NetworkExportTap },
    StopBasebandExport { id: u32 },
    StartNetworkExport { tap: NetworkExportTap },
    StopNetworkExport,
    StartTimeMachine { tap: Box<TimeMachineTap> },
    StopTimeMachine,
}

type FatalReport = Box<dyn FnOnce(DeviceError) + Send>;

struct Lane {
    meta: Arc<ArcSwap<DspMeta>>,
    spectrum_tx: broadcast::Sender<SpectrumSnapshot>,
    cmd_tx: mpsc::Sender<DspCommand>,
    overruns: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    dsp: Option<JoinHandle<()>>,
}

pub struct CaptureRuntime {
    device: Option<Box<dyn SdrDevice>>,
    lanes: Vec<Lane>,
    per_stream: StreamScope,
}

impl CaptureRuntime {
    pub fn start(
        mut device: Box<dyn SdrDevice>,
        settings: &DeviceSettings,
        on_fatal: impl FnOnce(DeviceError) + Send + 'static,
    ) -> Result<Self, DeviceError> {
        let lane_count = device.capabilities().rx_streams.clamp(1, MAX_STREAMS) as usize;
        let spectrum_plan = SpectrumPlan::new(FFT_SIZE, lane_count);
        let per_stream = device.capabilities().per_stream;
        let Some(sample_rate) = settings.sample_rate else {
            return Err(DeviceError::Unsupported(
                "device did not report a sample rate; everything downstream is derived from it"
                    .to_string(),
            ));
        };
        let fatal: Arc<Mutex<Option<FatalReport>>> = Arc::new(Mutex::new(Some(Box::new(on_fatal))));

        let mut sinks: Vec<RxSink> = Vec::with_capacity(lane_count);
        let mut lanes: Vec<Lane> = Vec::with_capacity(lane_count);
        let mut tails: Vec<(
            rtrb::Consumer<Complex<f32>>,
            mpsc::Receiver<DspCommand>,
            SpectrumAnalyzer,
        )> = Vec::with_capacity(lane_count);
        for stream in 0..lane_count {
            let (mut producer, consumer) = RingBuffer::<Complex<f32>>::new(RING_CAPACITY);
            let overruns = Arc::new(AtomicU64::new(0));
            let ov = overruns.clone();
            let fatal = fatal.clone();
            sinks.push(RxSink::with_fatal_handler(
                move |samples: &[Complex<f32>]| {
                    let free = producer.slots();
                    let take = free.min(samples.len());
                    if take > 0
                        && let Ok(chunk) = producer.write_chunk_uninit(take)
                    {
                        chunk.fill_from_iter(samples[..take].iter().copied());
                    }
                    if take < samples.len() {
                        ov.fetch_add((samples.len() - take) as u64, Ordering::Relaxed);
                    }
                },
                move |err| {
                    if let Some(report) = fatal
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take()
                    {
                        report(err);
                    }
                },
            ));
            let (spectrum_tx, _) = broadcast::channel::<SpectrumSnapshot>(8);
            let (cmd_tx, cmd_rx) = mpsc::channel::<DspCommand>();
            let center_hz = settings
                .for_stream(stream as u32, &per_stream)
                .center_hz
                .unwrap_or(crate::DEFAULT_CENTER_HZ);
            lanes.push(Lane {
                meta: Arc::new(ArcSwap::from_pointee(DspMeta {
                    center_hz,
                    sample_rate,
                })),
                spectrum_tx,
                cmd_tx,
                overruns,
                stop: Arc::new(AtomicBool::new(false)),
                dsp: None,
            });
            tails.push((consumer, cmd_rx, spectrum_plan.analyzer()));
        }

        device.rx_start(sinks)?;
        let mut runtime = Self {
            device: Some(device),
            lanes,
            per_stream,
        };

        for (index, (mut consumer, cmd_rx, analyzer)) in tails.into_iter().enumerate() {
            let lane = &runtime.lanes[index];
            let meta = lane.meta.clone();
            let tx = lane.spectrum_tx.clone();
            let stop = lane.stop.clone();
            let overruns = lane.overruns.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("sdrmm-dsp-{index}"))
                .spawn(move || {
                    dsp_loop(
                        &mut consumer,
                        &cmd_rx,
                        &meta,
                        &tx,
                        &stop,
                        &overruns,
                        analyzer,
                    )
                });
            match spawned {
                Ok(handle) => runtime.lanes[index].dsp = Some(handle),
                Err(e) => {
                    runtime.stop();
                    return Err(DeviceError::Io(format!("spawn dsp thread: {e}")));
                }
            }
        }
        Ok(runtime)
    }

    pub fn subscribe(&self, stream: u32) -> Option<broadcast::Receiver<SpectrumSnapshot>> {
        self.lanes
            .get(stream as usize)
            .map(|lane| lane.spectrum_tx.subscribe())
    }

    pub(crate) fn command_senders(&self) -> Vec<mpsc::Sender<DspCommand>> {
        self.lanes.iter().map(|lane| lane.cmd_tx.clone()).collect()
    }

    pub(crate) fn overruns_counters(&self) -> Vec<Arc<AtomicU64>> {
        self.lanes
            .iter()
            .map(|lane| lane.overruns.clone())
            .collect()
    }

    pub fn set_meta(&self, settings: &DeviceSettings) {
        let sample_rate = crate::sample_rate_of(settings);
        for (stream, lane) in self.lanes.iter().enumerate() {
            let center_hz = settings
                .for_stream(stream as u32, &self.per_stream)
                .center_hz
                .unwrap_or(crate::DEFAULT_CENTER_HZ);
            lane.meta.store(Arc::new(DspMeta {
                center_hz,
                sample_rate,
            }));
        }
    }

    pub fn device_settings(&self) -> Option<DeviceSettings> {
        self.device.as_ref().map(|d| d.settings().clone())
    }

    pub fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        self.device
            .as_mut()
            .ok_or_else(|| DeviceError::Io("the device has been stopped".to_string()))?
            .apply(settings)
    }

    pub fn stop(&mut self) {
        for lane in &self.lanes {
            lane.stop.store(true, Ordering::Release);
        }
        if let Some(mut device) = self.device.take() {
            device.rx_stop();
        }
        for lane in &mut self.lanes {
            if let Some(handle) = lane.dsp.take() {
                let _ = handle.join();
            }
        }
    }
}

impl Drop for CaptureRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

fn dsp_loop(
    consumer: &mut rtrb::Consumer<Complex<f32>>,
    commands: &mpsc::Receiver<DspCommand>,
    meta: &ArcSwap<DspMeta>,
    tx: &broadcast::Sender<SpectrumSnapshot>,
    stop: &AtomicBool,
    overruns: &AtomicU64,
    mut analyzer: SpectrumAnalyzer,
) {
    let mut hist = vec![Complex::new(0.0, 0.0); FFT_SIZE];
    let mut window = vec![Complex::new(0.0, 0.0); FFT_SIZE];
    let mut db = vec![0.0f32; FFT_SIZE];
    let mut channels: Vec<(u32, Box<ChannelHost>)> = Vec::new();
    let mut tap: Option<RecorderTap> = None;
    let mut network_tap: Option<NetworkExportTap> = None;
    let mut history: Option<TimeMachineTap> = None;
    let mut write_pos = 0usize;
    let mut since_last = 0usize;
    let mut total: u64 = 0;
    let mut dropped_seen: u64 = 0;
    let mut seq: u32 = 0;

    while !stop.load(Ordering::Acquire) {
        drain_commands(
            commands,
            &mut channels,
            &mut tap,
            &mut network_tap,
            &mut history,
        );
        let dropped = overruns.load(Ordering::Relaxed);
        total += dropped - dropped_seen;
        dropped_seen = dropped;
        let avail = consumer.slots();
        if avail == 0 {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        let snapshot = *meta.load_full();
        let hop = ((snapshot.sample_rate / TARGET_FPS) as usize).max(FFT_SIZE / 4);

        let Ok(chunk) = consumer.read_chunk(avail) else {
            continue;
        };
        let (a, b) = chunk.as_slices();
        for slice in [a, b] {
            if tap
                .as_ref()
                .is_some_and(|t| !t.push(slice, total, snapshot.center_hz))
            {
                tap = None;
            }
            if network_tap
                .as_mut()
                .is_some_and(|network| !network.push(slice))
            {
                network_tap = None;
            }
            if history
                .as_mut()
                .is_some_and(|keeper| !keeper.push(slice, snapshot.center_hz))
            {
                history = None;
            }
            for (_, host) in &mut channels {
                host.process(slice, snapshot.center_hz);
            }
            for &s in slice {
                hist[write_pos] = s;
                write_pos += 1;
                if write_pos == FFT_SIZE {
                    write_pos = 0;
                }
                total += 1;
                since_last += 1;
                if since_last >= hop {
                    since_last = 0;
                    for (i, w) in window.iter_mut().enumerate() {
                        *w = hist[(write_pos + i) % FFT_SIZE];
                    }
                    let frame = SpectrumFrame {
                        timestamp: total,
                        center_hz: snapshot.center_hz,
                        span_hz: snapshot.sample_rate as f32,
                    };
                    if let Some(completed) = analyzer.power_db(&window, &mut db, frame) {
                        seq = seq.wrapping_add(1);
                        let _ = tx.send(SpectrumSnapshot {
                            seq,
                            timestamp: completed.timestamp,
                            center_hz: completed.center_hz,
                            span_hz: completed.span_hz,
                            db: Arc::from(db.as_slice()),
                        });
                    }
                }
            }
        }
        chunk.commit_all();
    }
}

fn drain_commands(
    commands: &mpsc::Receiver<DspCommand>,
    channels: &mut Vec<(u32, Box<ChannelHost>)>,
    tap: &mut Option<RecorderTap>,
    network_tap: &mut Option<NetworkExportTap>,
    history: &mut Option<TimeMachineTap>,
) {
    while let Ok(cmd) = commands.try_recv() {
        match cmd {
            DspCommand::AddChannel { id, host } => {
                channels.retain(|(existing, _)| *existing != id);
                channels.push((id, host));
            }
            DspCommand::RemoveChannel { id } => channels.retain(|(existing, _)| *existing != id),
            DspCommand::Retune { id, offset_hz } => {
                if let Some((_, host)) = channels.iter_mut().find(|(existing, _)| *existing == id) {
                    host.ddc.set_offset(offset_hz);
                    host.offset_hz = offset_hz;
                    host.rx.retuned();
                    host.retuned();
                } else {
                    tracing::debug!(id, "retune for a channel no longer hosted");
                }
            }
            DspCommand::ApplySettings { id, settings } => {
                if let Some((_, host)) = channels.iter_mut().find(|(existing, _)| *existing == id) {
                    host.apply(settings);
                } else {
                    tracing::debug!(id, "settings for a channel no longer hosted");
                }
            }
            DspCommand::PositionChanged { id, fix } => {
                if let Some((_, host)) = channels.iter_mut().find(|(existing, _)| *existing == id) {
                    host.position_changed(fix.as_ref());
                } else {
                    tracing::debug!(id, "position for a channel no longer hosted");
                }
            }
            DspCommand::StartRecording { tap: armed } => *tap = Some(armed),
            DspCommand::StopRecording => *tap = None,
            DspCommand::StartChannelRecording { id, tap: armed } => {
                match channels.iter_mut().find(|(existing, _)| *existing == id) {
                    Some((_, host)) => host.set_audio_recording(Some(armed)),
                    None => tracing::debug!(id, "audio recording for a channel no longer hosted"),
                }
            }
            DspCommand::StopChannelRecording { id } => {
                if let Some((_, host)) = channels.iter_mut().find(|(existing, _)| *existing == id) {
                    host.set_audio_recording(None);
                }
            }
            DspCommand::StartBasebandRecording { id, tap: armed } => {
                match channels.iter_mut().find(|(existing, _)| *existing == id) {
                    Some((_, host)) => host.set_baseband_recording(Some(armed)),
                    None => {
                        tracing::debug!(id, "baseband recording for a channel no longer hosted");
                    }
                }
            }
            DspCommand::StopBasebandRecording { id } => {
                if let Some((_, host)) = channels.iter_mut().find(|(existing, _)| *existing == id) {
                    host.set_baseband_recording(None);
                }
            }
            DspCommand::StartBasebandExport { id, tap: armed } => {
                match channels.iter_mut().find(|(existing, _)| *existing == id) {
                    Some((_, host)) => host.set_baseband_export(Some(armed)),
                    None => tracing::debug!(id, "baseband export for a channel no longer hosted"),
                }
            }
            DspCommand::StopBasebandExport { id } => {
                if let Some((_, host)) = channels.iter_mut().find(|(existing, _)| *existing == id) {
                    host.set_baseband_export(None);
                }
            }
            DspCommand::StartNetworkExport { tap: armed } => *network_tap = Some(armed),
            DspCommand::StopNetworkExport => *network_tap = None,
            DspCommand::StartTimeMachine { tap: armed } => *history = Some(*armed),
            DspCommand::StopTimeMachine => *history = None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use sdrmm_wire::{
        AudioAgcMode, AudioProcessing, NfmParams, NoiseBlankerSettings, Sideband, SsbParams,
    };

    use super::*;

    const RATE: f64 = 48_000.0;
    const BLOCK: usize = 480;

    fn nfm_settings(squelch_db: Option<f32>) -> ChannelSettings {
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db,
            squelch_auto_db: None,
            params: ChannelParams::Nfm(NfmParams::default()),
            audio: Default::default(),
        }
    }

    fn sinks(pcm_tx: broadcast::Sender<PcmBlock>, pcm_pos: Arc<AtomicU64>) -> ChannelSinks {
        ChannelSinks {
            pcm_tx,
            pcm_pos,
            video_tx: broadcast::channel(8).0,
            video_pos: Arc::new(AtomicU64::new(0)),
            iq_tx: broadcast::channel(crate::iq::IQ_CHANNEL_CAP).0,
            level_db: Arc::new(AtomicU32::new(sdrmm_dsp::LEVEL_FLOOR_DB.to_bits())),
            peak_db: Arc::new(AtomicU32::new(sdrmm_dsp::LEVEL_FLOOR_DB.to_bits())),
            squelch_db: Arc::new(AtomicU32::new(f32::NAN.to_bits())),
        }
    }

    fn host(settings: &ChannelSettings) -> (Box<ChannelHost>, broadcast::Receiver<PcmBlock>) {
        let (pcm_tx, pcm_rx) = broadcast::channel(4096);
        let host = ChannelHost::build(
            RATE,
            settings,
            sinks(pcm_tx, Arc::new(AtomicU64::new(0))),
            DecodedSink::null(),
        )
        .expect("host builds");
        (host, pcm_rx)
    }

    fn tapped_host(
        settings: &ChannelSettings,
    ) -> (Box<ChannelHost>, broadcast::Receiver<crate::iq::IqBlock>) {
        let (pcm_tx, _pcm_rx) = broadcast::channel(4096);
        let mut built = sinks(pcm_tx, Arc::new(AtomicU64::new(0)));
        let (iq_tx, iq_rx) = broadcast::channel(64);
        built.iq_tx = iq_tx;
        let host =
            ChannelHost::build(RATE, settings, built, DecodedSink::null()).expect("host builds");
        (host, iq_rx)
    }

    fn tone(freq_hz: f64, amp: f32, len: usize) -> Vec<Complex<f32>> {
        (0..len)
            .map(|k| {
                let p = TAU * freq_hz * k as f64 / RATE;
                Complex::new(p.cos() as f32 * amp, p.sin() as f32 * amp)
            })
            .collect()
    }

    fn run(
        host: &mut ChannelHost,
        rx: &mut broadcast::Receiver<PcmBlock>,
        input: &[Complex<f32>],
    ) -> Vec<PcmBlock> {
        let mut blocks = Vec::new();
        for chunk in input.chunks(BLOCK) {
            host.process(chunk, 0.0);
            while let Ok(block) = rx.try_recv() {
                blocks.push(block);
            }
        }
        blocks
    }

    fn goertzel_power(samples: &[f32], freq_hz: f64) -> f64 {
        let w = TAU * freq_hz / RATE;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f64, 0.0f64);
        for &x in samples {
            let s0 = f64::from(x) + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        coeff.mul_add(-(s1 * s2), s1 * s1 + s2 * s2)
    }

    fn assert_silence_after_settle(blocks: &[PcmBlock], settle: u64, what: &str) {
        assert!(
            !blocks.is_empty(),
            "closed gate must still emit keep-alive fill"
        );
        let settled = blocks.iter().filter(|b| b.start_frame >= settle);
        let mut seen = 0usize;
        for block in settled {
            seen += 1;
            assert!(
                matches!(block.payload, PcmPayload::Silence(_)),
                "{what}: gate open on out-of-channel energy at stamp {}",
                block.start_frame
            );
        }
        assert!(seen > 0, "no settled blocks to judge");
    }

    #[test]
    fn adjacent_tone_does_not_open_the_squelch() {
        let (mut host, mut rx) = host(&nfm_settings(Some(-30.0)));
        let blocks = run(&mut host, &mut rx, &tone(15_000.0, 0.35, 48_000));
        assert_silence_after_settle(&blocks, 24_000, "nfm");
    }

    #[test]
    fn ssb_squelch_gates_on_the_sideband_not_the_ddc_passband() {
        let settings = ChannelSettings {
            offset_hz: 0.0,
            squelch_db: Some(-50.0),
            squelch_auto_db: None,
            params: ChannelParams::Ssb(SsbParams {
                sideband: Sideband::Usb,
                bandwidth_hz: 2_700.0,
            }),
            audio: Default::default(),
        };
        let (mut host, mut rx) = host(&settings);
        let blocks = run(&mut host, &mut rx, &tone(10_000.0, 1.0, 48_000));
        assert_silence_after_settle(&blocks, 24_000, "ssb");
    }

    #[test]
    fn an_automatic_squelch_finds_its_own_threshold() {
        let settings = ChannelSettings {
            squelch_db: Some(-100.0),
            squelch_auto_db: Some(8.0),
            ..nfm_settings(None)
        };
        let (mut host, mut rx) = host(&settings);
        let published = host.sinks.squelch_db.clone();

        let mut rng = 0x1234_5678u32;
        let mut noise = |len: usize| -> Vec<Complex<f32>> {
            (0..len)
                .map(|_| {
                    let mut next = || {
                        rng ^= rng << 13;
                        rng ^= rng >> 17;
                        rng ^= rng << 5;
                        (rng as f32 / u32::MAX as f32 - 0.5) * 0.002
                    };
                    Complex::new(next(), next())
                })
                .collect()
        };
        let quiet = run(&mut host, &mut rx, &noise(96_000));
        assert_silence_after_settle(&quiet, 24_000, "auto squelch on noise");
        let threshold = f32::from_bits(published.load(Ordering::Relaxed));
        assert!(
            (-100.0..-30.0).contains(&threshold),
            "threshold landed at {threshold} dB, nowhere near the noise it heard"
        );

        let loud = run(&mut host, &mut rx, &tone(0.0, 0.5, 48_000));
        assert!(
            loud.iter()
                .any(|b| matches!(b.payload, PcmPayload::Samples(_))),
            "the carrier never opened the gate"
        );
    }

    #[test]
    fn channel_filter_prevents_adjacent_capture() {
        let (mut host, mut rx) = host(&nfm_settings(None));
        let deviation = 2_500.0;
        let mut phase = 0.0f64;
        let wanted: Vec<Complex<f32>> = (0..96_000)
            .map(|k| {
                phase += TAU * deviation * (TAU * 1_000.0 * k as f64 / RATE).cos() / RATE;
                Complex::from_polar(1.0, phase as f32)
            })
            .collect();
        let interferer = tone(15_000.0, 2.0, 96_000);
        let input: Vec<Complex<f32>> = wanted.iter().zip(&interferer).map(|(a, b)| a + b).collect();

        let mut audio: Vec<f32> = Vec::new();
        for chunk in input.chunks(BLOCK) {
            host.process(chunk, 0.0);
            while let Ok(block) = rx.try_recv() {
                if let PcmPayload::Samples(samples) = block.payload {
                    audio.extend_from_slice(&samples);
                }
            }
        }
        let window = &audio[48_000..96_000];
        let tone_power = goertzel_power(window, 1_000.0);
        let probes = [700.0, 1_500.0, 2_300.0].map(|f| goertzel_power(window, f));
        let mean = probes.iter().sum::<f64>() / probes.len() as f64;
        assert!(
            tone_power > 10.0 * mean,
            "adjacent capture: tone {tone_power:.3e} vs probe mean {mean:.3e}"
        );
    }

    #[test]
    fn pcm_stamps_are_contiguous_across_rebuild() {
        let (pcm_tx, mut rx) = broadcast::channel(4096);
        let pos = Arc::new(AtomicU64::new(0));
        let settings = nfm_settings(None);
        let mut host = ChannelHost::build(
            RATE,
            &settings,
            sinks(pcm_tx.clone(), pos.clone()),
            DecodedSink::null(),
        )
        .expect("host");
        let input = tone(1_000.0, 0.5, 24_000);
        for chunk in input.chunks(BLOCK) {
            host.process(chunk, 0.0);
        }
        let mut host = ChannelHost::build(RATE, &settings, sinks(pcm_tx, pos), DecodedSink::null())
            .expect("rebuilt host");
        for chunk in input.chunks(BLOCK) {
            host.process(chunk, 0.0);
        }

        let mut expected = 0u64;
        let mut seen = 0usize;
        while let Ok(block) = rx.try_recv() {
            assert_eq!(block.start_frame, expected, "stamp gap at block {seen}");
            let frames = match &block.payload {
                PcmPayload::Samples(s) => s.len() / usize::from(block.channels),
                PcmPayload::Silence(n) => *n,
            };
            expected += frames as u64;
            seen += 1;
        }
        assert_eq!(
            expected, 48_000,
            "both hosts' PCM must be stamped end to end"
        );
    }

    #[test]
    fn the_receive_chain_runs_well_ahead_of_realtime_at_a_radios_rate() {
        const DEVICE_RATE: f64 = 2_400_000.0;
        const MTU: usize = 131_072;
        const SECONDS: f64 = 2.0;
        const MIN_FACTOR: f64 = 3.0;

        let settings = ChannelSettings {
            offset_hz: 250_000.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Wfm(sdrmm_wire::WfmParams::default()),
            audio: Default::default(),
        };
        let (pcm_tx, mut pcm_rx) = broadcast::channel::<PcmBlock>(4096);
        let mut host = ChannelHost::build(
            DEVICE_RATE,
            &settings,
            sinks(pcm_tx, Arc::new(AtomicU64::new(0))),
            DecodedSink::null(),
        )
        .expect("host builds");
        let block: Vec<Complex<f32>> = (0..MTU)
            .map(|k| {
                let p = TAU * 0.13 * k as f64;
                Complex::new(p.cos() as f32 * 0.4, p.sin() as f32 * 0.4)
            })
            .collect();

        let blocks = (DEVICE_RATE * SECONDS / MTU as f64) as usize;
        let start = std::time::Instant::now();
        for _ in 0..blocks {
            host.process(&block, 100_000_000.0);
            while pcm_rx.try_recv().is_ok() {}
        }
        let factor = SECONDS / start.elapsed().as_secs_f64();
        assert!(
            factor > MIN_FACTOR,
            "wfm at {DEVICE_RATE} Sa/s ran at {factor:.1}x realtime, under the {MIN_FACTOR}x floor"
        );
    }

    #[test]
    fn the_baseband_tap_carries_the_channel_down_converted() {
        let mut settings = nfm_settings(None);
        settings.offset_hz = 3_000.0;
        let (mut host, mut rx) = tapped_host(&settings);

        let input = tone(3_000.0, 0.5, crate::iq::IQ_BLOCK_SAMPLES * 4);
        for block in input.chunks(BLOCK) {
            host.process(block, 100_000_000.0);
        }

        let burst = rx.try_recv().expect("a subscribed tap sends bursts");
        assert_eq!(burst.samples.len(), crate::iq::IQ_BLOCK_SAMPLES);
        assert_eq!(
            burst.center_hz, 100_003_000.0,
            "the tap names the absolute centre"
        );
        assert_eq!(burst.sample_rate, RATE as f32);

        let tail = &burst.samples[burst.samples.len() / 2..];
        let mean: Complex<f32> = tail.iter().sum::<Complex<f32>>() / tail.len() as f32;
        let power: f32 = tail.iter().map(|s| s.norm()).sum::<f32>() / tail.len() as f32;
        assert!(
            mean.norm() > power * 0.9,
            "tap output is not at baseband: |mean| {:.4} against mean |s| {power:.4}",
            mean.norm()
        );
    }

    #[test]
    fn the_baseband_tap_keeps_running_through_a_closed_squelch() {
        let (mut host, mut rx) = tapped_host(&nfm_settings(Some(0.0)));

        let input = tone(0.0, 1e-6, crate::iq::IQ_BLOCK_SAMPLES * 4);
        for block in input.chunks(BLOCK) {
            host.process(block, 100_000_000.0);
        }

        assert!(
            rx.try_recv().is_ok(),
            "the tap went quiet with the audio instead of showing the closed channel"
        );
    }

    #[test]
    fn an_unwatched_tap_sends_nothing() {
        let (mut host, _pcm) = host(&nfm_settings(None));
        let mut rx = host.sinks.iq_tx.subscribe();
        drop(rx);
        rx = host.sinks.iq_tx.subscribe();
        drop(rx);

        let input = tone(0.0, 0.5, crate::iq::IQ_BLOCK_SAMPLES * 4);
        for block in input.chunks(BLOCK) {
            host.process(block, 100_000_000.0);
        }
        assert_eq!(host.sinks.iq_tx.receiver_count(), 0);
        assert!(host.sinks.iq_tx.subscribe().try_recv().is_err());
    }

    fn nfm_audio_settings(audio: AudioProcessing) -> ChannelSettings {
        ChannelSettings {
            audio,
            ..nfm_settings(None)
        }
    }

    fn fm_tone(f_mod: f64, deviation_hz: f64, len: usize) -> Vec<Complex<f32>> {
        let mut phase = 0.0f64;
        (0..len)
            .map(|k| {
                phase += TAU * deviation_hz * (TAU * f_mod * k as f64 / RATE).cos() / RATE;
                Complex::from_polar(1.0, phase as f32)
            })
            .collect()
    }

    fn samples(blocks: &[PcmBlock]) -> Vec<f32> {
        blocks
            .iter()
            .flat_map(|block| match &block.payload {
                PcmPayload::Samples(pcm) => pcm.to_vec(),
                PcmPayload::Silence(n) => vec![0.0; *n],
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / x.len() as f64).sqrt() as f32
    }

    #[test]
    fn the_audio_chain_runs_on_a_channel_that_never_had_one() {
        let quiet = fm_tone(1_000.0, 250.0, 192_000);
        let (mut plain, mut rx) = host(&nfm_settings(None));
        let untouched = samples(&run(&mut plain, &mut rx, &quiet));
        assert!(rms(&untouched[96_000..]) < 0.15, "signal was not quiet");

        let (mut levelled, mut rx) = host(&nfm_audio_settings(AudioProcessing {
            agc: AudioAgcMode::Fast,
            ..AudioProcessing::default()
        }));
        let lifted = samples(&run(&mut levelled, &mut rx, &quiet));
        let level = rms(&lifted[144_000..]);
        assert!((0.18..0.32).contains(&level), "levelled to {level}");
    }

    #[test]
    fn a_default_chain_leaves_the_audio_alone() {
        let input = fm_tone(1_000.0, 2_500.0, 48_000);
        let (mut plain, mut rx) = host(&nfm_settings(None));
        let plain_audio = samples(&run(&mut plain, &mut rx, &input));
        let (mut chained, mut rx) = host(&nfm_audio_settings(AudioProcessing::default()));
        assert_eq!(samples(&run(&mut chained, &mut rx, &input)), plain_audio);
    }

    #[test]
    fn the_blanker_cleans_impulses_before_the_channel_filter() {
        let mut input = fm_tone(1_000.0, 2_500.0, 96_000);
        for n in (1_000..input.len()).step_by(1_000) {
            input[n] = Complex::new(6.0, 0.0);
        }

        let peak = |settings: &ChannelSettings| {
            let (mut host, mut rx) = tapped_host(settings);
            let mut worst = 0.0f32;
            for chunk in input.chunks(BLOCK) {
                host.process(chunk, 0.0);
                while let Ok(block) = rx.try_recv() {
                    if block.timestamp < 8_192 {
                        continue;
                    }
                    worst = block.samples.iter().fold(worst, |m, s| m.max(s.norm()));
                }
            }
            worst
        };

        let dirty = peak(&nfm_settings(None));
        assert!(dirty > 1.5, "the impulses never reached the demod: {dirty}");
        let clean = peak(&nfm_audio_settings(AudioProcessing {
            blanker: NoiseBlankerSettings {
                enabled: true,
                threshold: 4.0,
            },
            ..AudioProcessing::default()
        }));
        assert!(
            clean < 1.3 && clean < 0.6 * dirty,
            "impulses survived into the demod: {dirty} -> {clean}"
        );
    }

    #[test]
    fn apply_switches_a_stage_on_in_place() {
        let (mut host, mut rx) = host(&nfm_settings(None));
        let quiet = fm_tone(1_000.0, 250.0, 96_000);
        run(&mut host, &mut rx, &quiet);
        host.apply(nfm_audio_settings(AudioProcessing {
            agc: AudioAgcMode::Fast,
            ..AudioProcessing::default()
        }));
        let lifted = samples(&run(&mut host, &mut rx, &quiet));
        let level = rms(&lifted[48_000..]);
        assert!((0.18..0.32).contains(&level), "levelled to {level}");
    }

    #[test]
    fn retuning_forgets_what_the_chain_learnt() {
        let (mut host, mut rx) = host(&nfm_audio_settings(AudioProcessing {
            agc: AudioAgcMode::Slow,
            ..AudioProcessing::default()
        }));
        run(&mut host, &mut rx, &fm_tone(1_000.0, 250.0, 96_000));
        host.retuned();
        let after = samples(&run(&mut host, &mut rx, &fm_tone(1_000.0, 2_500.0, 4_800)));
        assert!(rms(&after[..2_400]) < 1.0, "the old gain followed the tune");
    }
}
