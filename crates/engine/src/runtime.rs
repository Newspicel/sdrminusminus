//! Per-device-set runtime (PLAN §7): one [`Lane`] per receive stream — the capture thread
//! pushes that stream's IQ into an SPSC ring; a DSP thread drains it, runs the spectrum tap
//! and the hosted channels, and broadcasts snapshots plus per-channel PCM. No locks and no
//! steady-state allocation on the DSP hot path beyond the one documented per-block snapshot
//! hand-off (see [`ChannelHost::process`]) — device settings arrive via an [`ArcSwap`]
//! snapshot, channel changes via a per-lane command queue drained between blocks, output
//! leaves via broadcast channels.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
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
use sdrmm_dsp::{Ddc, SpectrumAnalyzer, Squelch};
use sdrmm_wire::{
    ChannelParams, ChannelSettings, DecoderEvent, DeviceSettings, MAX_STREAMS, StreamScope,
};
use tokio::sync::broadcast;

use crate::{
    audio::{PcmBlock, PcmPayload},
    recording::RecorderTap,
};

/// FFT size for the spectrum tap (PLAN §9: 1k–64k configurable; M0 fixes one size).
const FFT_SIZE: usize = 4096;
/// Internal spectrum cadence; per-client fps throttling happens downstream (PLAN §9).
const TARGET_FPS: f64 = 30.0;
/// Ring depth in samples (~0.5 s at 2.4 Msps) — absorbs scheduling jitter before overrun.
pub(crate) const RING_CAPACITY: usize = 1 << 20;
/// Dynamic range below the per-frame peak used for the adaptive dB window default.
const DEFAULT_DB_RANGE: f32 = 80.0;
/// Squelch gate feel, tuned for voice: 6 dB hysteresis and a 100 ms hold keep fades and
/// syllable gaps from chattering the gate.
const SQUELCH_HYSTERESIS_DB: f32 = 6.0;
const SQUELCH_HOLD_S: f32 = 0.1;

/// One computed spectrum, broadcast to all subscribers of a device set. `db` is the full
/// DC-centered FFT; each subscriber decimates/quantizes to its own bin count (PLAN §9).
#[derive(Clone, Debug)]
pub struct SpectrumSnapshot {
    pub seq: u32,
    /// Sample count since capture start (PLAN §5 timestamp).
    pub timestamp: u64,
    pub center_hz: f64,
    pub span_hz: f32,
    pub db: Arc<[f32]>,
}

/// Live DSP-plane metadata the DSP thread reads per drain (never per sample).
#[derive(Clone, Copy)]
pub struct DspMeta {
    pub center_hz: f64,
    pub sample_rate: f64,
}

/// A decoder frame as it leaves the DSP plane, before the control plane stamps wall-clock
/// time onto it (the DSP thread never formats time).
pub(crate) struct RawDecoded {
    pub(crate) device_set: u32,
    pub(crate) channel: u32,
    /// Absolute RF frequency of the channel at the moment the frame was produced.
    pub(crate) freq_hz: f64,
    pub(crate) event: DecoderEvent,
}

/// The DSP plane's outlet for decoder frames. The queue is bounded, so a stalled control
/// plane costs frames rather than blocking the DSP thread — and every loss is counted and
/// surfaced (PLAN §5: bounded queue, never silent loss).
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

/// One hosted channel on the DSP thread: DDC → channel filter → squelch gate → demod → PCM
/// broadcast. The filter confines both the squelch measurement and the demod input to the
/// mode's occupied bandwidth, so the gate cannot open on adjacent-channel energy and the
/// detector never sees it. Built control-side (construction may allocate and fail), then
/// moved to the DSP thread, where `process` runs lock-free with only the documented bounded
/// PCM hand-off (see the send site).
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
    /// Audio samples per channel-rate IQ sample, with a running remainder so squelched
    /// zero-fill stays duration-accurate (WFM decimates 5:1 inside the demod).
    pcm_per_input: f64,
    zero_carry: f64,
    /// 48 kHz-domain position of the channel's next PCM sample. Shared through the
    /// channel's audio identity so stamps stay continuous across pipeline rebuilds; only
    /// the DSP thread advances it (hosts are swapped, never concurrent), so `Relaxed`
    /// suffices — the stamps consumers act on travel inside the messages themselves.
    pcm_pos: Arc<AtomicU64>,
    pcm_tx: broadcast::Sender<PcmBlock>,
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
}

