use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;
use num_complex::Complex;
use sdrmm_device::RxSink;

use super::{
    ChannelHost, DSP_BLOCK, DspCommand, DspMeta, FFT_SIZE, frontend::Frontend, retire::Reclaimer,
};
use crate::{
    capture_ring::CaptureConsumer,
    network_export::NetworkExportTap,
    publishing::{recording::RecordingPublisher, spectrum::SpectrumPublisher},
    recording::RecorderTap,
    spectrum::{SpectrumAnalyzer, SpectrumFrame},
    time_machine::TimeMachineTap,
};

const TARGET_FPS: f64 = 30.0;
const IDLE_PARK: Duration = Duration::from_millis(20);

#[derive(Default)]
pub(crate) struct Waker(OnceLock<std::thread::Thread>);

impl Waker {
    pub(crate) fn adopt_current(&self) {
        let _ = self.0.set(std::thread::current());
    }

    pub(crate) fn wake(&self) {
        if let Some(thread) = self.0.get() {
            thread.unpark();
        }
    }
}

pub(super) struct LaneShared {
    pub(super) meta: Arc<ArcSwap<DspMeta>>,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) stalled_us: Arc<AtomicU64>,
    pub(super) waker: Arc<Waker>,
}

struct ArrayOutput {
    id: u32,
    sink: RxSink,
    next: Option<u64>,
}

impl ArrayOutput {
    fn push(&mut self, samples: &[Complex<f32>], index: u64) {
        if let Some(next) = self.next {
            self.sink.dropped(index.saturating_sub(next));
        }
        self.next = Some(index + samples.len() as u64);
        self.sink.push(samples);
    }
}

pub(super) enum Retired {
    Channel(Box<ChannelHost>),
    Recording(Option<RecorderTap>, Option<RecordingPublisher>),
    Network(NetworkExportTap),
    History(TimeMachineTap),
}

impl Retired {
    pub(super) fn release(self) {
        match self {
            Self::Channel(host) => drop(host),
            Self::Recording(tap, publisher) => drop((tap, publisher)),
            Self::Network(tap) => drop(tap),
            Self::History(tap) => drop(tap),
        }
    }
}

pub(super) fn dsp_loop(
    consumer: &mut CaptureConsumer,
    commands: &mpsc::Receiver<DspCommand>,
    lane: &LaneShared,
    mut analyzer: SpectrumAnalyzer,
    mut publisher: SpectrumPublisher,
    mut retirement: Reclaimer<Retired>,
) {
    let LaneShared {
        meta,
        stop,
        stalled_us,
        ..
    } = lane;
    let mut hist = vec![Complex::new(0.0, 0.0); FFT_SIZE];
    let mut window = vec![Complex::new(0.0, 0.0); FFT_SIZE];
    let mut db = vec![0.0f32; FFT_SIZE];
    let mut channels: Vec<(u32, Box<ChannelHost>)> = Vec::new();
    let mut arrays: Vec<ArrayOutput> = Vec::new();
    let mut tap: Option<RecorderTap> = None;
    let mut recording_publisher: Option<RecordingPublisher> = None;
    let mut network_tap: Option<NetworkExportTap> = None;
    let mut history: Option<TimeMachineTap> = None;
    let mut write_pos = 0usize;
    let mut since_last = 0usize;
    let mut seq: u32 = 0;
    let mut next_input = None;
    let mut served = Instant::now();
    let mut frontend = Frontend::new(*meta.load_full());

    while !stop.load(Ordering::Acquire) {
        drain_commands(
            commands,
            &mut channels,
            &mut arrays,
            CommandSinks {
                tap: &mut tap,
                network_tap: &mut network_tap,
                history: &mut history,
                recording_publisher: &mut recording_publisher,
            },
            &mut retirement,
        );
        let snapshot = *meta.load_full();
        let hop = ((snapshot.sample_rate / TARGET_FPS) as usize).max(FFT_SIZE / 4);
        frontend.follow(snapshot);
        let consumed =
            consumer.consume_fresh(DSP_BLOCK, Duration::from_millis(100), |raw, mut total| {
                record_stall(stalled_us, &mut served);
                if next_input.is_some_and(|next| next != total) {
                    frontend.reset();
                    hist.fill(Complex::new(0.0, 0.0));
                    write_pos = 0;
                    since_last = 0;
                }
                next_input = Some(total + raw.len() as u64);
                let slice = frontend.apply(raw);
                for array in &mut arrays {
                    array.push(slice, total);
                }
                if tap.as_ref().is_some_and(|t| {
                    recording_publisher.as_mut().is_none_or(|publisher| {
                        !publisher.publish(t, slice, total, snapshot.center_hz)
                    })
                }) {
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
                    host.process_at(slice, total, snapshot.center_hz, snapshot.lo_offset_hz);
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
                            lo_hz: snapshot.lo_hz(),
                        };
                        if let Some(completed) = analyzer.power_db(&window, &mut db, frame) {
                            seq = seq.wrapping_add(1);
                            publisher.publish(seq, completed, &db);
                        }
                    }
                }
            });
        if consumed == 0 {
            std::thread::park_timeout(IDLE_PARK);
        }
    }
}

