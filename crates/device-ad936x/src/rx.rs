use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use sdrmm_device::{
    Block, BlockPool, CaptureRadio, CaptureStream, DeviceError, FatalHandle, GapScope, LaneEvent,
    LaneMark, Next, RxSink, Sample, SinkItem, StreamFailure, UNKNOWN_ERROR, Uncertainty, lock,
    net::Read,
};

use crate::{
    iio::{
        Link, Stopper, close_buffer, mask, mask_len, open_buffer, parse_answer, read_buf,
        remaining, set_remote_timeout,
    },
    layout::Stream,
    source::Source,
};

const REFILL_TIMEOUT: Duration = Duration::from_secs(4);

const REMOTE_TIMEOUT: Duration = Duration::from_secs(2);

const BUFFER_SPAN: Duration = Duration::from_millis(20);

const MIN_BUFFER_SAMPLES: usize = 4_096;
const MAX_BUFFER_SAMPLES: usize = 1 << 19;

const ALIGN: usize = 8;

pub(crate) fn buffer_samples(rate: f64, sample_bytes: usize) -> usize {
    let wanted = if rate.is_finite() && rate > 0.0 {
        (rate * BUFFER_SPAN.as_secs_f64()) as usize
    } else {
        MIN_BUFFER_SAMPLES
    };
    let per_sample = sample_bytes.max(1);
    let step = (ALIGN / gcd(ALIGN, per_sample)).max(1);
    let clamped = wanted.clamp(MIN_BUFFER_SAMPLES, MAX_BUFFER_SAMPLES);
    clamped.div_ceil(step) * step
}

pub(crate) fn in_flight_samples(rate: f64, sample_bytes: usize) -> u64 {
    2 * buffer_samples(rate, sample_bytes) as u64
}

const fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

pub(crate) struct RxRadio {
    source: Source,
    stream: Stream,
    lanes: usize,
    samples: usize,
    pool: BlockPool,
    armed: Mutex<Option<Stopper>>,
}

impl std::fmt::Debug for RxRadio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RxRadio")
            .field("device", &self.stream.device)
            .field("lanes", &self.lanes)
            .finish_non_exhaustive()
    }
}

impl RxRadio {
    pub(crate) fn new(source: Source, stream: Stream, lanes: usize, rate: Option<f64>) -> Self {
        let samples = buffer_samples(rate.unwrap_or(0.0), stream.sample_bytes(lanes));
        Self {
            source,
            stream,
            lanes,
            samples,
            pool: BlockPool::default(),
            armed: Mutex::new(None),
        }
    }

    pub(crate) const fn buffer_samples(&self) -> usize {
        self.samples
    }
}

impl CaptureRadio for RxRadio {
    type Stream = RxStream;

    fn arm(&self) -> Result<RxStream, DeviceError> {
        let mut link = Link::new(self.source.open()?);
        let stopper = link.stopper();
        set_remote_timeout(&mut link, REMOTE_TIMEOUT)?;
        let elements = self.stream.elements(self.lanes);
        let mask = mask(&elements, self.stream.scan_total);
        open_buffer(&mut link, &self.stream.device, self.samples, &mask)?;
        *lock(&self.armed) = Some(stopper.clone());
        tracing::debug!(
            device = self.stream.device,
            lanes = self.lanes,
            samples = self.samples,
            "ad936x receive buffer opened"
        );
        let refill = self.samples * self.stream.sample_bytes(self.lanes);
        Ok(RxStream {
            inner: Mutex::new(Inner {
                link,
                pending: None,
            }),
            device: self.stream.device.clone(),
            command: read_buf(&self.stream.device, refill),
            refill,
            mask_bytes: mask_len(mask.len()),
            pool: self.pool.clone(),
            stopper,
        })
    }

    fn disarm(&self) {
        if let Some(stopper) = lock(&self.armed).take() {
            sdrmm_device::StopHandle::stop(&stopper);
        }
    }
}

pub(crate) struct RxStream {
    inner: Mutex<Inner>,
    device: String,
    command: String,
    refill: usize,
    mask_bytes: usize,
    pool: BlockPool,
    stopper: Stopper,
}

struct Inner {
    link: Link,
    pending: Option<Refill>,
}

impl std::fmt::Debug for RxStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RxStream")
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}

