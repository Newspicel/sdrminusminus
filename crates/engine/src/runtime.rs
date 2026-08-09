//! Per-device-set runtime (PLAN §7): the capture thread pushes IQ into an SPSC ring; a DSP
//! thread drains it, runs the spectrum tap and the hosted channels, and broadcasts snapshots
//! plus per-channel PCM. No locks and no steady-state allocation on the DSP hot path beyond
//! the one documented per-block snapshot hand-off (see [`ChannelHost::process`]) — device
//! settings arrive via an [`ArcSwap`] snapshot, channel changes via a command queue drained
//! between blocks, output leaves via broadcast channels.

use std::{
    sync::{
        Arc,
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
use sdrmm_wire::{ChannelParams, ChannelSettings};
use tokio::sync::broadcast;

use crate::audio::{PcmBlock, PcmPayload};

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
}

impl ChannelHost {
    pub(crate) fn build(
        device_rate: f64,
        settings: &ChannelSettings,
        pcm_tx: broadcast::Sender<PcmBlock>,
        pcm_pos: Arc<AtomicU64>,
    ) -> Result<Box<Self>, ChannelError> {
        let type_id = settings.params.type_id();
        let descriptor = sdrmm_channels::descriptors()
            .into_iter()
            .find(|d| d.type_id == type_id)
            .ok_or_else(|| ChannelError::UnknownType(type_id.to_owned()))?;
        let ddc = Ddc::new(device_rate, descriptor.input_rate_hz, settings.offset_hz)
            .map_err(|e| ChannelError::InvalidSettings(e.to_string()))?;
        let filter = sdrmm_channels::channel_filter(&settings.params)?;
        let rx = sdrmm_channels::create(
            ChannelCtx {
                input_rate: descriptor.input_rate_hz,
            },
            settings,
        )?;
        Ok(Box::new(Self {
            ddc,
            filter,
            params: settings.params.clone(),
            squelch: Squelch::new(
                descriptor.input_rate_hz,
                settings.squelch_db.unwrap_or(0.0),
                SQUELCH_HYSTERESIS_DB,
                SQUELCH_HOLD_S,
            ),
            threshold_db: settings.squelch_db,
            rx,
            outputs: ChannelOutputs::default(),
            scratch: Vec::new(),
            filtered: Vec::new(),
            pcm_per_input: f64::from(AUDIO_RATE) / descriptor.input_rate_hz,
            zero_carry: 0.0,
            pcm_pos,
            pcm_tx,
        }))
    }

    fn process(&mut self, input: &[Complex<f32>]) {
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
    }
}

/// Control-plane → DSP-thread channel operations (PLAN §7: settings via command queue,
/// applied between blocks). Handling MAY allocate — these are rare control events.
pub(crate) enum DspCommand {
    AddChannel { id: u32, host: Box<ChannelHost> },
    RemoveChannel { id: u32 },
    Retune { id: u32, offset_hz: f64 },
    ApplySettings { id: u32, settings: ChannelSettings },
}

/// Owns the running device and its DSP thread; drop/stop tears both down cleanly.
pub struct CaptureRuntime {
    device: Box<dyn SdrDevice>,
    meta: Arc<ArcSwap<DspMeta>>,
    spectrum_tx: broadcast::Sender<SpectrumSnapshot>,
    cmd_tx: mpsc::Sender<DspCommand>,
    overruns: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    dsp: Option<JoinHandle<()>>,
}

impl CaptureRuntime {
    /// Wire the device to a fresh ring + DSP thread and start streaming. `on_fatal` is the
    /// cold path a dying capture thread reports through (see [`RxSink::fail`]); the engine
    /// routes it to its fault drainer so device death becomes visible state.
    pub fn start(
        mut device: Box<dyn SdrDevice>,
        center_hz: f64,
        sample_rate: f64,
        on_fatal: impl FnOnce(DeviceError) + Send + 'static,
    ) -> Result<Self, DeviceError> {
        let (mut producer, mut consumer) = RingBuffer::<Complex<f32>>::new(RING_CAPACITY);
        let overruns = Arc::new(AtomicU64::new(0));
        let ov = overruns.clone();

        // Capture sink: lock-free write into the ring; dropped samples are counted, never
        // silently lost (PLAN §5 backpressure, CLAUDE.md no-silent-failure).
        let sink = RxSink::with_fatal_handler(
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
            on_fatal,
        );

        let meta = Arc::new(ArcSwap::from_pointee(DspMeta {
            center_hz,
            sample_rate,
        }));
        let (spectrum_tx, _) = broadcast::channel::<SpectrumSnapshot>(8);
        let (cmd_tx, cmd_rx) = mpsc::channel::<DspCommand>();
        let stop = Arc::new(AtomicBool::new(false));

        device.rx_start(sink)?;

        let dsp = {
            let meta = meta.clone();
            let tx = spectrum_tx.clone();
            let stop = stop.clone();
            let overruns = overruns.clone();
            std::thread::Builder::new()
                .name("sdrmm-dsp".to_string())
                .spawn(move || dsp_loop(&mut consumer, &cmd_rx, &meta, &tx, &stop, &overruns))
                .map_err(|e| DeviceError::Io(format!("spawn dsp thread: {e}")))?
        };

        Ok(Self {
            device,
            meta,
            spectrum_tx,
            cmd_tx,
            overruns,
            stop,
            dsp: Some(dsp),
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SpectrumSnapshot> {
        self.spectrum_tx.subscribe()
    }

    /// Clone of the DSP command queue for the engine's control-plane state: channel
    /// commands must be queued while the engine `inner` lock is held (see
    /// `DeviceSetState::cmd_tx`), which a method on the mutex-guarded runtime cannot offer.
    pub(crate) fn command_sender(&self) -> mpsc::Sender<DspCommand> {
        self.cmd_tx.clone()
    }

    /// Shared ring-drop counter, readable without taking the per-set runtime lock (state
    /// snapshots must never wait on a wedged device).
    pub(crate) fn overruns_counter(&self) -> Arc<AtomicU64> {
        self.overruns.clone()
    }

    pub fn set_meta(&self, center_hz: f64, sample_rate: f64) {
        self.meta.store(Arc::new(DspMeta {
            center_hz,
            sample_rate,
        }));
    }

    pub fn apply(&mut self, settings: &sdrmm_wire::DeviceSettings) -> Result<(), DeviceError> {
        self.device.apply(settings)
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.device.rx_stop();
        if let Some(handle) = self.dsp.take() {
            let _ = handle.join();
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
    let mut write_pos = 0usize;
    let mut since_last = 0usize;
    let mut total: u64 = 0;
    let mut dropped_seen: u64 = 0;
    let mut seq: u32 = 0;

    while !stop.load(Ordering::Acquire) {
        drain_commands(commands, &mut channels);
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
            for (_, host) in &mut channels {
                host.process(slice);
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
        let host = ChannelHost::build(RATE, settings, pcm_tx, Arc::new(AtomicU64::new(0)))
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
            host.process(chunk);
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
            host.process(chunk);
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
        let mut host =
            ChannelHost::build(RATE, &settings, pcm_tx.clone(), pos.clone()).expect("host");
        let input = tone(1_000.0, 0.5, 24_000);
        for chunk in input.chunks(BLOCK) {
            host.process(chunk);
        }
        // Swap in a fresh host the way a device rate change does.
        let mut host = ChannelHost::build(RATE, &settings, pcm_tx, pos).expect("rebuilt host");
        for chunk in input.chunks(BLOCK) {
            host.process(chunk);
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
