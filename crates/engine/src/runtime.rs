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
    AUDIO_RATE, ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx,
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
    iq::{IqBlock, IqTap},
    network_export::NetworkExportTap,
    recording::RecorderTap,
    spectrum::{SpectrumAnalyzer, SpectrumFrame, SpectrumPlan},
    video::VideoPacket,
};

/// FFT size for the spectrum tap (: 1k–64k configurable; M0 fixes one size).
const FFT_SIZE: usize = 4096;
const TARGET_FPS: f64 = 30.0;
/// Ring depth in samples (~0.5 s at 2.4 Msps) — absorbs scheduling jitter before overrun.
pub(crate) const RING_CAPACITY: usize = 1 << 20;
/// Squelch gate feel, tuned for voice: 6 dB hysteresis and a 100 ms hold keep fades and
/// syllable gaps from chattering the gate.
const SQUELCH_HYSTERESIS_DB: f32 = 6.0;
const SQUELCH_HOLD_S: f32 = 0.1;

#[derive(Clone, Debug)]
pub struct SpectrumSnapshot {
    pub seq: u32,
    /// Sample count since capture start ( timestamp).
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
    /// Absolute RF frequency of the channel at the moment the frame was produced.
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

    /// A sink that discards everything, for hosts built outside a device set (tests).
    #[cfg(test)]
    pub(crate) fn null() -> Self {
        let (tx, rx) = mpsc::sync_channel(1);
        // Keeping the receiver alive would leak a thread; dropping it makes every send fail,
        // which the drop counter absorbs — exactly the "no decoder consumer" case.
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

/// The per-channel stream identities a host writes into, and the shared positions that keep
/// their timelines continuous across a pipeline rebuild. Carried as one value because a rebuild
/// hands the replacement host exactly what the old one had — a subscriber must not be able to
/// tell that the pipeline under it was swapped.
#[derive(Clone)]
pub(crate) struct ChannelSinks {
    pub(crate) pcm_tx: broadcast::Sender<PcmBlock>,
    /// 48 kHz-domain position of the channel's next PCM sample.
    pub(crate) pcm_pos: Arc<AtomicU64>,
    pub(crate) video_tx: broadcast::Sender<VideoPacket>,
    /// Channel-rate samples this channel has demodulated, which stamps its pictures.
    pub(crate) video_pos: Arc<AtomicU64>,
    pub(crate) iq_tx: broadcast::Sender<IqBlock>,
    /// Smoothed and peak-held level in dBFS, as `f32::to_bits`. An atomic pair rather than a
    /// message: the level is a *state* a reader samples whenever it likes, not an event, and a
    /// DSP thread must never block to publish one.
    pub(crate) level_db: Arc<AtomicU32>,
    pub(crate) peak_db: Arc<AtomicU32>,
}

/// One hosted channel on the DSP thread: DDC → channel filter → squelch gate → demod → PCM and
/// picture broadcasts. The filter confines both the squelch measurement and the demod input to
/// the mode's occupied bandwidth, so the gate cannot open on adjacent-channel energy and the
/// detector never sees it. Built control-side (construction may allocate and fail), then
/// moved to the DSP thread, where `process` runs lock-free with only the documented bounded
/// hand-offs (see the send sites).
pub(crate) struct ChannelHost {
    ddc: Ddc,
    filter: ChannelFilter,
    /// The params the filter was built from; `apply` rebuilds only on a change so
    /// squelch-only tweaks keep filter state.
    params: ChannelParams,
    squelch: Squelch,
    /// `None` bypasses the gate (always open); mirrors [`ChannelSettings::squelch_db`].
    threshold_db: Option<f32>,
    rx: Box<dyn ChannelRx>,
    outputs: ChannelOutputs,
    scratch: Vec<Complex<f32>>,
    filtered: Vec<Complex<f32>>,
    /// Audio frames per channel-rate IQ sample, with a running remainder so squelched
    /// zero-fill stays duration-accurate (WFM decimates 5:1 inside the demod).
    pcm_per_input: f64,
    zero_carry: f64,
    /// Interleave of the channel's audio, from its params: what the demod writes when it runs
    /// and what the squelched zero-fill has to match.
    audio_channels: u8,
    /// Where this channel's output goes. The positions inside are shared through the channel's
    /// media identity so stamps stay continuous across pipeline rebuilds; only the DSP thread
    /// advances them (hosts are swapped, never concurrent), so `Relaxed` suffices — the stamps
    /// consumers act on travel inside the messages themselves.
    sinks: ChannelSinks,
    /// Whether this channel scans out pictures, which decides whether the video position is
    /// worth advancing at all.
    produces_video: bool,
    /// Pictures sent by *this* host. Restarts with the host, unlike the position above: it
    /// numbers frames for display and nothing downstream resynchronizes on it.
    video_seq: u32,
    /// Offset from the device center, mirrored from the settings so decoder frames can be
    /// stamped with the absolute frequency they were heard on.
    offset_hz: f64,
    decoded: DecodedSink,
    /// Whether the type emits decoder frames at all; `emits_events` is this narrowed by what
    /// the channel says it needs, and is recomputed whenever its settings change.
    decodes: bool,
    /// Whether a closed squelch must still feed this channel (see [`ChannelHost::process`]).
    emits_events: bool,
    /// Zero-filled stand-in handed to a decoder while the gate is closed; reused, so the
    /// substitution costs no allocation in steady state.
    gated: Vec<Complex<f32>>,
    /// Bursts this channel's passband out to whoever is watching it. Idle — and free — until
    /// something subscribes.
    iq_tap: IqTap,
    /// The rate the tap paces itself against, mirrored from the pipeline it was built for.
    input_rate: f64,
    /// Runs on every block, gate or no gate: a channel's level is what tells an operator whether
    /// a closed squelch is a quiet channel or a threshold set too high.
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
        // A native-rate channel is handed the device's own samples: the DDC then only mixes the
        // channel down to baseband (input rate == output rate is an NCO and nothing else), which
        // is the whole point — ADS-B's 0.5 µs pulses do not survive a rate conversion.
        let input_rate = match descriptor.native_rate_range() {
            Some(_) => device_rate,
            None => descriptor.input_rate_hz,
        };
        let ddc = Ddc::new(device_rate, input_rate, settings.offset_hz)
            .map_err(|e| ChannelError::InvalidSettings(e.to_string()))?;
        let filter = sdrmm_channels::channel_filter(&settings.params)?;
        let rx = sdrmm_channels::create(ChannelCtx { input_rate }, settings)?;
        let decodes = descriptor.decoder_kind.is_some();
        let emits_events = decodes && rx.needs_gated_input();
        Ok(Box::new(Self {
            ddc,
            filter,
            params: settings.params.clone(),
            squelch: Squelch::new(
                input_rate,
                settings.squelch_db.unwrap_or(0.0),
                SQUELCH_HYSTERESIS_DB,
                SQUELCH_HOLD_S,
            ),
            threshold_db: settings.squelch_db,
            rx,
            outputs: ChannelOutputs::default(),
            scratch: Vec::new(),
            filtered: Vec::new(),
            pcm_per_input: f64::from(AUDIO_RATE) / input_rate,
            zero_carry: 0.0,
            audio_channels: sdrmm_channels::audio_channels(&settings.params),
            sinks,
            produces_video: descriptor.has_video,
            video_seq: 0,
            offset_hz: settings.offset_hz,
            decoded,
            decodes,
            emits_events,
            gated: Vec::new(),
            iq_tap: IqTap::new(input_rate),
            input_rate,
            meter: LevelMeter::new(input_rate),
        }))
    }

    fn process(&mut self, input: &[Complex<f32>], center_hz: f64) {
        self.ddc.process(input, &mut self.scratch);
        if self.scratch.is_empty() {
            return;
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
        let open = match self.threshold_db {
            Some(_) => self.squelch.process(&self.filtered),
            None => true,
        };
        // A picture is stamped with how much of the channel's own stream has gone by, so the
        // clock has to run whether or not the gate is open — a closed squelch is a gap in the
        // video, and it has to read as one.
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
            if !self.outputs.audio_pcm.is_empty() {
                // Deliberate bounded deviation from the "no allocation/locks" letter:
                // handing PCM to the encoder costs one Arc copy plus a tokio broadcast send
                // (short internal critical section, bounded channel, no syscalls) per
                // ~25 ms block, and no other thread ever holds that lock across blocking
                // work — the DSP thread cannot stall on it.
                let frames = self.outputs.audio_pcm.len() / usize::from(self.audio_channels);
                let stamp = self
                    .sinks
                    .pcm_pos
                    .fetch_add(frames as u64, Ordering::Relaxed);
                let _ = self.sinks.pcm_tx.send(PcmBlock {
                    start_frame: stamp,
                    channels: self.audio_channels,
                    payload: PcmPayload::Samples(Arc::from(self.outputs.audio_pcm.as_slice())),
                });
            }
        } else {
            // A decoder measures time in the samples it has processed — its bit clock, its
            // element timing, its inter-frame gaps. Skipping the gated span the way an audio
            // demod can would splice those spans out and hand it a stream that never had a
            // gap in it, so it is fed silence of the same length instead: the truth about
            // what was on the air, at the right duration. Audio-only demods keep the cheap
            // skip, which is where the squelch's CPU saving actually matters.
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
                let _ = self.sinks.pcm_tx.send(PcmBlock {
                    start_frame: stamp,
                    channels: self.audio_channels,
                    payload: PcmPayload::Silence(zeros),
                });
            }
        }
    }