struct Refill {
    block: Block,
    got: usize,
    piece: usize,
    masked: bool,
    stage: Stage,
    started: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Count,
    Mask { left: usize },
    Data { left: usize },
    Done,
}

enum Progress {
    Made,
    Waiting,
}

impl Refill {
    fn new(block: Block) -> Self {
        Self {
            block,
            got: 0,
            piece: 0,
            masked: false,
            stage: Stage::Count,
            started: Instant::now(),
        }
    }

    fn finished(&self, room: usize) -> bool {
        self.stage == Stage::Done || self.got >= room
    }

    fn step(
        &mut self,
        link: &mut Link,
        mask_bytes: usize,
        room: usize,
        timeout: Duration,
    ) -> Result<Progress, DeviceError> {
        match self.stage {
            Stage::Count => self.count(link, mask_bytes, room, timeout),
            Stage::Mask { left } => {
                let mut sink = [0u8; 64];
                let want = left.min(sink.len());
                let n = match link.take(&mut sink[..want], timeout) {
                    Read::Got(n) => n,
                    Read::Idle => return Ok(Progress::Waiting),
                    Read::Ended => return Err(link.ended()),
                };
                self.stage = if n == left {
                    Stage::Data { left: self.piece }
                } else {
                    Stage::Mask { left: left - n }
                };
                Ok(Progress::Made)
            }
            Stage::Data { left } => {
                let at = self.got;
                let n = match link.take(&mut self.block.bytes_mut()[at..at + left], timeout) {
                    Read::Got(n) => n,
                    Read::Idle => return Ok(Progress::Waiting),
                    Read::Ended => return Err(link.ended()),
                };
                self.got += n;
                self.stage = if n == left {
                    Stage::Count
                } else {
                    Stage::Data { left: left - n }
                };
                Ok(Progress::Made)
            }
            Stage::Done => Ok(Progress::Made),
        }
    }

    fn count(
        &mut self,
        link: &mut Link,
        mask_bytes: usize,
        room: usize,
        timeout: Duration,
    ) -> Result<Progress, DeviceError> {
        let Some(line) = link.poll_line(timeout)? else {
            return Ok(Progress::Waiting);
        };
        let piece = parse_answer(&line, "refill the sample buffer")?;
        if piece > room - self.got {
            return Err(DeviceError::Io(format!(
                "the radio sent {piece} bytes into a buffer with room for {}",
                room - self.got
            )));
        }
        self.piece = piece;
        self.stage = if piece == 0 {
            Stage::Done
        } else if self.masked {
            Stage::Data { left: piece }
        } else {
            self.masked = true;
            Stage::Mask { left: mask_bytes }
        };
        Ok(Progress::Made)
    }
}

impl RxStream {
    fn advance(
        &self,
        link: &mut Link,
        pending: &mut Option<Refill>,
        timeout: Duration,
    ) -> Result<Option<Block>, DeviceError> {
        let refill = match pending {
            Some(refill) => refill,
            None => {
                link.send(&self.command)?;
                pending.insert(Refill::new(self.pool.take(self.refill)))
            }
        };
        let deadline = Instant::now() + timeout;
        while !refill.finished(self.refill) {
            if refill.started.elapsed() > REFILL_TIMEOUT {
                return Err(DeviceError::Io(format!(
                    "the radio did not fill its buffer within {REFILL_TIMEOUT:?}"
                )));
            }
            if let Progress::Waiting =
                refill.step(link, self.mask_bytes, self.refill, remaining(deadline))?
            {
                return Ok(None);
            }
        }
        let Some(done) = pending.take() else {
            return Ok(None);
        };
        let mut block = done.block;
        block.truncate(done.got);
        Ok(Some(block))
    }
}

impl CaptureStream for RxStream {
    type Block = Block;
    type Stop = Stopper;

    fn stop_handle(&self) -> Stopper {
        self.stopper.clone()
    }

    fn next_block(&self, timeout: Duration) -> Next<Block> {
        let mut inner = lock(&self.inner);
        if self.stopper.is_stopped() {
            return Next::Ended;
        }
        let Inner { link, pending } = &mut *inner;
        match self.advance(link, pending, timeout) {
            Ok(Some(block)) if block.is_empty() => Next::Idle,
            Ok(Some(block)) => Next::Block(block),
            Ok(None) => Next::Idle,
            Err(e) => {
                link.transport().fail(e.to_string());
                Next::Ended
            }
        }
    }

