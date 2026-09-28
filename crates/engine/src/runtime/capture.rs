use std::{
    collections::BTreeMap,
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
use sdrmm_device::{DeviceError, MarkPoster, RxSink, SdrDevice, SinkItem, SweepPlan, SweepSink};
use sdrmm_dsp::SpectrumAnalyzer as CpuSpectrumAnalyzer;
use sdrmm_wire::{DeviceSettings, MAX_STREAMS, StreamScope, array::MAX_VIRTUAL_LANES};
use tokio::sync::broadcast;

use super::{
    DspCommand, DspMeta, FFT_SIZE, SpectrumSnapshot, Waker,
    clip::ClipMeter,
    retire::Reclaimer,
    worker::{LaneShared, dsp_loop},
};
use crate::{
    array::{TapPort, TapWriter},
    capture_ring::{CaptureConsumer, CaptureProducer, capture_ring},
    publishing::spectrum::SpectrumPublisher,
    spectrum::{SpectrumAnalyzer, SpectrumFrame, SpectrumPlan},
};

pub(crate) const RING_SECONDS: f64 = 0.1;
const LIVE_MAX_AGE: Duration = Duration::from_millis(100);
const RING_MIN: usize = 1 << 17;
const RING_MAX: usize = 1 << 23;
const SPECTRUM_TAP_SLOTS: usize = 8;

pub(crate) fn ring_capacity(sample_rate: f64) -> usize {
    ((sample_rate * RING_SECONDS) as usize).clamp(RING_MIN, RING_MAX)
}

fn max_age_for(device: &dyn SdrDevice) -> Duration {
    if device.playback().is_some() {
        Duration::MAX
    } else {
        LIVE_MAX_AGE
    }
}

type FatalReport = Box<dyn FnOnce(DeviceError) + Send>;

struct Lane {
    meta: Arc<ArcSwap<DspMeta>>,
    spectrum_tx: broadcast::Sender<SpectrumSnapshot>,
    cmd_tx: mpsc::Sender<DspCommand>,
    overruns: Arc<AtomicU64>,
    stalled_us: Arc<AtomicU64>,
    clip: Arc<ClipMeter>,
    waker: Arc<Waker>,
    stop: Arc<AtomicBool>,
    dsp: Option<JoinHandle<()>>,
    capture_metrics: Arc<crate::metrics::QueueMetrics>,
    spectrum_metrics: Arc<crate::metrics::QueueMetrics>,
}

impl Lane {
    fn new(
        meta: DspMeta,
        spectrum_tx: broadcast::Sender<SpectrumSnapshot>,
        cmd_tx: mpsc::Sender<DspCommand>,
        capture_metrics: Arc<crate::metrics::QueueMetrics>,
    ) -> Self {
        Self {
            meta: Arc::new(ArcSwap::from_pointee(meta)),
            spectrum_tx,
            cmd_tx,
            overruns: capture_metrics.dropped_counter(),
            stalled_us: Arc::new(AtomicU64::new(0)),
            clip: Arc::new(ClipMeter::default()),
            waker: Arc::new(Waker::default()),
            stop: Arc::new(AtomicBool::new(false)),
            dsp: None,
            capture_metrics,
            spectrum_metrics: Arc::new(crate::metrics::QueueMetrics::default()),
        }
    }

    fn set(&self, center_hz: f64, sample_rate: f64, dc_block: bool) {
        if self.meta.load().sample_rate != sample_rate {
            let bands = super::subbands::Subbands::new(sample_rate);
            if let Err(error) = self.cmd_tx.send(DspCommand::SetSubbands(Box::new(bands))) {
                tracing::warn!(%error, "capture stopped before subband update");
            }
            self.waker.wake();
        }
        self.meta.store(Arc::new(DspMeta {
            center_hz,
            sample_rate,
            dc_block,
        }));
    }

    fn spawn(
        &mut self,
        name: String,
        mut consumer: CaptureConsumer,
        commands: mpsc::Receiver<DspCommand>,
        analyzer: SpectrumAnalyzer,
        max_age: Duration,
    ) -> Result<(), DeviceError> {
        let shared = LaneShared {
            meta: self.meta.clone(),
            stop: self.stop.clone(),
            stalled_us: self.stalled_us.clone(),
            waker: self.waker.clone(),
            max_age,
        };
        let publisher = SpectrumPublisher::with_metrics(
            self.spectrum_tx.clone(),
            FFT_SIZE,
            self.spectrum_metrics.clone(),
        )
        .map_err(|error| DeviceError::Io(format!("start spectrum publisher: {error}")))?;
        let retirement = Reclaimer::new(super::worker::Retired::release)
            .map_err(|error| DeviceError::Io(format!("start retirement worker: {error}")))?;
        let handle = std::thread::Builder::new()
            .name(name)
            .spawn(move || {
                sdrmm_device::schedule::claim(sdrmm_device::Latency::Critical);
                shared.waker.adopt_current();
                dsp_loop(
                    &mut consumer,
                    &commands,
                    &shared,
                    analyzer,
                    publisher,
                    retirement,
                );
            })
            .map_err(|error| DeviceError::Io(format!("spawn dsp thread: {error}")))?;
        self.dsp = Some(handle);
        Ok(())
    }

    fn halt(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.waker.wake();
        if let Some(handle) = self.dsp.take()
            && handle.join().is_err()
        {
            tracing::error!("a dsp thread panicked");
        }
    }

    fn signal(&self) {
        self.stop.store(true, Ordering::Release);
    }
}

pub(crate) struct RetiredLane(Lane);

impl Drop for RetiredLane {
    fn drop(&mut self) {
        self.0.halt();
    }
}

pub(crate) struct VirtualLaneSink {
    #[cfg_attr(not(test), expect(dead_code))]
    stream: u32,
    producer: CaptureProducer,
    waker: Arc<Waker>,
    next_index: u64,
}

impl VirtualLaneSink {
    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) const fn stream(&self) -> u32 {
        self.stream
    }

    pub(crate) fn push(&mut self, samples: &[Complex<f32>]) {
        self.producer.push(samples, self.next_index);
        self.next_index += samples.len() as u64;
        self.waker.wake();
    }

    pub(crate) const fn skip(&mut self, samples: u64) {
        self.next_index += samples;
    }

    #[cfg(test)]
    pub(crate) const fn next_index(&self) -> u64 {
        self.next_index
    }

    #[cfg(test)]
    pub(crate) fn detached(stream: u32, capacity: usize) -> (Self, CaptureConsumer) {
        let (producer, consumer) = capture_ring(capacity);
        (
            Self {
                stream,
                producer,
                waker: Arc::new(Waker::default()),
                next_index: 0,
            },
            consumer,
        )
    }
}

