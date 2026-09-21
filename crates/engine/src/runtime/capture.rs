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
use sdrmm_device::{DeviceError, RxSink, SdrDevice, SweepPlan, SweepSink};
use sdrmm_dsp::SpectrumAnalyzer as CpuSpectrumAnalyzer;
use sdrmm_wire::{Coherence, DeviceSettings, MAX_STREAMS, StreamScope};
use tokio::sync::broadcast;

use super::{
    DspCommand, DspMeta, FFT_SIZE, SpectrumSnapshot, Waker,
    retire::Reclaimer,
    worker::{LaneShared, dsp_loop},
};
use crate::{
    capture_ring::{CaptureConsumer, capture_ring},
    coherent::CoherentTaps,
    publishing::spectrum::SpectrumPublisher,
    spectrum::{SpectrumAnalyzer, SpectrumFrame, SpectrumPlan},
};

pub(crate) const RING_SECONDS: f64 = 0.1;
const LIVE_MAX_AGE: Duration = Duration::from_millis(100);
const RING_MIN: usize = 1 << 17;
const RING_MAX: usize = 1 << 23;

pub(crate) fn ring_capacity(sample_rate: f64) -> usize {
    ((sample_rate * RING_SECONDS) as usize).clamp(RING_MIN, RING_MAX)
}

/// A radio streams whether or not anything is listening, so capture that the DSP could not reach
/// in time is stale and skipping it is what keeps the picture live. A recording has no live edge
/// to stay near: it waits instead, and every sample it holds is still the one that was asked for.
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
    waker: Arc<Waker>,
    stop: Arc<AtomicBool>,
    dsp: Option<JoinHandle<()>>,
    capture_metrics: Arc<crate::metrics::QueueMetrics>,
    spectrum_metrics: Arc<crate::metrics::QueueMetrics>,
}

pub struct CaptureRuntime {
    device: Option<Box<dyn SdrDevice>>,
    lanes: Vec<Lane>,
    per_stream: StreamScope,
    sweeping: bool,
    coherent: Option<CoherentTaps>,
    _awake: sdrmm_device::schedule::Awake,
}