    fn dropped(&self) -> u64 {
        0
    }

    fn failure(&self) -> StreamFailure {
        lock(&self.inner).link.failure()
    }
}

impl Drop for RxStream {
    fn drop(&mut self) {
        let inner = &mut *lock(&self.inner);
        inner.pending = None;
        close_buffer(&mut inner.link, &self.device);
        inner.link.close();
    }
}

struct FanOut {
    sinks: Vec<RxSink>,
    lane_buffers: Vec<Vec<Sample>>,
    lane_samples: usize,
    expected: u64,
}

impl FanOut {
    fn new(sinks: Vec<RxSink>, lane_samples: usize) -> Self {
        let lane_samples = lane_samples.max(1);
        Self {
            lane_buffers: sinks
                .iter()
                .map(|_| Vec::with_capacity(lane_samples))
                .collect(),
            sinks,
            lane_samples,
            expected: 0,
        }
    }

    fn lanes(&self) -> u64 {
        self.sinks.len().max(1) as u64
    }

    fn item(&mut self, item: SinkItem<'_>) {
        match item {
            SinkItem::Samples { samples, index } => self.samples(samples, index),
            SinkItem::Event(event) => self.event(event),
        }
    }

    fn samples(&mut self, samples: &[Sample], index: u64) {
        if index > self.expected {
            self.gap(index - self.expected);
        }
        self.expected = index + samples.len() as u64;
        let lanes = self.sinks.len();
        let mut phase = (index % self.lanes()) as usize;
        for chunk in samples.chunks(self.lane_samples * lanes) {
            for lane in &mut self.lane_buffers {
                lane.clear();
            }
            for (slot, sample) in chunk.iter().enumerate() {
                self.lane_buffers[(phase + slot) % lanes].push(*sample);
            }
            phase = (phase + chunk.len()) % lanes;
            for (sink, lane) in self.sinks.iter_mut().zip(&self.lane_buffers) {
                sink.push(lane);
            }
        }
    }

    fn gap(&mut self, gap: u64) {
        let lanes = self.lanes();
        if gap.is_multiple_of(lanes) {
            for sink in &mut self.sinks {
                sink.dropped(gap / lanes);
            }
            return;
        }
        for sink in &mut self.sinks {
            sink.dropped_estimate(gap.div_ceil(lanes), 1, GapScope::Device);
            sink.realigned(Uncertainty::Unaligned, UNKNOWN_ERROR, GapScope::Device);
        }
    }

    fn per_lane(&self, samples: u64) -> u64 {
        if samples == UNKNOWN_ERROR {
            UNKNOWN_ERROR
        } else {
            samples.div_ceil(self.lanes())
        }
    }

    fn event(&mut self, event: LaneEvent) {
        match event {
            LaneEvent::Uncertain {
                at,
                error,
                scope,
                cause: Uncertainty::EstimatedGap,
            } => {
                let lost = self.per_lane(at.saturating_sub(self.expected));
                let error = self.per_lane(error);
                self.expected = self.expected.max(at);
                for sink in &mut self.sinks {
                    sink.dropped_estimate(lost, error, scope);
                }
            }
            LaneEvent::Uncertain {
                error,
                scope,
                cause,
                ..
            } => {
                let error = self.per_lane(error);
                for sink in &mut self.sinks {
                    sink.realigned(cause, error, scope);
                }
            }
            LaneEvent::Mark { mark, .. } => {
                let mark = self.lane_mark(mark);
                for sink in &mut self.sinks {
                    sink.mark(mark);
                }
            }
            LaneEvent::HardwareTime { ns, .. } => {
                for sink in &mut self.sinks {
                    sink.stamp_hardware(ns);
                }
            }
        }
    }

    fn lane_mark(&self, mark: LaneMark) -> LaneMark {
        match mark {
            LaneMark::NoiseSource { on, in_flight } => LaneMark::NoiseSource {
                on,
                in_flight: self.per_lane(in_flight),
            },
            LaneMark::Retuned { in_flight } => LaneMark::Retuned {
                in_flight: self.per_lane(in_flight),
            },
            LaneMark::GainChanged { in_flight } => LaneMark::GainChanged {
                in_flight: self.per_lane(in_flight),
            },
        }
    }
}