pub struct CaptureRuntime {
    device: Option<Box<dyn SdrDevice>>,
    lanes: Vec<Lane>,
    virtual_lanes: BTreeMap<u32, Lane>,
    tap_ports: Vec<Arc<TapPort>>,
    mark_posters: Vec<MarkPoster>,
    per_stream: StreamScope,
    sweeping: bool,
    max_age: Duration,
    _awake: sdrmm_device::schedule::Awake,
}

struct LaneTail {
    consumer: CaptureConsumer,
    commands: mpsc::Receiver<DspCommand>,
    analyzer: SpectrumAnalyzer,
}

impl CaptureRuntime {
    pub(crate) fn queue_health(&self, device_set: u32) -> Vec<sdrmm_wire::PipelineQueue> {
        self.streams()
            .flat_map(|(stream, lane)| {
                [
                    (sdrmm_wire::PipelineStage::Capture, &lane.capture_metrics),
                    (sdrmm_wire::PipelineStage::Spectrum, &lane.spectrum_metrics),
                ]
                .map(|(stage, metrics)| sdrmm_wire::PipelineQueue {
                    device_set,
                    stream,
                    channel: None,
                    stage,
                    health: metrics.snapshot(),
                })
            })
            .collect()
    }

    fn streams(&self) -> impl Iterator<Item = (u32, &Lane)> {
        self.lanes
            .iter()
            .enumerate()
            .map(|(stream, lane)| (stream as u32, lane))
            .chain(
                self.virtual_lanes
                    .iter()
                    .map(|(stream, lane)| (*stream, lane)),
            )
    }

    pub fn start(
        device: Box<dyn SdrDevice>,
        settings: &DeviceSettings,
        dc_block: bool,
        on_fatal: impl FnOnce(DeviceError) + Send + 'static,
    ) -> Result<Self, DeviceError> {
        Self::start_with_taps(device, settings, dc_block, Vec::new(), on_fatal)
    }