impl CaptureRuntime {
    pub(crate) fn queue_health(&self, device_set: u32) -> Vec<sdrmm_wire::PipelineQueue> {
        self.lanes
            .iter()
            .enumerate()
            .flat_map(|(stream, lane)| {
                [
                    (sdrmm_wire::PipelineStage::Capture, &lane.capture_metrics),
                    (sdrmm_wire::PipelineStage::Spectrum, &lane.spectrum_metrics),
                ]
                .map(|(stage, metrics)| sdrmm_wire::PipelineQueue {
                    device_set,
                    stream: stream as u32,
                    channel: None,
                    stage,
                    health: metrics.snapshot(),
                })
            })
            .collect()
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
        let (mut lane_taps, mut coherent) = match device.capabilities().coherence {
            Coherence::None => (Vec::new(), None),
            _ if lane_count < 2 => (Vec::new(), None),
            _ => {
                let (taps, shared) = crate::coherent::lane_taps(lane_count, sample_rate);
                (taps, Some(shared))
            }
        };
        lane_taps.reverse();
        let total_lanes = lane_count + usize::from(coherent.is_some());
        let spectrum_plan = SpectrumPlan::new(FFT_SIZE, total_lanes);

        let mut sinks: Vec<RxSink> = Vec::with_capacity(lane_count);
        let mut lanes: Vec<Lane> = Vec::with_capacity(total_lanes);
        let mut tails: Vec<(
            CaptureConsumer,
            mpsc::Receiver<DspCommand>,
            SpectrumAnalyzer,
        )> = Vec::with_capacity(lane_count);
        let ring = ring_capacity(sample_rate);
        for stream in 0..total_lanes {
            let (mut producer, consumer) = capture_ring(ring);
            let overruns = consumer.metrics.dropped_counter();
            let stalled_us = Arc::new(AtomicU64::new(0));
            let waker = Arc::new(Waker::default());
            let wake = waker.clone();
            let fatal = fatal.clone();
            let mut lane_tap = lane_taps.pop();
            if stream >= lane_count {
                if let Some(taps) = coherent.as_mut() {
                    taps.beam = Some(crate::coherent::BeamSink {
                        producer,
                        waker: waker.clone(),
                    });
                }
            } else {
                let room = producer.room();
                sinks.push(
                    RxSink::with_fatal_handler(
                        move |samples: &[Complex<f32>], index: u64| {
                            if let Some(tap) = lane_tap.as_mut() {
                                tap.push(samples, index);
                            }
                            producer.push(samples, index);
                            wake.wake();
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
                    )
                    .with_room(room),
                );
            }
            let spectrum_tx = taps
                .get(stream)
                .cloned()
                .unwrap_or_else(|| broadcast::channel::<SpectrumSnapshot>(8).0);
            let (cmd_tx, cmd_rx) = mpsc::channel::<DspCommand>();
            let center_hz = settings
                .for_stream(stream as u32, &per_stream)
                .center_hz
                .unwrap_or(crate::DEFAULT_CENTER_HZ);
            lanes.push(Lane {
                meta: Arc::new(ArcSwap::from_pointee(DspMeta {
                    center_hz,
                    sample_rate,
                    dc_block,
                })),
                spectrum_tx,
                cmd_tx,
                overruns,
                stalled_us,
                waker,
                stop: Arc::new(AtomicBool::new(false)),
                dsp: None,
                capture_metrics: consumer.metrics.clone(),
                spectrum_metrics: Arc::new(crate::metrics::QueueMetrics::default()),
            });
            tails.push((consumer, cmd_rx, spectrum_plan.analyzer()));
        }

        let mut runtime = Self {
            device: None,
            lanes,
            per_stream,
            sweeping: false,
            coherent,
            _awake: sdrmm_device::schedule::stay_awake("a radio is streaming"),
        };

        for (index, (mut consumer, cmd_rx, analyzer)) in tails.into_iter().enumerate() {
            let lane = &runtime.lanes[index];
            let shared = LaneShared {
                meta: lane.meta.clone(),
                stop: lane.stop.clone(),
                stalled_us: lane.stalled_us.clone(),
                waker: lane.waker.clone(),
                max_age,
            };
            let publisher = SpectrumPublisher::with_metrics(
                lane.spectrum_tx.clone(),
                FFT_SIZE,
                lane.spectrum_metrics.clone(),
            )
            .map_err(|error| DeviceError::Io(format!("start spectrum publisher: {error}")))?;
            let retirement = Reclaimer::new(super::worker::Retired::release)
                .map_err(|error| DeviceError::Io(format!("start retirement worker: {error}")))?;
            let spawned = std::thread::Builder::new()
                .name(format!("sdrmm-dsp-{index}"))
                .spawn(move || {
                    sdrmm_device::schedule::claim(sdrmm_device::Latency::Critical);
                    shared.waker.adopt_current();
                    dsp_loop(
                        &mut consumer,
                        &cmd_rx,
                        &shared,
                        analyzer,
                        publisher,
                        retirement,
                    );
                });
            match spawned {
                Ok(handle) => runtime.lanes[index].dsp = Some(handle),
                Err(e) => {
                    runtime.stop();
                    return Err(DeviceError::Io(format!("spawn dsp thread: {e}")));
                }
            }
        }
        device.rx_start(sinks)?;
        runtime.device = Some(device);
        Ok(runtime)
    }

    pub(crate) fn take_coherent(&mut self) -> Option<CoherentTaps> {
        self.coherent.take()
    }

    pub(crate) fn return_coherent(&mut self, taps: Option<CoherentTaps>) {
        if taps.is_some() {
            self.coherent = taps;
        }
    }

    #[must_use]
    pub fn is_coherent(&self) -> bool {
        self.coherent.is_some()
    }

    #[must_use]
    pub fn capabilities(&self) -> Option<sdrmm_wire::Capabilities> {
        self.device
            .as_ref()
            .map(|device| device.capabilities().clone())
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

    pub(crate) fn stall_counters(&self) -> Vec<Arc<AtomicU64>> {
        self.lanes
            .iter()
            .map(|lane| lane.stalled_us.clone())
            .collect()
    }

    pub fn set_meta(&mut self, settings: &DeviceSettings, dc_block: bool) {
        let sample_rate = crate::sample_rate_of(settings);
        if let Some(taps) = &mut self.coherent {
            taps.sample_rate = sample_rate;
        }
        for (stream, lane) in self.lanes.iter().enumerate() {
            if lane.meta.load().sample_rate != sample_rate {
                let bands = super::subbands::Subbands::new(sample_rate);
                if let Err(error) = lane.cmd_tx.send(DspCommand::SetSubbands(Box::new(bands))) {
                    tracing::warn!(%error, "capture stopped before subband update");
                }
                lane.waker.wake();
            }
            let center_hz = settings
                .for_stream(stream as u32, &self.per_stream)
                .center_hz
                .unwrap_or(crate::DEFAULT_CENTER_HZ);
            lane.meta.store(Arc::new(DspMeta {
                center_hz,
                sample_rate,
                dc_block,
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

    pub fn set_noise_source(&mut self, on: bool) -> Result<(), DeviceError> {
        self.device
            .as_mut()
            .ok_or_else(|| DeviceError::Io("the device has been stopped".to_string()))?
            .set_noise_source(on)
    }

    pub fn stop(&mut self) {
        drop(self.halt());
    }

    pub fn release_device(&mut self) -> Option<Box<dyn SdrDevice>> {
        self.halt()
    }

    fn halt(&mut self) -> Option<Box<dyn SdrDevice>> {
        for lane in &self.lanes {
            lane.stop.store(true, Ordering::Release);
        }
        let device = self.device.take().map(|mut device| {
            if self.sweeping {
                device.sweep_stop();
            } else {
                device.rx_stop();
            }
            device
        });
        for lane in &mut self.lanes {
            lane.waker.wake();
            if let Some(handle) = lane.dsp.take() {
                let _ = handle.join();
            }
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
        taps.resize_with(lane_count, || broadcast::channel::<SpectrumSnapshot>(8).0);
        let lanes: Vec<Lane> = taps
            .iter()
            .map(|spectrum_tx| {
                let (cmd_tx, _cmd_rx) = mpsc::channel::<DspCommand>();
                Lane {
                    meta: Arc::new(ArcSwap::from_pointee(DspMeta {
                        center_hz: crate::DEFAULT_CENTER_HZ,
                        sample_rate: plan.sample_rate_hz,
                        dc_block: false,
                    })),
                    spectrum_tx: spectrum_tx.clone(),
                    cmd_tx,
                    overruns: Arc::new(AtomicU64::new(0)),
                    stalled_us: Arc::new(AtomicU64::new(0)),
                    waker: Arc::new(Waker::default()),
                    stop: Arc::new(AtomicBool::new(false)),
                    dsp: None,
                    capture_metrics: Arc::new(crate::metrics::QueueMetrics::default()),
                    spectrum_metrics: Arc::new(crate::metrics::QueueMetrics::default()),
                }
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
            per_stream,
            sweeping: true,
            coherent: None,
            _awake: sdrmm_device::schedule::stay_awake("a radio is sweeping"),
        })
    }
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
mod tests {
    use super::*;

    #[test]
    fn every_rate_keeps_the_same_slack_in_seconds() {
        for rate in [2_048_000.0, 2_400_000.0, 8_000_000.0, 20_000_000.0] {
            let seconds = ring_capacity(rate) as f64 / rate;
            assert!(
                seconds >= RING_SECONDS,
                "{rate} S/s left only {seconds} s of slack"
            );
        }
    }

    #[test]
    fn a_fast_radio_is_capped_and_a_slow_one_floored() {
        assert_eq!(ring_capacity(1_000_000_000.0), RING_MAX);
        assert_eq!(ring_capacity(48_000.0), RING_MIN);
        assert_eq!(ring_capacity(2_400_000.0), 240_000);
    }

    #[test]
    fn the_floor_holds_two_of_the_largest_blocks_a_driver_pushes_at_once() {
        let block = sdrmm_device::capture::CaptureConfig::new("ring", "ring").block_samples;
        assert!(RING_MIN >= 2 * block.max(super::super::DSP_BLOCK));
    }

    #[test]
    fn a_recording_is_held_for_the_dsp_while_a_radio_is_skipped_past() {
        let dir = tempfile::TempDir::new().expect("scratch");
        let stem = dir.path().join("aged");
        let mut writer = sdrmm_recorder::SigmfWriter::create(&stem, 48_000.0, 100e6, "age policy")
            .expect("open");
        writer
            .write_block(&[Complex::new(0.0f32, 0.0); 48_000])
            .expect("write");
        writer.finalize().expect("finalize");

        let recordings =
            sdrmm_device_recording::RecordingDriver::new(Some(dir.path().to_path_buf()));
        let synthetic = sdrmm_device_virtual::VirtualDriver::new();
        let open = |driver: &dyn sdrmm_device::DeviceDriver, key: &str| {
            let info = sdrmm_device::DeviceDriver::probe(driver)
                .into_iter()
                .find(|info| info.key.ends_with(key))
                .expect("probed");
            sdrmm_device::DeviceDriver::open(driver, &info).expect("open")
        };
        assert_eq!(
            max_age_for(open(&recordings, "aged").as_ref()),
            Duration::MAX
        );
        assert_eq!(
            max_age_for(open(&synthetic, "siggen").as_ref()),
            LIVE_MAX_AGE
        );
    }

    #[test]
    fn a_rate_that_is_not_a_number_still_sizes_a_ring() {
        assert_eq!(ring_capacity(f64::NAN), RING_MIN);
        assert_eq!(ring_capacity(-1.0), RING_MIN);
        assert_eq!(ring_capacity(f64::INFINITY), RING_MAX);
    }
}