impl ChannelHost {
    pub(crate) fn build(
        device_rate: f64,
        settings: &ChannelSettings,
        pcm_tx: broadcast::Sender<PcmBlock>,
        pcm_pos: Arc<AtomicU64>,
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
            pcm_pos,
            pcm_tx,
            offset_hz: settings.offset_hz,
            decoded,
            decodes,
            emits_events,
            gated: Vec::new(),
        }))
    }

    fn process(&mut self, input: &[Complex<f32>], center_hz: f64) {
        self.ddc.process(input, &mut self.scratch);
        if self.scratch.is_empty() {
            return;
        }
        self.filter.process(&self.scratch, &mut self.filtered);
        let open = match self.threshold_db {
            Some(_) => self.squelch.process(&self.filtered),
            None => true,
        };
        // send() only errors with no receivers (encoder mid-teardown) — expected and fine.
        if open {
            self.outputs.reset();
            self.rx.process(&self.filtered, &mut self.outputs);
            // Decoder frames are rare (a handful per second even under ADS-B traffic), so
            // draining owned events here costs the same bounded, documented deviation from
            // PLAN §7's no-allocation letter as the PCM hand-off below.
            if !self.outputs.events.is_empty() {
                let freq_hz = center_hz + self.offset_hz;
                for event in self.outputs.events.drain(..) {
                    self.decoded.publish(freq_hz, event);
                }
            }
            if !self.outputs.audio_pcm.is_empty() {
                // Deliberate bounded deviation from PLAN §7's "no allocation/locks" letter:
                // handing PCM to the encoder costs one Arc copy plus a tokio broadcast send
                // (short internal critical section, bounded channel, no syscalls) per
                // ~25 ms block, and no other thread ever holds that lock across blocking
                // work — the DSP thread cannot stall on it.
                let stamp = self
                    .pcm_pos
                    .fetch_add(self.outputs.audio_pcm.len() as u64, Ordering::Relaxed);
                let _ = self.pcm_tx.send(PcmBlock {
                    start_sample: stamp,
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
                if !self.outputs.events.is_empty() {
                    let freq_hz = center_hz + self.offset_hz;
                    for event in self.outputs.events.drain(..) {
                        self.decoded.publish(freq_hz, event);
                    }
                }
            }
            // A closed gate still emits (zeroed) audio so client jitter buffers stay alive;
            // silence travels as a bare length, so this path allocates nothing.
            self.zero_carry += self.filtered.len() as f64 * self.pcm_per_input;
            let zeros = self.zero_carry as usize;
            if zeros > 0 {
                self.zero_carry -= zeros as f64;
                let stamp = self.pcm_pos.fetch_add(zeros as u64, Ordering::Relaxed);
                let _ = self.pcm_tx.send(PcmBlock {
                    start_sample: stamp,
                    payload: PcmPayload::Silence(zeros),
                });
            }
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
                // The control plane validated these settings before queueing them; landing
                // here means an engine bug, so shout instead of dropping the failure.
                Err(e) => {
                    tracing::error!(error = %e, "validated channel filter rejected on dsp thread");
                }
            }
        }
        if let Err(e) = self.rx.apply(settings) {
            tracing::error!(error = %e, "validated channel settings rejected on dsp thread");
        }
        // Settings decide it: an NFM channel needs the gated span only once it has been given
        // a tone mode, and stops needing it again when that is turned off.
        self.emits_events = self.decodes && self.rx.needs_gated_input();
    }
}