pub(crate) fn fan_out(sinks: Vec<RxSink>, lane_samples: usize) -> RxSink {
    if sinks.len() <= 1 {
        return sinks
            .into_iter()
            .next()
            .unwrap_or_else(|| RxSink::new(|_, _| {}));
    }
    let mut sinks = sinks;
    let failures: Vec<FatalHandle> = sinks.iter_mut().map(RxSink::share_failure).collect();
    let mut fan = FanOut::new(sinks, lane_samples);
    RxSink::with_items(
        move |item| fan.item(item),
        move |error| {
            for failure in &failures {
                failure.fail(error.clone());
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, mpsc};

    use sdrmm_device::SinkItem;

    use super::*;
    use crate::iio::testing::Scripted;

    const POLL: Duration = Duration::from_millis(20);

    fn stream(transport: &Arc<Scripted>, refill: usize) -> RxStream {
        RxStream {
            inner: Mutex::new(Inner {
                link: Link::new(transport.clone()),
                pending: None,
            }),
            device: "cf-ad9361-lpc".to_string(),
            command: read_buf("cf-ad9361-lpc", refill),
            refill,
            mask_bytes: mask_len(1),
            pool: BlockPool::default(),
            stopper: Stopper::flag(),
        }
    }

    #[test]
    fn a_refill_is_gathered_across_polls_and_a_stop_is_felt_between_them() {
        let transport = Scripted::with(&[b"8\n", b"00000003\n", b"abcd"]);
        let stream = stream(&transport, 8);
        assert!(
            matches!(stream.next_block(POLL), Next::Idle),
            "half a piece is not a block"
        );
        assert_eq!(
            &*lock(&transport.sent),
            b"READBUF cf-ad9361-lpc 8\r\n",
            "one request for the whole refill"
        );
        transport.feed(&[b"efgh"]);
        let Next::Block(block) = stream.next_block(POLL) else {
            panic!("the rest of the piece completes the block");
        };
        assert_eq!(&*block, b"abcdefgh");
        assert!(transport.failed().is_none(), "waiting is not a fault");
    }

    #[test]
    fn a_refill_in_pieces_reads_the_mask_once_and_ends_at_an_empty_piece() {
        let transport = Scripted::with(&[b"4\n00000003\nabcd", b"2\nef", b"0\n"]);
        let stream = stream(&transport, 16);
        let Next::Block(block) = stream.next_block(POLL) else {
            panic!("an empty piece ends the refill");
        };
        assert_eq!(&*block, b"abcdef");
    }

    #[test]
    fn a_piece_larger_than_the_room_left_ends_the_stream_rather_than_the_buffer() {
        let transport = Scripted::with(&[b"64\n"]);
        let stream = stream(&transport, 8);
        assert!(matches!(stream.next_block(POLL), Next::Ended));
        assert!(
            stream.failure().reason.contains("room for 8"),
            "{}",
            stream.failure().reason
        );
    }

    #[test]
    fn a_refused_refill_carries_the_radios_reason() {
        let transport = Scripted::with(&[b"-5\n"]);
        let stream = stream(&transport, 8);
        assert!(matches!(stream.next_block(POLL), Next::Ended));
        assert!(
            stream.failure().reason.contains("input/output error"),
            "{}",
            stream.failure().reason
        );
    }

    #[test]
    fn a_buffer_holds_about_a_frame_of_signal_whatever_the_rate() {
        let at = |rate: f64| buffer_samples(rate, 4);
        assert_eq!(at(0.0), MIN_BUFFER_SAMPLES, "an untuned radio still opens");
        assert_eq!(at(1_000.0), MIN_BUFFER_SAMPLES, "a slow rate has a floor");
        assert!((at(2_400_000.0) as f64 - 48_000.0).abs() < 8.0);
        assert_eq!(at(1e12), MAX_BUFFER_SAMPLES, "and a ceiling");
    }

    #[test]
    fn a_buffer_length_is_a_whole_number_of_aligned_bytes() {
        for sample_bytes in [2, 4, 6, 8, 16] {
            for rate in [0.0, 2.4e6, 61.44e6] {
                let samples = buffer_samples(rate, sample_bytes);
                assert_eq!(
                    samples * sample_bytes % ALIGN,
                    0,
                    "{samples} samples of {sample_bytes} bytes"
                );
            }
        }
    }

    fn recording() -> (RxSink, mpsc::Receiver<(u64, Vec<f32>)>) {
        let (tx, rx) = mpsc::channel();
        (
            RxSink::new(move |samples: &[Sample], index| {
                let _ = tx.send((index, samples.iter().map(|s| s.re).collect()));
            }),
            rx,
        )
    }

    #[test]
    fn one_sink_is_handed_straight_through() {
        let (sink, seen) = recording();
        let mut sink = fan_out(vec![sink], 4);
        sink.push(&[Sample::new(1.0, 0.0), Sample::new(2.0, 0.0)]);
        assert_eq!(seen.try_recv().expect("pushed"), (0, vec![1.0, 2.0]));
    }

    #[test]
    fn two_lanes_are_split_apart_and_each_keeps_its_own_count() {
        let (first, left) = recording();
        let (second, right) = recording();
        let mut sink = fan_out(vec![first, second], 4);
        let block: Vec<Sample> = (1..=6).map(|n| Sample::new(n as f32, 0.0)).collect();
        sink.push(&block);
        sink.push(&block);
        assert_eq!(left.try_recv().expect("lane 0"), (0, vec![1.0, 3.0, 5.0]));
        assert_eq!(right.try_recv().expect("lane 1"), (0, vec![2.0, 4.0, 6.0]));
        assert_eq!(left.try_recv().expect("lane 0"), (3, vec![1.0, 3.0, 5.0]));
        assert_eq!(right.try_recv().expect("lane 1"), (3, vec![2.0, 4.0, 6.0]));
    }

    #[test]
    fn a_block_that_ends_between_lanes_keeps_the_next_one_on_its_lanes() {
        let (first, left) = recording();
        let (second, right) = recording();
        let mut sink = fan_out(vec![first, second], 4);
        let samples: Vec<Sample> = (1..=6).map(|n| Sample::new(n as f32, 0.0)).collect();
        sink.push(&samples[..3]);
        sink.push(&samples[3..]);
        let lane = |seen: &mpsc::Receiver<(u64, Vec<f32>)>| {
            seen.try_iter()
                .flat_map(|(_, values)| values)
                .collect::<Vec<f32>>()
        };
        assert_eq!(lane(&left), [1.0, 3.0, 5.0]);
        assert_eq!(lane(&right), [2.0, 4.0, 6.0]);
    }

    #[test]
    fn a_gap_the_supervisor_reports_moves_every_lane_by_its_own_share() {
        let (first, left) = recording();
        let (second, right) = recording();
        let mut sink = fan_out(vec![first, second], 4);
        let block = [Sample::new(1.0, 0.0), Sample::new(2.0, 0.0)];
        sink.push(&block);
        sink.dropped(100);
        sink.push(&block);
        assert_eq!(left.try_recv().expect("lane 0").0, 0);
        assert_eq!(right.try_recv().expect("lane 1").0, 0);
        assert_eq!(
            left.try_recv().expect("lane 0").0,
            51,
            "one sample delivered plus half the interleaved gap"
        );
        assert_eq!(right.try_recv().expect("lane 1").0, 51);
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Seen {
        Samples { index: u64, len: usize },
        Event(LaneEvent),
    }

    fn items() -> (RxSink, Arc<Mutex<Vec<Seen>>>) {
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let log = seen.clone();
        let sink = RxSink::with_items(
            move |item: SinkItem<'_>| {
                lock(&log).push(match item {
                    SinkItem::Samples { samples, index } => Seen::Samples {
                        index,
                        len: samples.len(),
                    },
                    SinkItem::Event(event) => Seen::Event(event),
                });
            },
            |_| {},
        );
        (sink, seen)
    }

    #[test]
    fn two_buffers_of_one_lane_are_in_flight() {
        assert_eq!(in_flight_samples(10e6, 8), 400_000);
        assert_eq!(
            in_flight_samples(0.0, 4),
            2 * MIN_BUFFER_SAMPLES as u64,
            "an unknown rate still holds the smallest buffer twice"
        );
        assert_eq!(in_flight_samples(61.44e6, 4), 2 * MAX_BUFFER_SAMPLES as u64);
    }

    #[test]
    fn a_gap_not_divisible_by_lanes_is_uncertain() {
        let (first, left) = items();
        let (second, right) = items();
        let mut sink = fan_out(vec![first, second], 4);
        let block = [Sample::new(1.0, 0.0), Sample::new(2.0, 0.0)];
        sink.push(&block);
        sink.dropped(101);
        sink.push(&block);
        for lane in [left, right] {
            assert_eq!(
                *lock(&lane),
                vec![
                    Seen::Samples { index: 0, len: 1 },
                    Seen::Event(LaneEvent::Uncertain {
                        at: 52,
                        error: 1,
                        scope: GapScope::Device,
                        cause: Uncertainty::EstimatedGap,
                    }),
                    Seen::Event(LaneEvent::Uncertain {
                        at: 52,
                        error: UNKNOWN_ERROR,
                        scope: GapScope::Device,
                        cause: Uncertainty::Unaligned,
                    }),
                    Seen::Samples { index: 52, len: 1 },
                ]
            );
        }
    }

    #[test]
    fn an_estimated_gap_from_the_supervisor_stays_an_estimate_on_every_lane() {
        let (first, left) = items();
        let (second, right) = items();
        let mut sink = fan_out(vec![first, second], 4);
        let block = [Sample::new(1.0, 0.0), Sample::new(2.0, 0.0)];
        sink.push(&block);
        sink.dropped_estimate(100, 100, GapScope::Lane);
        sink.realigned(Uncertainty::Rearmed, UNKNOWN_ERROR, GapScope::Lane);
        sink.push(&block);
        for lane in [left, right] {
            assert_eq!(
                *lock(&lane),
                vec![
                    Seen::Samples { index: 0, len: 1 },
                    Seen::Event(LaneEvent::Uncertain {
                        at: 51,
                        error: 50,
                        scope: GapScope::Lane,
                        cause: Uncertainty::EstimatedGap,
                    }),
                    Seen::Event(LaneEvent::Uncertain {
                        at: 51,
                        error: UNKNOWN_ERROR,
                        scope: GapScope::Lane,
                        cause: Uncertainty::Rearmed,
                    }),
                    Seen::Samples { index: 51, len: 1 },
                ],
                "the lanes must not count the estimate a second time as an exact gap"
            );
        }
    }

    #[test]
    fn fan_out_never_grows_its_buffers() {
        let (first, left) = items();
        let (second, right) = items();
        let mut fan = FanOut::new(vec![first, second], 4);
        let before: Vec<(*const Sample, usize)> = fan
            .lane_buffers
            .iter()
            .map(|lane| (lane.as_ptr(), lane.capacity()))
            .collect();
        let block: Vec<Sample> = (0..20).map(|n| Sample::new(n as f32, 0.0)).collect();
        fan.samples(&block, 0);
        let after: Vec<(*const Sample, usize)> = fan
            .lane_buffers
            .iter()
            .map(|lane| (lane.as_ptr(), lane.capacity()))
            .collect();
        assert_eq!(before, after);
        for lane in [left, right] {
            assert_eq!(
                *lock(&lane),
                vec![
                    Seen::Samples { index: 0, len: 4 },
                    Seen::Samples { index: 4, len: 4 },
                    Seen::Samples { index: 8, len: 2 },
                ]
            );
        }
    }

    #[test]
    fn a_fault_reaches_every_lane_with_the_kind_it_had() {
        let (faults, seen) = mpsc::channel();
        let sinks: Vec<RxSink> = (0..3)
            .map(|lane| {
                let faults = faults.clone();
                RxSink::with_fatal_handler(
                    |_, _| {},
                    move |error| {
                        let _ = faults.send((lane, error.to_string()));
                    },
                )
            })
            .collect();
        let mut sink = fan_out(sinks, 4);
        sink.fail(DeviceError::Disconnected("unplugged".to_string()));
        let mut told: Vec<usize> = seen
            .try_iter()
            .map(|(lane, why)| {
                assert!(why.contains("no longer attached"), "{why}");
                lane
            })
            .collect();
        told.sort_unstable();
        assert_eq!(told, vec![0, 1, 2]);
    }

    #[test]
    fn no_sinks_at_all_still_yields_something_that_can_be_pushed_to() {
        let mut sink = fan_out(Vec::new(), 4);
        sink.push(&[Sample::new(1.0, 0.0)]);
    }
}
