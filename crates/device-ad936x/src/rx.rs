use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use sdrmm_device::{
    Block, BlockPool, CaptureRadio, CaptureStream, DeviceError, FatalHandle, Next, RxSink, Sample,
    StreamFailure, lock, net::Read,
};

use crate::{
    iio::{
        Link, Stopper, close_buffer, mask, mask_len, open_buffer, parse_answer, read_buf,
        remaining, set_remote_timeout,
    },
    layout::Stream,
    source::Source,
};

/// How long one refill may take before the conversation is treated as broken. Generous against
/// the buffer's own span so a radio that is merely busy is not restarted under it.
const REFILL_TIMEOUT: Duration = Duration::from_secs(4);

/// What the radio is told to wait for its own converter, so a stalled buffer comes back as a
/// refusal rather than leaving the refill parked.
const REMOTE_TIMEOUT: Duration = Duration::from_secs(2);

/// Roughly how much signal one buffer holds. Short enough that a retune is felt straight away,
/// long enough that a megasample-per-second stream is not a round trip per millisecond.
const BUFFER_SPAN: Duration = Duration::from_millis(20);

const MIN_BUFFER_SAMPLES: usize = 4_096;
const MAX_BUFFER_SAMPLES: usize = 1 << 19;

/// The buffer length the kernel side aligns to, in bytes.
const ALIGN: usize = 8;

/// How many samples of one lane a buffer holds at this rate.
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

const fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// The receive buffer of one radio, opened afresh whenever the capture is armed.
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

/// One open receive buffer, refilled a block at a time.
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

/// One buffer on its way in.
///
/// IIOD answers a refill as a run of length-prefixed pieces, the first of which is followed by
/// the mask of what is enabled. The pieces are gathered across as many polls as they take, so a
/// stop is felt between any two reads rather than after the whole buffer.
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
    /// Carries the refill in flight as far as `timeout` allows, starting one if none is.
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
        // The buffer is closed even when the conversation was already stopped: over usb the
        // command still reaches the radio, and a buffer left open refuses the next one.
        let inner = &mut *lock(&self.inner);
        inner.pending = None;
        close_buffer(&mut inner.link, &self.device);
        inner.link.close();
    }
}

/// Splits the lanes of one interleaved buffer across the sinks that asked for them.
///
/// The capture path carries one sink, so a radio whose lanes share a buffer hands over a sink
/// that de-interleaves. A gap the supervisor reports as a jump in the sample index is divided
/// back out per lane, so every lane stays on the same timeline as its neighbours.
pub(crate) fn fan_out(sinks: Vec<RxSink>) -> RxSink {
    let lanes = sinks.len();
    if lanes <= 1 {
        return sinks
            .into_iter()
            .next()
            .unwrap_or_else(|| RxSink::new(|_, _| {}));
    }
    let mut sinks = sinks;
    let failures: Vec<FatalHandle> = sinks.iter_mut().map(RxSink::share_failure).collect();
    let mut lane_buffers: Vec<Vec<Sample>> = vec![Vec::new(); lanes];
    let mut expected: Option<u64> = None;
    RxSink::with_fatal_handler(
        move |samples, index| {
            if let Some(expected) = expected.filter(|expected| index > *expected) {
                let lost = (index - expected) / lanes as u64;
                for sink in &mut sinks {
                    sink.dropped(lost);
                }
            }
            expected = Some(index + samples.len() as u64);
            for lane in &mut lane_buffers {
                lane.clear();
            }
            for (slot, sample) in samples.iter().enumerate() {
                lane_buffers[slot % lanes].push(*sample);
            }
            for (sink, lane) in sinks.iter_mut().zip(&lane_buffers) {
                sink.push(lane);
            }
        },
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
        let mut sink = fan_out(vec![sink]);
        sink.push(&[Sample::new(1.0, 0.0), Sample::new(2.0, 0.0)]);
        assert_eq!(seen.try_recv().expect("pushed"), (0, vec![1.0, 2.0]));
    }

    #[test]
    fn two_lanes_are_split_apart_and_each_keeps_its_own_count() {
        let (first, left) = recording();
        let (second, right) = recording();
        let mut sink = fan_out(vec![first, second]);
        let block: Vec<Sample> = (1..=6).map(|n| Sample::new(n as f32, 0.0)).collect();
        sink.push(&block);
        sink.push(&block);
        assert_eq!(left.try_recv().expect("lane 0"), (0, vec![1.0, 3.0, 5.0]));
        assert_eq!(right.try_recv().expect("lane 1"), (0, vec![2.0, 4.0, 6.0]));
        assert_eq!(left.try_recv().expect("lane 0"), (3, vec![1.0, 3.0, 5.0]));
        assert_eq!(right.try_recv().expect("lane 1"), (3, vec![2.0, 4.0, 6.0]));
    }

    #[test]
    fn a_gap_the_supervisor_reports_moves_every_lane_by_its_own_share() {
        let (first, left) = recording();
        let (second, right) = recording();
        let mut sink = fan_out(vec![first, second]);
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
        let mut sink = fan_out(sinks);
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
        let mut sink = fan_out(Vec::new());
        sink.push(&[Sample::new(1.0, 0.0)]);
    }
}