fn record_stall(stalled_us: &AtomicU64, served: &mut Instant) {
    let now = Instant::now();
    let gap = u64::try_from(now.duration_since(*served).as_micros()).unwrap_or(u64::MAX);
    *served = now;
    stalled_us.fetch_max(gap, Ordering::Relaxed);
}

struct CommandSinks<'a> {
    tap: &'a mut Option<RecorderTap>,
    network_tap: &'a mut Option<NetworkExportTap>,
    history: &'a mut Option<TimeMachineTap>,
    recording_publisher: &'a mut Option<RecordingPublisher>,
}

fn drain_commands(
    commands: &mpsc::Receiver<DspCommand>,
    channels: &mut Vec<(u32, Box<ChannelHost>)>,
    arrays: &mut Vec<ArrayOutput>,
    sinks: CommandSinks<'_>,
    retirement: &mut Reclaimer<Retired>,
) {
    let CommandSinks {
        tap,
        network_tap,
        history,
        recording_publisher,
    } = sinks;
    for _ in 0..64 {
        if !retirement.available() {
            break;
        }
        let Ok(cmd) = commands.try_recv() else {
            break;
        };
        match cmd {
            DspCommand::ConnectArray { id, sink } => {
                arrays.retain(|array| array.id != id);
                arrays.push(ArrayOutput {
                    id,
                    sink,
                    next: None,
                });
            }
            DspCommand::DisconnectArray { id } => arrays.retain(|array| array.id != id),
            DspCommand::AddChannel { id, mut host } => {
                if let Some((_, previous)) =
                    channels.iter_mut().find(|(existing, _)| *existing == id)
                {
                    host.inherit(previous);
                    std::mem::swap(previous, &mut host);
                    retirement.retire(Retired::Channel(host));
                } else {
                    channels.push((id, host));
                }
            }
            DspCommand::RemoveChannel { id } => {
                if let Some(index) = channels.iter().position(|(existing, _)| *existing == id) {
                    retirement.retire(Retired::Channel(channels.remove(index).1));
                }
            }
            DspCommand::PositionChanged { id, fix } => {
                if let Some((_, host)) = channels.iter_mut().find(|(existing, _)| *existing == id) {
                    host.position_changed(fix.as_ref());
                } else {
                    tracing::debug!(id, "position for a channel no longer hosted");
                }
            }
            DspCommand::StartRecording {
                tap: armed,
                publisher,
            } => {
                retirement.retire(Retired::Recording(
                    tap.replace(armed),
                    recording_publisher.replace(publisher),
                ));
            }
            DspCommand::StopRecording => {
                retirement.retire(Retired::Recording(tap.take(), recording_publisher.take()));
            }
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
            DspCommand::StartNetworkExport { tap: armed } => {
                if let Some(old) = network_tap.replace(armed) {
                    retirement.retire(Retired::Network(old));
                }
            }
            DspCommand::StopNetworkExport => {
                if let Some(old) = network_tap.take() {
                    retirement.retire(Retired::Network(old));
                }
            }
            DspCommand::StartTimeMachine { tap: armed } => {
                if let Some(old) = history.replace(*armed) {
                    retirement.retire(Retired::History(old));
                }
            }
            DspCommand::StopTimeMachine => {
                if let Some(old) = history.take() {
                    retirement.retire(Retired::History(old));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::broadcast;

    use super::*;
    use crate::{capture_ring::capture_ring, spectrum::SpectrumPlan};

    #[test]
    fn a_waker_with_no_thread_yet_is_a_no_op() {
        Waker::default().wake();
    }

    #[test]
    fn waking_releases_a_parked_thread_far_sooner_than_the_backstop() {
        let waker = Arc::new(Waker::default());
        let ready = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let parked = {
            let (waker, ready) = (waker.clone(), ready.clone());
            std::thread::spawn(move || {
                waker.adopt_current();
                ready.store(true, Ordering::Release);
                let start = Instant::now();
                std::thread::park_timeout(Duration::from_secs(30));
                let _ = tx.send(start.elapsed());
            })
        };
        while !ready.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        waker.wake();
        let waited = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the parked thread was never released");
        parked.join().expect("thread panicked");
        assert!(
            waited < Duration::from_secs(5),
            "wake did not beat the backstop: {waited:?}"
        );
    }

    #[test]
    fn the_stall_counter_keeps_the_worst_gap_not_the_last() {
        let stalled = AtomicU64::new(0);
        let mut served = Instant::now() - Duration::from_millis(400);
        record_stall(&stalled, &mut served);
        let worst = stalled.load(Ordering::Relaxed);
        assert!(worst >= 400_000, "expected the 400 ms gap, got {worst} us");

        record_stall(&stalled, &mut served);
        assert_eq!(
            stalled.load(Ordering::Relaxed),
            worst,
            "a short gap must not erase the worst one"
        );
    }

    #[test]
    fn array_forwarding_keeps_buffered_samples_before_a_later_overflow_gap() {
        let (mut producer, mut consumer) = capture_ring(8);
        let (commands, command_rx) = mpsc::channel();
        let (received, output) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let waker = Arc::new(Waker::default());
        let shared = LaneShared {
            meta: Arc::new(ArcSwap::from_pointee(DspMeta {
                center_hz: 100e6,
                sample_rate: 48_000.0,
                lo_offset_hz: 0.0,
                dc_block: false,
            })),
            stop: stop.clone(),
            stalled_us: Arc::new(AtomicU64::new(0)),
            waker: waker.clone(),
        };
        let mut first = true;
        commands
            .send(DspCommand::ConnectArray {
                id: 1,
                sink: RxSink::new(move |samples, index| {
                    if samples.is_empty() {
                        return;
                    }
                    received
                        .send((index, samples.to_vec()))
                        .expect("receive array block");
                    if first {
                        first = false;
                        blocked
                            .recv_timeout(Duration::from_secs(5))
                            .expect("release DSP");
                    }
                }),
            })
            .expect("connect array");
        assert_eq!(producer.push(&[Complex::new(0.0, 0.0); 2], 0), 2);
        let (spectrum, _) = broadcast::channel(8);
        let publisher = SpectrumPublisher::new(spectrum, FFT_SIZE).expect("publisher");
        let worker = std::thread::spawn(move || {
            shared.waker.adopt_current();
            dsp_loop(
                &mut consumer,
                &command_rx,
                &shared,
                SpectrumPlan::new(FFT_SIZE, 1).analyzer(),
                publisher,
                Reclaimer::new(Retired::release).expect("retirement"),
            );
        });
        let initial = output
            .recv_timeout(Duration::from_secs(5))
            .expect("DSP is blocked");
        let input: Vec<_> = (2..12)
            .map(|index| Complex::new(index as f32, 0.0))
            .collect();
        let count = producer.push(&input, 2);
        release.send(()).expect("resume DSP");
        waker.wake();
        let buffered = output
            .recv_timeout(Duration::from_secs(5))
            .expect("buffered array samples");
        let following = [Complex::new(12.0, 0.0), Complex::new(13.0, 0.0)];
        assert_eq!(producer.push(&following, 12), following.len());
        waker.wake();
        let after_gap = output
            .recv_timeout(Duration::from_secs(5))
            .expect("samples after the gap");
        stop.store(true, Ordering::Release);
        waker.wake();
        worker.join().expect("DSP exits");
        assert_eq!(initial.0, 0);
        assert_eq!(count, 6);
        assert_eq!(buffered.1, input[..count]);
        assert_eq!(
            buffered.0, 2,
            "the dropped tail must not shift the buffered prefix"
        );
        assert_eq!(after_gap, (12, following.to_vec()));
    }
}