    /// Send the channel's passband on to whoever is watching it.
    ///
    /// Deliberately above the squelch: a gate that is closed is precisely when an operator wants
    /// to see what is on the channel, and a tap that went quiet with the audio would be blind
    /// exactly when it is needed. Costs one atomic load while nobody is subscribed.
    fn tap_baseband(&mut self, center_hz: f64) {
        if self.sinks.iq_tx.receiver_count() == 0 {
            // A tap nobody is reading must not carry a half-filled burst across the gap until
            // someone does: those samples were not adjacent to the ones that will follow them.
            self.iq_tap.reset();
            return;
        }
        let rate = self.input_rate as f32;
        let center = center_hz + self.offset_hz;
        let sinks = &self.sinks;
        self.iq_tap.push(&self.filtered, rate, center, |block| {
            // send() only errors when there are no receivers, which the guard above already
            // handles; a subscriber that unsubscribed mid-block is the same non-event.
            let _ = sinks.iq_tx.send(block);
        });
    }

    fn publish_frames(&mut self, center_hz: f64, video_pos: u64) {
        // Decoder frames are rare (a handful per second even under ADS-B traffic) and a picture
        // is one `Arc` fifty times a second, so draining owned output here costs the same
        // bounded, documented deviation from the no-allocation letter as the PCM hand-off.
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
        if let Err(e) = self.rx.apply(settings) {
            tracing::error!(error = %e, "validated channel settings rejected on dsp thread");
        } else {
            // Only once the demod has taken the settings: the interleave the squelched
            // zero-fill claims must be the one the channel is now actually producing.
            self.audio_channels = channels;
        }
        // Settings decide it: an NFM channel needs the gated span only once it has been given
        // a tone mode, and stops needing it again when that is turned off.
        self.emits_events = self.decodes && self.rx.needs_gated_input();
    }