/// Control-plane → DSP-thread channel operations (PLAN §7: settings via command queue,
/// applied between blocks). Handling MAY allocate — these are rare control events.
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
    /// Arm the recorder tap. From here the tap (and its queue sender) lives on the DSP
    /// thread; dropping it — via [`DspCommand::StopRecording`], or with the thread itself —
    /// closes the writer's queue, which is the finalize handshake.
    StartRecording {
        tap: RecorderTap,
    },
    StopRecording,
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
    /// lives, and auto-reconnect (PLAN §16 M5) re-opens the very same radio — leaving the
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
        // A radio reporting zero rx streams still gets one lane, or its device set would
        // have no spectrum and no channel host at all (design §10); the MAX_STREAMS ceiling
        // bounds the thread count against a buggy backend, whose own sink-count check then
        // refuses the mismatch.
        let lane_count = device.capabilities().rx_streams.clamp(1, MAX_STREAMS) as usize;
        let per_stream = device.capabilities().per_stream;
        let sample_rate = crate::sample_rate_of(settings);
        // One radio, one death: whichever lane's capture fails first reports for the whole
        // device, and the remaining sinks' reports collapse into the spent one-shot.
        let fatal: Arc<Mutex<Option<FatalReport>>> = Arc::new(Mutex::new(Some(Box::new(on_fatal))));

        let mut sinks: Vec<RxSink> = Vec::with_capacity(lane_count);
        let mut lanes: Vec<Lane> = Vec::with_capacity(lane_count);
        // The per-lane halves that move onto the DSP thread, parallel to `lanes`.
        let mut tails: Vec<(rtrb::Consumer<Complex<f32>>, mpsc::Receiver<DspCommand>)> =
            Vec::with_capacity(lane_count);
        for stream in 0..lane_count {
            let (mut producer, consumer) = RingBuffer::<Complex<f32>>::new(RING_CAPACITY);
            let overruns = Arc::new(AtomicU64::new(0));
            let ov = overruns.clone();
            let fatal = fatal.clone();
            // Capture sink: lock-free write into the ring; dropped samples are counted,
            // never silently lost (PLAN §5 backpressure, CLAUDE.md no-silent-failure).
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
            tails.push((consumer, cmd_rx));
        }

        device.rx_start(sinks)?;
        let mut runtime = Self {
            device: Some(device),
            lanes,
            per_stream,
        };

        for (index, (mut consumer, cmd_rx)) in tails.into_iter().enumerate() {
            let lane = &runtime.lanes[index];
            let meta = lane.meta.clone();
            let tx = lane.spectrum_tx.clone();
            let stop = lane.stop.clone();
            let overruns = lane.overruns.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("sdrmm-dsp-{index}"))
                .spawn(move || dsp_loop(&mut consumer, &cmd_rx, &meta, &tx, &stop, &overruns));
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

    /// Clones of every lane's DSP command queue, in stream order, for the engine's
    /// control-plane state: channel commands must be queued while the engine `inner` lock is
    /// held (see `DeviceSetState::cmd_txs`), which a method on the mutex-guarded runtime
    /// cannot offer.
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

    /// Push the resolved tuning to every lane's DSP meta. One radio still has one clock, so
    /// the sample rate is shared — but each lane's centre is resolved through
    /// [`DeviceSettings::for_stream`]: channel offsets are relative to the lane's centre, so
    /// pushing one shared centre would make a per-stream retune invisible to the DSP plane,
    /// which then decodes (and stamps decoder frames with) the wrong frequency while looking
    /// fine.
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