    pub fn start_with_taps(
        mut device: Box<dyn SdrDevice>,
        settings: &DeviceSettings,
        dc_block: bool,
        taps: Vec<broadcast::Sender<SpectrumSnapshot>>,
        on_fatal: impl FnOnce(DeviceError) + Send + 'static,
    ) -> Result<Self, DeviceError> {
        let max_age = max_age_for(device.as_ref());
        let lane_count = device.capabilities().rx_streams.clamp(1, MAX_STREAMS) as usize;
        let per_stream = device.capabilities().per_stream;
        let Some(sample_rate) = settings.sample_rate else {
            return Err(DeviceError::Unsupported(
                "device did not report a sample rate; everything downstream is derived from it"
                    .to_string(),
            ));
        };
        let fatal: Arc<Mutex<Option<FatalReport>>> = Arc::new(Mutex::new(Some(Box::new(on_fatal))));
        let plan = SpectrumPlan::new(FFT_SIZE, lane_count);
        let mut sinks = Vec::with_capacity(lane_count);
        let mut runtime = Self {
            device: None,
            lanes: Vec::with_capacity(lane_count),
            virtual_lanes: BTreeMap::new(),
            tap_ports: Vec::with_capacity(lane_count),
            mark_posters: Vec::with_capacity(lane_count),
            per_stream,
            sweeping: false,
            max_age,
            _awake: sdrmm_device::schedule::stay_awake("a radio is streaming"),
        };
        let mut tails = Vec::with_capacity(lane_count);
        let ring = ring_capacity(sample_rate);
        for stream in 0..lane_count {
            let (producer, consumer) = capture_ring(ring);
            let (cmd_tx, cmd_rx) = mpsc::channel::<DspCommand>();
            let spectrum_tx = taps
                .get(stream)
                .cloned()
                .unwrap_or_else(|| broadcast::channel(SPECTRUM_TAP_SLOTS).0);
            let center_hz = settings
                .for_stream(stream as u32, &per_stream)
                .center_hz
                .unwrap_or(crate::DEFAULT_CENTER_HZ);
            let lane = Lane::new(
                DspMeta {
                    center_hz,
                    sample_rate,
                    dc_block,
                },
                spectrum_tx,
                cmd_tx,
                consumer.metrics.clone(),
            );
            let (port, writer) = TapPort::new(stream as u32);
            let sink = lane_sink(writer, producer, &lane, fatal.clone());
            runtime.mark_posters.push(sink.mark_poster());
            runtime.tap_ports.push(port);
            sinks.push(sink);
            runtime.lanes.push(lane);
            tails.push(LaneTail {
                consumer,
                commands: cmd_rx,
                analyzer: plan.analyzer(),
            });
        }
        for (index, tail) in tails.into_iter().enumerate() {
            let started = runtime.lanes[index].spawn(
                format!("sdrmm-dsp-{index}"),
                tail.consumer,
                tail.commands,
                tail.analyzer,
                max_age,
            );
            if let Err(error) = started {
                runtime.stop();
                return Err(error);
            }
        }
        device.rx_start(sinks)?;
        runtime.device = Some(device);
        Ok(runtime)
    }

    #[must_use]
    pub fn capabilities(&self) -> Option<sdrmm_wire::Capabilities> {
        self.device
            .as_ref()
            .map(|device| device.capabilities().clone())
    }

    pub fn subscribe(&self, stream: u32) -> Option<broadcast::Receiver<SpectrumSnapshot>> {
        self.lane(stream).map(|lane| lane.spectrum_tx.subscribe())
    }