    pub(crate) fn position_changed(&mut self, fix: Option<&PositionFix>) {
        self.rx.position_changed(fix);
    }
}

pub(crate) enum DspCommand {
    AddChannel {
        id: u32,
        host: Box<ChannelHost>,
    },
    RemoveChannel {
        id: u32,
    },
    Retune {
        id: u32,
        offset_hz: f64,
    },
    ApplySettings {
        id: u32,
        settings: ChannelSettings,
    },
    PositionChanged {
        id: u32,
        fix: Option<PositionFix>,
    },
    /// Arm the recorder tap. From here the tap (and its queue sender) lives on the DSP
    /// thread; dropping it — via [`DspCommand::StopRecording`], or with the thread itself —
    /// closes the writer's queue, which is the finalize handshake.
    StartRecording {
        tap: RecorderTap,
    },
    StopRecording,
    StartNetworkExport {
        tap: NetworkExportTap,
    },
    StopNetworkExport,
}

/// The one-shot device-death report shared by every lane's capture sink.
type FatalReport = Box<dyn FnOnce(DeviceError) + Send>;

/// One receive stream's share of the runtime: the DSP thread draining its ring, its
/// spectrum broadcast, its command queue, and its overrun counter. Lanes share the device
/// and its clock (one radio, one sample rate); a lane's *centre* is its own wherever the
/// capability scopes tuning per stream, and everything downstream of the sink is per-stream.
struct Lane {
    meta: Arc<ArcSwap<DspMeta>>,
    spectrum_tx: broadcast::Sender<SpectrumSnapshot>,
    cmd_tx: mpsc::Sender<DspCommand>,
    overruns: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    dsp: Option<JoinHandle<()>>,
}

