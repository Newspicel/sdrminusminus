//! Per-device-set runtime (PLAN §7): the capture thread pushes IQ into an SPSC ring; a DSP
//! thread drains it, runs the spectrum tap, and broadcasts snapshots. No locks or allocation
//! on the DSP hot path — settings arrive via an [`ArcSwap`] snapshot, output leaves via a
//! broadcast channel.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

use arc_swap::ArcSwap;
use num_complex::Complex;
use rtrb::RingBuffer;
use sdrmm_device::{RxSink, SdrDevice};
use sdrmm_dsp::SpectrumAnalyzer;
use tokio::sync::broadcast;

/// FFT size for the spectrum tap (PLAN §9: 1k–64k configurable; M0 fixes one size).
const FFT_SIZE: usize = 4096;
/// Internal spectrum cadence; per-client fps throttling happens downstream (PLAN §9).
const TARGET_FPS: f64 = 30.0;
/// Ring depth in samples (~0.5 s at 2.4 Msps) — absorbs scheduling jitter before overrun.
const RING_CAPACITY: usize = 1 << 20;
/// Dynamic range below the per-frame peak used for the adaptive dB window default.
const DEFAULT_DB_RANGE: f32 = 80.0;

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

/// Owns the running device and its DSP thread; drop/stop tears both down cleanly.
pub struct CaptureRuntime {
    device: Box<dyn SdrDevice>,
    meta: Arc<ArcSwap<DspMeta>>,
    spectrum_tx: broadcast::Sender<SpectrumSnapshot>,
    overruns: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    dsp: Option<JoinHandle<()>>,
}

impl CaptureRuntime {
    /// Wire the device to a fresh ring + DSP thread and start streaming.
    pub fn start(
        mut device: Box<dyn SdrDevice>,
        center_hz: f64,
        sample_rate: f64,
    ) -> Result<Self, sdrmm_device::DeviceError> {
        let (mut producer, mut consumer) = RingBuffer::<Complex<f32>>::new(RING_CAPACITY);
        let overruns = Arc::new(AtomicU64::new(0));
        let ov = overruns.clone();

        // Capture sink: lock-free write into the ring; dropped samples are counted, never
        // silently lost (PLAN §5 backpressure, CLAUDE.md no-silent-failure).
        let sink = RxSink::new(move |samples: &[Complex<f32>]| {
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
        });

        let meta = Arc::new(ArcSwap::from_pointee(DspMeta {
            center_hz,
            sample_rate,
        }));
        let (spectrum_tx, _) = broadcast::channel::<SpectrumSnapshot>(8);
        let stop = Arc::new(AtomicBool::new(false));

        device.rx_start(sink)?;

        let dsp = {
            let meta = meta.clone();
            let tx = spectrum_tx.clone();
            let stop = stop.clone();
            let overruns = overruns.clone();
            std::thread::Builder::new()
                .name("sdrmm-dsp".to_string())
                .spawn(move || dsp_loop(&mut consumer, &meta, &tx, &stop, &overruns))
                .map_err(|e| sdrmm_device::DeviceError::Io(format!("spawn dsp thread: {e}")))?
        };

        Ok(Self {
            device,
            meta,
            spectrum_tx,
            overruns,
            stop,
            dsp: Some(dsp),
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SpectrumSnapshot> {
        self.spectrum_tx.subscribe()
    }

    pub fn set_meta(&self, center_hz: f64, sample_rate: f64) {
        self.meta.store(Arc::new(DspMeta {
            center_hz,
            sample_rate,
        }));
    }

    pub fn apply(
        &mut self,
        settings: &sdrmm_wire::DeviceSettings,
    ) -> Result<(), sdrmm_device::DeviceError> {
        self.device.apply(settings)
    }

    pub fn overruns(&self) -> u64 {
        self.overruns.load(Ordering::Relaxed)
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

/// The DSP thread body (PLAN §7): drain ring → maintain a rolling FFT window → emit a spectrum
/// every `hop` samples. `hop` derives from the live sample rate, so cadence stays ~`TARGET_FPS`
/// regardless of tuning.
fn dsp_loop(
    consumer: &mut rtrb::Consumer<Complex<f32>>,
    meta: &ArcSwap<DspMeta>,
    tx: &broadcast::Sender<SpectrumSnapshot>,
    stop: &AtomicBool,
    _overruns: &AtomicU64,
) {
    let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE);
    let mut hist = vec![Complex::new(0.0, 0.0); FFT_SIZE];
    let mut window = vec![Complex::new(0.0, 0.0); FFT_SIZE];
    let mut db = vec![0.0f32; FFT_SIZE];
    let mut write_pos = 0usize;
    let mut since_last = 0usize;
    let mut total: u64 = 0;
    let mut seq: u32 = 0;

    while !stop.load(Ordering::Acquire) {
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

/// The default adaptive dB window for a snapshot: `[peak - DEFAULT_DB_RANGE, peak]` (PLAN §9).
#[must_use]
pub fn adaptive_db_window(db: &[f32]) -> (f32, f32) {
    let peak = db.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let peak = if peak.is_finite() { peak } else { 0.0 };
    (peak - DEFAULT_DB_RANGE, peak)
}