    fn lane(&self, stream: u32) -> Option<&Lane> {
        self.lanes
            .get(stream as usize)
            .or_else(|| self.virtual_lanes.get(&stream))
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

    pub(crate) fn clip_meters(&self) -> Vec<Arc<ClipMeter>> {
        self.lanes.iter().map(|lane| lane.clip.clone()).collect()
    }

    pub(crate) fn stall_counters(&self) -> Vec<Arc<AtomicU64>> {
        self.lanes
            .iter()
            .map(|lane| lane.stalled_us.clone())
            .collect()
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn tap_ports(&self) -> Vec<Arc<TapPort>> {
        self.tap_ports.clone()
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn mark_posters(&self) -> Vec<MarkPoster> {
        self.mark_posters.clone()
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn add_virtual_lane(
        &mut self,
        stream: u32,
        center_hz: f64,
        sample_rate: f64,
        dc_block: bool,
    ) -> Result<(VirtualLaneSink, mpsc::Sender<DspCommand>), DeviceError> {
        if (stream as usize) < self.lanes.len() || self.virtual_lanes.contains_key(&stream) {
            return Err(DeviceError::Unsupported(format!(
                "stream {stream} is already in use"
            )));
        }
        if self.virtual_lanes.len() >= MAX_VIRTUAL_LANES as usize {
            return Err(DeviceError::Unsupported(format!(
                "at most {MAX_VIRTUAL_LANES} virtual lanes per radio"
            )));
        }
        if !(sample_rate.is_finite() && sample_rate > 0.0) {
            return Err(DeviceError::Unsupported(format!(
                "virtual lane rate {sample_rate} is not a rate"
            )));
        }
        let (producer, consumer) = capture_ring(ring_capacity(sample_rate));
        let (cmd_tx, cmd_rx) = mpsc::channel::<DspCommand>();
        let mut lane = Lane::new(
            DspMeta {
                center_hz,
                sample_rate,
                dc_block,
            },
            broadcast::channel(SPECTRUM_TAP_SLOTS).0,
            cmd_tx.clone(),
            consumer.metrics.clone(),
        );
        let analyzer = SpectrumPlan::new(FFT_SIZE, 1).analyzer();
        lane.spawn(
            format!("sdrmm-dsp-v{stream}"),
            consumer,
            cmd_rx,
            analyzer,
            self.max_age,
        )?;
        let sink = VirtualLaneSink {
            stream,
            producer,
            waker: lane.waker.clone(),
            next_index: 0,
        };
        self.virtual_lanes.insert(stream, lane);
        Ok((sink, cmd_tx))
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn set_virtual_meta(
        &mut self,
        stream: u32,
        center_hz: f64,
        sample_rate: f64,
    ) -> bool {
        match self.virtual_lanes.get(&stream) {
            Some(lane) => {
                let dc_block = lane.meta.load().dc_block;
                lane.set(center_hz, sample_rate, dc_block);
                true
            }
            None => false,
        }
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn remove_virtual_lane(&mut self, stream: u32) -> Option<RetiredLane> {
        let lane = self.virtual_lanes.remove(&stream)?;
        lane.signal();
        Some(RetiredLane(lane))
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn virtual_streams(&self) -> Vec<u32> {
        self.virtual_lanes.keys().copied().collect()
    }

    pub fn set_meta(&mut self, settings: &DeviceSettings, dc_block: bool) {
        let sample_rate = crate::sample_rate_of(settings);
        for (stream, lane) in self.lanes.iter().enumerate() {
            let center_hz = settings
                .for_stream(stream as u32, &self.per_stream)
                .center_hz
                .unwrap_or(crate::DEFAULT_CENTER_HZ);
            lane.set(center_hz, sample_rate, dc_block);
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

    pub fn agc_gains(&self) -> Result<Vec<sdrmm_wire::AgcGain>, DeviceError> {
        self.device
            .as_ref()
            .map_or_else(|| Ok(Vec::new()), |device| device.agc_gains())
    }

    pub fn stop(&mut self) {
        drop(self.halt());
    }

    pub fn release_device(&mut self) -> Option<Box<dyn SdrDevice>> {
        self.halt()
    }

    fn halt(&mut self) -> Option<Box<dyn SdrDevice>> {
        for lane in self.lanes.iter().chain(self.virtual_lanes.values()) {
            lane.signal();
        }
        let device = self.device.take().map(|mut device| {
            if self.sweeping {
                device.sweep_stop();
            } else {
                device.rx_stop();
            }
            device
        });
        for lane in self.lanes.iter_mut().chain(self.virtual_lanes.values_mut()) {
            lane.halt();
        }
        device
    }

    #[must_use]
    pub fn taps(&self) -> Vec<broadcast::Sender<SpectrumSnapshot>> {
        self.lanes
            .iter()
            .map(|lane| lane.spectrum_tx.clone())
            .collect()
    }

    #[must_use]
    pub const fn is_sweeping(&self) -> bool {
        self.sweeping
    }

    pub fn start_sweep(
        mut device: Box<dyn SdrDevice>,
        plan: &SweepPlan,
        offset_hz: f64,
        taps: Vec<broadcast::Sender<SpectrumSnapshot>>,
        on_fatal: impl FnOnce(DeviceError) + Send + 'static,
    ) -> Result<Self, (Box<dyn SdrDevice>, DeviceError)> {
        if let Err(e) = plan.check() {
            return Err((device, e));
        }
        let lane_count = device.capabilities().rx_streams.clamp(1, MAX_STREAMS) as usize;
        let per_stream = device.capabilities().per_stream;
        let mut taps = taps;
        taps.resize_with(lane_count, || broadcast::channel(SPECTRUM_TAP_SLOTS).0);
        let lanes: Vec<Lane> = taps
            .iter()
            .map(|spectrum_tx| {
                let (cmd_tx, _cmd_rx) = mpsc::channel::<DspCommand>();
                Lane::new(
                    DspMeta {
                        center_hz: crate::DEFAULT_CENTER_HZ,
                        sample_rate: plan.sample_rate_hz,
                        dc_block: false,
                    },
                    spectrum_tx.clone(),
                    cmd_tx,
                    Arc::new(crate::metrics::QueueMetrics::default()),
                )
            })
            .collect();
        let sink = match sweep_sink(taps[0].clone(), plan.sample_rate_hz, offset_hz, on_fatal) {
            Ok(sink) => sink,
            Err(error) => return Err((device, error)),
        };
        if let Err(e) = device.sweep_start(plan, sink) {
            return Err((device, e));
        }
        Ok(Self {
            device: Some(device),
            lanes,
            virtual_lanes: BTreeMap::new(),
            tap_ports: Vec::new(),
            mark_posters: Vec::new(),
            per_stream,
            sweeping: true,
            max_age: LIVE_MAX_AGE,
            _awake: sdrmm_device::schedule::stay_awake("a radio is sweeping"),
        })
    }
}

fn lane_sink(
    mut writer: TapWriter,
    mut producer: CaptureProducer,
    lane: &Lane,
    fatal: Arc<Mutex<Option<FatalReport>>>,
) -> RxSink {
    let meter = lane.clip.clone();
    let wake = lane.waker.clone();
    let room = producer.room();
    RxSink::with_items(
        move |item: SinkItem<'_>| match item {
            SinkItem::Samples { samples, index } => {
                writer.samples(samples, index);
                meter.measure(samples);
                producer.push(samples, index);
                wake.wake();
            }
            SinkItem::Event(event) => writer.event(event),
        },
        move |error| {
            if let Some(report) = fatal
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                report(error);
            }
        },
    )
    .with_room(room)
}

fn sweep_sink(
    tx: broadcast::Sender<SpectrumSnapshot>,
    sample_rate: f64,
    offset_hz: f64,
    on_fatal: impl FnOnce(DeviceError) + Send + 'static,
) -> Result<SweepSink, DeviceError> {
    let mut publisher = SpectrumPublisher::new(tx, FFT_SIZE)
        .map_err(|error| DeviceError::Io(format!("start sweep publisher: {error}")))?;
    let mut analyzer = CpuSpectrumAnalyzer::new(FFT_SIZE);
    let mut db = vec![0.0f32; FFT_SIZE];
    let mut seq = 0u32;
    let mut timestamp = 0u64;
    let mut short = 0u64;
    Ok(SweepSink::with_fatal_handler(
        move |center_hz, samples| {
            let Some(window) = samples.get(..FFT_SIZE) else {
                short += 1;
                tracing::warn!(
                    samples = samples.len(),
                    wanted = FFT_SIZE,
                    total = short,
                    "sweep block too short to transform; that tuning went unread"
                );
                return;
            };
            analyzer.power_db(window, &mut db);
            seq = seq.wrapping_add(1);
            timestamp += window.len() as u64;
            publisher.publish(
                seq,
                SpectrumFrame {
                    timestamp,
                    center_hz: center_hz + offset_hz,
                    span_hz: sample_rate as f32,
                },
                &db,
            );
        },
        on_fatal,
    ))
}

impl Drop for CaptureRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests;
