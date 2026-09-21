use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};

use num_complex::Complex;
use sdrmm_channels::monitor::{MonitorOutput, SpectrumMonitor};
use sdrmm_wire::{SpectrumMonitorNode, TransmissionState};

use crate::{
    DspCommand, Engine, EngineError,
    publishing::Publisher,
    runtime::{DspMeta, MAX_DSP_BLOCK},
    sample_rate_of,
};

static NEXT_MONITOR: AtomicU64 = AtomicU64::new(1);

type Output = Box<dyn FnMut(MonitorOutput) + Send>;

struct Packet {
    samples: Vec<Complex<f32>>,
    start: u64,
    meta: DspMeta,
    captured_at: std::time::SystemTime,
}

pub(crate) struct MonitorTap {
    publisher: Publisher<Packet>,
    dropped: Arc<AtomicU64>,
}

pub struct MonitorHandle {
    id: u64,
    commands: mpsc::Sender<DspCommand>,
    alive: Arc<AtomicBool>,
}

impl MonitorHandle {
    pub fn is_active(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }
}

impl Drop for MonitorHandle {
    fn drop(&mut self) {
        if self
            .commands
            .send(DspCommand::RemoveMonitor { id: self.id })
            .is_err()
        {
            tracing::debug!("monitor source stopped");
        }
    }
}

struct Worker {
    monitor: Option<SpectrumMonitor>,
    meta: Option<DspMeta>,
    settings: SpectrumMonitorNode,
    output: Output,
    dropped: Arc<AtomicU64>,
    id_base: u64,
    anchor: Option<(u64, std::time::SystemTime)>,
    alive: Arc<AtomicBool>,
}

impl Worker {
    fn deliver(&mut self, outputs: Vec<MonitorOutput>) {
        for mut output in outputs {
            output.transmission += self.id_base;
            if let sdrmm_wire::DecoderEvent::Transmission(event) = &mut output.event {
                event.id = output.transmission;
                event.started_at = self.sample_time(event.start_sample);
                event.ended_at = self.sample_time(event.end_sample);
            }
            (self.output)(output);
        }
    }

    fn sample_time(&self, sample: u64) -> Option<String> {
        let (origin, time) = self.anchor?;
        let rate = self.meta?.sample_rate;
        let seconds = sample.abs_diff(origin) as f64 / rate;
        let duration = std::time::Duration::try_from_secs_f64(seconds).ok()?;
        let time = if sample >= origin {
            time.checked_add(duration)
        } else {
            time.checked_sub(duration)
        }?;
        jiff::Timestamp::try_from(time)
            .ok()
            .map(|time| format!("{time:.9}"))
    }

    fn finish(&mut self, reason: &str) {
        if let Some(monitor) = &mut self.monitor {
            let outputs = monitor.finish(TransmissionState::Interrupted, Some(reason));
            self.deliver(outputs);
        }
    }

    fn process(&mut self, packet: &mut Packet) {
        if self.meta.is_none_or(|meta| {
            meta.center_hz != packet.meta.center_hz || meta.sample_rate != packet.meta.sample_rate
        }) {
            self.finish("source tuning changed");
            self.id_base = NEXT_MONITOR.fetch_add(1, Ordering::Relaxed) << 32;
            self.meta = Some(packet.meta);
            self.anchor = Some((packet.start, packet.captured_at));
            match SpectrumMonitor::new(
                packet.meta.sample_rate,
                packet.meta.center_hz,
                self.settings.clone(),
            ) {
                Ok(monitor) => self.monitor = Some(monitor),
                Err(error) => {
                    self.monitor = None;
                    self.problem(packet.start, error.to_string());
                }
            }
        }
        let dropped = self.dropped.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            self.problem(
                packet.start,
                format!("monitor queue lost {dropped} IQ samples"),
            );
        }
        if let Some(monitor) = &mut self.monitor {
            let outputs = monitor.process(&packet.samples, packet.start);
            self.deliver(outputs);
        }
        packet.samples.clear();
    }

    fn problem(&mut self, at: u64, error: String) {
        let meta = self.meta.unwrap_or(DspMeta {
            center_hz: 0.0,
            sample_rate: 1.0,
            dc_block: false,
        });
        (self.output)(MonitorOutput {
            transmission: self.id_base,
            frequency_hz: meta.center_hz,
            audio: Vec::new(),
            event: sdrmm_wire::DecoderEvent::Transmission(sdrmm_wire::Transmission {
                id: self.id_base,
                state: TransmissionState::Problem,
                signal: sdrmm_wire::IdentSignal::default(),
                start_sample: at,
                end_sample: at,
                sample_rate_hz: meta.sample_rate,
                duration_ms: 0,
                started_at: None,
                ended_at: None,
                decoder: None,
                decoder_confirmed: false,
                audio: None,
                error: Some(error),
            }),
        });
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        let dropped = self.dropped.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            self.problem(0, format!("monitor queue lost {dropped} IQ samples"));
        }
        self.finish("monitor disconnected");
    }
}

impl MonitorTap {
    fn new(
        rate: f64,
        settings: SpectrumMonitorNode,
        output: Output,
        alive: Arc<AtomicBool>,
    ) -> Result<Self, EngineError> {
        let dropped = Arc::new(AtomicU64::new(0));
        let mut worker = Worker {
            monitor: None,
            meta: None,
            settings,
            output,
            dropped: dropped.clone(),
            id_base: 0,
            anchor: None,
            alive,
        };
        let capacity = ((rate * 0.5 / MAX_DSP_BLOCK as f64).ceil() as usize).clamp(16, 1024);
        let publisher = Publisher::new(
            "sdrmm-monitor",
            capacity,
            || Packet {
                samples: Vec::with_capacity(MAX_DSP_BLOCK),
                start: 0,
                captured_at: std::time::SystemTime::UNIX_EPOCH,
                meta: DspMeta {
                    center_hz: 0.0,
                    sample_rate: rate,
                    dc_block: false,
                },
            },
            move |packet| worker.process(packet),
            || {},
        )
        .map_err(|error| EngineError::Monitor(error.to_string()))?;
        Ok(Self { publisher, dropped })
    }