/// Owns the running device and its per-stream DSP lanes; drop/stop tears them down cleanly.
pub struct CaptureRuntime {
    /// Taken by [`CaptureRuntime::stop`] so the device is *dropped* there, not merely told to
    /// stop streaming. A USB backend holds its interface claim for as long as the handle
    /// lives, and auto-reconnect ( M5) re-opens the very same radio — leaving the
    /// dead set's handle alive would make every replug recovery fail with "busy".
    device: Option<Box<dyn SdrDevice>>,
    /// Index is the stream: lane k drains the sink that was `sinks[k]` in `rx_start`.
    lanes: Vec<Lane>,
    /// Which settings each lane holds on its own, captured from the device at start so
    /// [`CaptureRuntime::set_meta`] can resolve per-lane centres after the device is gone.
    per_stream: StreamScope,
}

impl CaptureRuntime {
    /// Wire the device to one fresh ring + DSP thread per rx stream and start streaming.
    /// Each lane's initial `DspMeta` centre is resolved through
    /// [`DeviceSettings::for_stream`], so a stored per-stream override is live from the very
    /// first drained block. `on_fatal` is the cold path a dying capture thread reports
    /// through (see [`RxSink::fail`]); the engine routes it to its fault drainer so device
    /// death becomes visible state.
    pub fn start(
        mut device: Box<dyn SdrDevice>,
        settings: &DeviceSettings,
        on_fatal: impl FnOnce(DeviceError) + Send + 'static,
    ) -> Result<Self, DeviceError> {
        let lane_count = device.capabilities().rx_streams.clamp(1, MAX_STREAMS) as usize;
        // Adapter discovery happens before capture begins, so a cold graphics driver can never
        // consume the ring's finite headroom. Every lane then shares the resulting device/queue.
        let spectrum_plan = SpectrumPlan::new(FFT_SIZE, lane_count);
        let per_stream = device.capabilities().per_stream;
        // Every DDC ratio, symbol clock, spectrum span and recorded `core:sample_rate` derives
        // from this; a default stood in here runs the whole chain mistuned and silent.
        let Some(sample_rate) = settings.sample_rate else {
            return Err(DeviceError::Unsupported(
                "device did not report a sample rate; everything downstream is derived from it"
                    .to_string(),
            ));
        };
        let fatal: Arc<Mutex<Option<FatalReport>>> = Arc::new(Mutex::new(Some(Box::new(on_fatal))));

        let mut sinks: Vec<RxSink> = Vec::with_capacity(lane_count);
        let mut lanes: Vec<Lane> = Vec::with_capacity(lane_count);
        // The per-lane halves that move onto the DSP thread, parallel to `lanes`.
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
            // Capture sink: lock-free write into the ring; dropped samples are counted,
            // never silently lost ( backpressure, CLAUDE.md no-silent-failure).
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
                    // The device is already streaming and earlier lanes are already running;
                    // tear all of that down instead of leaking it behind the error.
                    runtime.stop();
                    return Err(DeviceError::Io(format!("spawn dsp thread: {e}")));
                }
            }
        }
        Ok(runtime)
    }

    /// Spectrum subscription for one rx stream, or `None` when the stream is out of range —
    /// the engine turns that into its bad-request error naming the lane count.
    pub fn subscribe(&self, stream: u32) -> Option<broadcast::Receiver<SpectrumSnapshot>> {
        self.lanes
            .get(stream as usize)
            .map(|lane| lane.spectrum_tx.subscribe())
    }

    pub(crate) fn command_senders(&self) -> Vec<mpsc::Sender<DspCommand>> {
        self.lanes.iter().map(|lane| lane.cmd_tx.clone()).collect()
    }

    /// Shared per-lane ring-drop counters, in stream order, readable without taking the
    /// per-set runtime lock (state snapshots must never wait on a wedged device).
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

    /// What the device says it currently holds, which is not always what was asked for: a
    /// gain lands on the tuner's step grid, a rate on the resampler's achievable ratio. `None`
    /// once the device has been released ([`CaptureRuntime::stop`]).
    pub fn device_settings(&self) -> Option<DeviceSettings> {
        self.device.as_ref().map(|d| d.settings().clone())
    }

    pub fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        self.device
            .as_mut()
            .ok_or_else(|| DeviceError::Io("the device has been stopped".to_string()))?
            .apply(settings)
    }

    /// Stop streaming, release the device, and join every lane's DSP thread. Idempotent.
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
    let mut write_pos = 0usize;
    let mut since_last = 0usize;
    let mut total: u64 = 0;
    let mut dropped_seen: u64 = 0;
    let mut seq: u32 = 0;

    while !stop.load(Ordering::Acquire) {
        drain_commands(commands, &mut channels, &mut tap, &mut network_tap);
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
                        // send() only errors when there are no receivers — expected and fine.
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
                } else {
                    // Benign: the patch raced a removal; the removal already won.
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
            DspCommand::StartNetworkExport { tap: armed } => *network_tap = Some(armed),
            DspCommand::StopNetworkExport => *network_tap = None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use sdrmm_wire::{NfmParams, Sideband, SsbParams};

    use super::*;

    /// Device rate == channel rate makes the DDC transparent (no stages, offset 0), so these
    /// tests exercise exactly the channel filter + squelch + demod pipeline.
    const RATE: f64 = 48_000.0;
    const BLOCK: usize = 480;

    fn nfm_settings(squelch_db: Option<f32>) -> ChannelSettings {
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db,
            params: ChannelParams::Nfm(NfmParams::default()),
        }
    }

    /// A fresh media identity around `pcm_tx`, as the engine builds one per channel.
    fn sinks(pcm_tx: broadcast::Sender<PcmBlock>, pcm_pos: Arc<AtomicU64>) -> ChannelSinks {
        ChannelSinks {
            pcm_tx,
            pcm_pos,
            video_tx: broadcast::channel(8).0,
            video_pos: Arc::new(AtomicU64::new(0)),
            iq_tx: broadcast::channel(crate::iq::IQ_CHANNEL_CAP).0,
            level_db: Arc::new(AtomicU32::new(sdrmm_dsp::LEVEL_FLOOR_DB.to_bits())),
            peak_db: Arc::new(AtomicU32::new(sdrmm_dsp::LEVEL_FLOOR_DB.to_bits())),
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

    /// A host whose baseband tap is already subscribed, since the tap does nothing until one is.
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

    /// Run `input` through the host in blocks, draining PCM as it goes; returns every block.
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

    /// The tone's t=0 onset is broadband and may legitimately blip the gate open through
    /// the filter's transient; steady state — everything past `settle` samples — must be
    /// silence fill only.
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

    /// A −9 dBFS tone 15 kHz off-channel sits inside the DDC's flat passband but outside
    /// the 12.5 kHz NFM channel: it must not hold a −30 dB squelch open (the gate measures
    /// filtered power), so steady state is silence fill only.
    #[test]
    fn adjacent_tone_does_not_open_the_squelch() {
        let (mut host, mut rx) = host(&nfm_settings(Some(-30.0)));
        let blocks = run(&mut host, &mut rx, &tone(15_000.0, 0.35, 48_000));
        assert_silence_after_settle(&blocks, 24_000, "nfm");
    }

    /// Same setup on SSB: the mode's own selectivity is 2.7 kHz, so a full-scale tone
    /// 10 kHz away (silent in the audio) must not hold the gate open either.
    #[test]
    fn ssb_squelch_gates_on_the_sideband_not_the_ddc_passband() {
        let settings = ChannelSettings {
            offset_hz: 0.0,
            squelch_db: Some(-50.0),
            params: ChannelParams::Ssb(SsbParams {
                sideband: Sideband::Usb,
                bandwidth_hz: 2_700.0,
                agc: false,
            }),
        };
        let (mut host, mut rx) = host(&settings);
        let blocks = run(&mut host, &mut rx, &tone(10_000.0, 1.0, 48_000));
        assert_silence_after_settle(&blocks, 24_000, "ssb");
    }

    /// FM capture by a 2× adjacent-channel signal destroyed the wanted audio before the
    /// channel filter existed; with it, the 1 kHz modulation must dominate the audio.
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

    /// PCM stamps are the audio timeline: they must be gapless across blocks and across a
    /// pipeline rebuild that reuses the channel's shared sample position.
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

    /// The whole receive chain at a real radio's rate — DDC down from 2.4 MS/s, channel filter,
    /// discriminator, audio decimation, PCM hand-off — must run far enough ahead of realtime
    /// that the capture ring never backs up. This is the number behind "can the backend keep
    /// up with an RTL-SDR": if it ever approaches 1x, audio gaps stop being a client problem.
    ///
    /// The margin is deliberately wide (measured ~20x on a laptop) so a loaded CI runner cannot
    /// fail it for being slow — only a real regression in the signal path can. The DSP crates
    /// are opt-level 3 in the dev profile too (see the workspace manifest), so this holds for
    /// `cargo test` as much as for a release build.
    #[test]
    fn the_receive_chain_runs_well_ahead_of_realtime_at_a_radios_rate() {
        const DEVICE_RATE: f64 = 2_400_000.0;
        /// One SoapySDR read from an RTL-SDR: the block size the DSP thread really sees.
        const MTU: usize = 131_072;
        const SECONDS: f64 = 2.0;
        const MIN_FACTOR: f64 = 3.0;

        let settings = ChannelSettings {
            offset_hz: 250_000.0,
            squelch_db: None,
            params: ChannelParams::Wfm(sdrmm_wire::WfmParams::default()),
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
            // Drained as the encoder thread would, so the broadcast never lags into the timing.
            while pcm_rx.try_recv().is_ok() {}
        }
        let factor = SECONDS / start.elapsed().as_secs_f64();
        assert!(
            factor > MIN_FACTOR,
            "wfm at {DEVICE_RATE} Sa/s ran at {factor:.1}x realtime, under the {MIN_FACTOR}x floor"
        );
    }

    /// The tap's contract, end to end: an offset tone reaches the subscriber at *baseband*, which
    /// is what makes a constellation of it mean anything.
    #[test]
    fn the_baseband_tap_carries_the_channel_down_converted() {
        let mut settings = nfm_settings(None);
        settings.offset_hz = 3_000.0;
        let (mut host, mut rx) = tapped_host(&settings);

        // A tone on the channel's own frequency: at baseband it must be DC, not 3 kHz.
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

        // Down-converted to DC: consecutive samples share a phase, so their mean magnitude is the
        // magnitude of their mean. A tone left at 3 kHz would average to nearly nothing.
        let tail = &burst.samples[burst.samples.len() / 2..];
        let mean: Complex<f32> = tail.iter().sum::<Complex<f32>>() / tail.len() as f32;
        let power: f32 = tail.iter().map(|s| s.norm()).sum::<f32>() / tail.len() as f32;
        assert!(
            mean.norm() > power * 0.9,
            "tap output is not at baseband: |mean| {:.4} against mean |s| {power:.4}",
            mean.norm()
        );
    }

    /// A closed squelch is exactly when an operator wants to look at the passband, so the tap
    /// runs above the gate rather than with it.
    #[test]
    fn the_baseband_tap_keeps_running_through_a_closed_squelch() {
        let (mut host, mut rx) = tapped_host(&nfm_settings(Some(0.0)));

        // Far below any threshold: the gate stays shut for the whole run.
        let input = tone(0.0, 1e-6, crate::iq::IQ_BLOCK_SAMPLES * 4);
        for block in input.chunks(BLOCK) {
            host.process(block, 100_000_000.0);
        }

        assert!(
            rx.try_recv().is_ok(),
            "the tap went quiet with the audio instead of showing the closed channel"
        );
    }

    /// Nothing subscribed is the common case, and it must cost no sends and no allocation.
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
}