/// The DSP thread body (PLAN §7): drain commands, then drain the ring through the hosted
/// channels and a rolling FFT window emitting a spectrum every `hop` samples. `hop` derives
/// from the live sample rate, so cadence stays ~`TARGET_FPS` regardless of tuning.
fn dsp_loop(
    consumer: &mut rtrb::Consumer<Complex<f32>>,
    commands: &mpsc::Receiver<DspCommand>,
    meta: &ArcSwap<DspMeta>,
    tx: &broadcast::Sender<SpectrumSnapshot>,
    stop: &AtomicBool,
    overruns: &AtomicU64,
) {
    let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE);
    let mut hist = vec![Complex::new(0.0, 0.0); FFT_SIZE];
    let mut window = vec![Complex::new(0.0, 0.0); FFT_SIZE];
    let mut db = vec![0.0f32; FFT_SIZE];
    let mut channels: Vec<(u32, Box<ChannelHost>)> = Vec::new();
    let mut tap: Option<RecorderTap> = None;
    let mut write_pos = 0usize;
    let mut since_last = 0usize;
    let mut total: u64 = 0;
    let mut dropped_seen: u64 = 0;
    let mut seq: u32 = 0;

    while !stop.load(Ordering::Acquire) {
        drain_commands(commands, &mut channels, &mut tap);
        // Ring overruns advance the sample clock too: `timestamp` stays aligned with real
        // capture time across drops instead of silently compressing it (PLAN §5 sample-count
        // timestamps; the control plane surfaces the same counter as `DeviceSet.overruns`).
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
            // `total` is the stream position of `slice[0]` here — the per-sample advance
            // below runs within this same iteration. A failed push disarms the tap: the
            // fault is already in the recording's shared state, and a lossless recording
            // must never continue with silent holes (PLAN §5).
            if tap
                .as_ref()
                .is_some_and(|t| !t.push(slice, total, snapshot.center_hz))
            {
                tap = None;
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
                    analyzer.power_db(&window, &mut db);
                    seq = seq.wrapping_add(1);
                    // send() only errors when there are no receivers — expected and fine.
                    let _ = tx.send(SpectrumSnapshot {
                        seq,
                        timestamp: total,
                        center_hz: snapshot.center_hz,
                        span_hz: snapshot.sample_rate as f32,
                        db: Arc::from(db.as_slice()),
                    });
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
) {
    while let Ok(cmd) = commands.try_recv() {
        match cmd {
            DspCommand::AddChannel { id, host } => {
                // Swap-rebuilds send Remove first, but tolerate a duplicate add anyway.
                channels.retain(|(existing, _)| *existing != id);
                channels.push((id, host));
            }
            DspCommand::RemoveChannel { id } => channels.retain(|(existing, _)| *existing != id),
            DspCommand::Retune { id, offset_hz } => {
                if let Some((_, host)) = channels.iter_mut().find(|(existing, _)| *existing == id) {
                    host.ddc.set_offset(offset_hz);
                    host.offset_hz = offset_hz;
                    // The channel is now listening to a different signal; anything it accreted
                    // about the previous one has to go (`ChannelRx::retuned`). The control
                    // plane sends no `ApplySettings` for an offset-only patch, so this is the
                    // only place a decoder learns it moved.
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
            DspCommand::StartRecording { tap: armed } => *tap = Some(armed),
            DspCommand::StopRecording => *tap = None,
        }
    }
}

/// The default adaptive dB window for a snapshot: `[peak - DEFAULT_DB_RANGE, peak]` (PLAN §9).
#[must_use]
pub fn adaptive_db_window(db: &[f32]) -> (f32, f32) {
    let peak = db.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let peak = if peak.is_finite() { peak } else { 0.0 };
    (peak - DEFAULT_DB_RANGE, peak)
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

    fn host(settings: &ChannelSettings) -> (Box<ChannelHost>, broadcast::Receiver<PcmBlock>) {
        let (pcm_tx, pcm_rx) = broadcast::channel(4096);
        let host = ChannelHost::build(
            RATE,
            settings,
            pcm_tx,
            Arc::new(AtomicU64::new(0)),
            DecodedSink::null(),
        )
        .expect("host builds");
        (host, pcm_rx)
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
        let settled = blocks.iter().filter(|b| b.start_sample >= settle);
        let mut seen = 0usize;
        for block in settled {
            seen += 1;
            assert!(
                matches!(block.payload, PcmPayload::Silence(_)),
                "{what}: gate open on out-of-channel energy at stamp {}",
                block.start_sample
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
            pcm_tx.clone(),
            pos.clone(),
            DecodedSink::null(),
        )
        .expect("host");
        let input = tone(1_000.0, 0.5, 24_000);
        for chunk in input.chunks(BLOCK) {
            host.process(chunk, 0.0);
        }
        // Swap in a fresh host the way a device rate change does.
        let mut host = ChannelHost::build(RATE, &settings, pcm_tx, pos, DecodedSink::null())
            .expect("rebuilt host");
        for chunk in input.chunks(BLOCK) {
            host.process(chunk, 0.0);
        }

        let mut expected = 0u64;
        let mut seen = 0usize;
        while let Ok(block) = rx.try_recv() {
            assert_eq!(block.start_sample, expected, "stamp gap at block {seen}");
            let len = match &block.payload {
                PcmPayload::Samples(s) => s.len(),
                PcmPayload::Silence(n) => *n,
            };
            expected += len as u64;
            seen += 1;
        }
        assert_eq!(
            expected, 48_000,
            "both hosts' PCM must be stamped end to end"
        );
    }
}