    pub(crate) fn push(&mut self, samples: &[Complex<f32>], start: u64, meta: DspMeta) {
        for (index, chunk) in samples.chunks(MAX_DSP_BLOCK).enumerate() {
            if !self.publisher.submit(|packet| {
                packet.samples.clear();
                packet.samples.extend_from_slice(chunk);
                packet.start = start + (index * MAX_DSP_BLOCK) as u64;
                packet.meta = meta;
                packet.captured_at = std::time::SystemTime::now();
            }) {
                self.dropped
                    .fetch_add(chunk.len() as u64, Ordering::Relaxed);
            }
        }
    }
}

impl Engine {
    pub fn monitor(
        &self,
        device_set: u32,
        stream: u32,
        settings: SpectrumMonitorNode,
        output: impl FnMut(MonitorOutput) + Send + 'static,
    ) -> Result<MonitorHandle, EngineError> {
        let (commands, rate) = {
            let inner = self.lock();
            let state = inner
                .device_sets
                .get(&device_set)
                .ok_or(EngineError::DeviceSetNotFound(device_set))?;
            state.check_stream(stream)?;
            (
                state.cmd_txs[stream as usize].clone(),
                sample_rate_of(&state.settings),
            )
        };
        let alive = Arc::new(AtomicBool::new(true));
        let tap = MonitorTap::new(rate, settings, Box::new(output), alive.clone())?;
        let id = NEXT_MONITOR.fetch_add(1, Ordering::Relaxed);
        commands
            .send(DspCommand::AddMonitor {
                id,
                tap: Box::new(tap),
            })
            .map_err(|_| EngineError::Monitor("source stopped".to_owned()))?;
        Ok(MonitorHandle {
            id,
            commands,
            alive,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use sdrmm_test_support::assert_no_alloc;
    use sdrmm_wire::{DecoderEvent, TransmissionState};

    use super::*;

    #[test]
    fn monitor_capture_is_allocation_free_and_shutdown_flushes_transmissions() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let received = seen.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let mut tap = MonitorTap::new(
            48_000.0,
            SpectrumMonitorNode::default(),
            Box::new(move |event| received.lock().unwrap().push(event)),
            alive.clone(),
        )
        .unwrap();
        let samples: Vec<_> = (0..4800)
            .map(|i| {
                Complex::from_polar(
                    0.5,
                    (std::f64::consts::TAU * 10_000.0 * i as f64 / 48_000.0)
                        .rem_euclid(std::f64::consts::TAU) as f32,
                )
            })
            .collect();
        let meta = DspMeta {
            center_hz: 145_000_000.0,
            sample_rate: 48_000.0,
            dc_block: false,
        };
        assert_no_alloc("monitor capture", || tap.push(&samples, 0, meta));
        tap.publisher.flush();
        drop(tap);
        assert!(!alive.load(Ordering::Acquire));
        let seen = seen.lock().unwrap();
        assert!(seen.iter().any(|out| matches!(&out.event, DecoderEvent::Transmission(t) if t.state == TransmissionState::Started && t.started_at.is_some())));
        assert!(seen.iter().any(|out| matches!(&out.event, DecoderEvent::Transmission(t) if t.state == TransmissionState::Interrupted)));
    }

    #[test]
    fn retuning_interrupts_old_transmissions_and_uses_new_ids() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let received = seen.clone();
        let mut tap = MonitorTap::new(
            48_000.0,
            SpectrumMonitorNode::default(),
            Box::new(move |event| received.lock().unwrap().push(event)),
            Arc::new(AtomicBool::new(true)),
        )
        .unwrap();
        let samples: Vec<_> = (0..4800)
            .map(|i| {
                Complex::from_polar(
                    0.5,
                    (std::f64::consts::TAU * 10_000.0 * i as f64 / 48_000.0)
                        .rem_euclid(std::f64::consts::TAU) as f32,
                )
            })
            .collect();
        for index in 0..2 {
            tap.push(
                &samples,
                index * 4800,
                DspMeta {
                    center_hz: 145_000_000.0 + index as f64 * 1_000_000.0,
                    sample_rate: 48_000.0,
                    dc_block: false,
                },
            );
            tap.publisher.flush();
        }
        drop(tap);
        let seen = seen.lock().unwrap();
        let starts: Vec<_> = seen
            .iter()
            .filter_map(|out| match &out.event {
                DecoderEvent::Transmission(t) if t.state == TransmissionState::Started => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(starts.len(), 2);
        assert_ne!(starts[0].id, starts[1].id);
        assert!(
            (starts[1].signal.frequency_hz - starts[0].signal.frequency_hz - 1_000_000.0).abs()
                < 1.0
        );
    }

    #[test]
    fn queue_loss_is_reported_even_when_the_source_stops() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let received = seen.clone();
        let tap = MonitorTap::new(
            48_000.0,
            SpectrumMonitorNode::default(),
            Box::new(move |event| received.lock().unwrap().push(event)),
            Arc::new(AtomicBool::new(true)),
        )
        .unwrap();
        tap.dropped.store(8192, Ordering::Relaxed);
        drop(tap);
        assert!(seen.lock().unwrap().iter().any(|out| matches!(&out.event, DecoderEvent::Transmission(t) if t.state == TransmissionState::Problem && t.error.as_deref().is_some_and(|error| error.contains("8192")))));
    }
}
