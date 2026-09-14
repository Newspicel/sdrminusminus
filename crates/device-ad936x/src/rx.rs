use std::{sync::Mutex, time::Duration};

use sdrmm_device::{
    Block, BlockPool, CaptureRadio, CaptureStream, DeviceError, FatalHandle, Next, RxSink, Sample,
    StreamFailure, lock,
};

use crate::{
    iio::{
        Link, Response, Stopper, close_buffer, mask, mask_len, open_buffer, read_buf,
        set_remote_timeout,
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
        Ok(RxStream {
            inner: Mutex::new(link),
            device: self.stream.device.clone(),
            refill: self.samples * self.stream.sample_bytes(self.lanes),
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
    inner: Mutex<Link>,
    device: String,
    refill: usize,
    mask_bytes: usize,
    pool: BlockPool,
    stopper: Stopper,
}

impl std::fmt::Debug for RxStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RxStream")
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}

impl RxStream {
    /// Asks for one buffer and takes it.
    ///
    /// IIOD answers a refill as a run of length-prefixed pieces, the first of which is followed
    /// by the mask of what is enabled, so the pieces are gathered until the buffer is full or the
    /// radio says it has no more.
    fn refill(&self, link: &mut Link) -> Result<Block, DeviceError> {
        link.send(&read_buf(&self.device, self.refill))?;
        let mut block = self.pool.take(self.refill);
        let mut got = 0;
        let mut masked = false;
        while got < self.refill {
            let line = link.read_line(REFILL_TIMEOUT)?;
            let piece = Response::parse(&line)
                .ok_or_else(|| {
                    DeviceError::Io(format!("refill the sample buffer: iiod answered {line:?}"))
                })?
                .bytes("refill the sample buffer")?;
            if piece == 0 {
                break;
            }
            if !masked {
                let mut enabled = vec![0u8; self.mask_bytes];
                link.read_exact(&mut enabled, REFILL_TIMEOUT)?;
                masked = true;
            }
            if piece > self.refill - got {
                link.discard(piece, REFILL_TIMEOUT)?;
                return Err(DeviceError::Io(format!(
                    "the radio sent {piece} bytes into a buffer with room for {}",
                    self.refill - got
                )));
            }
            link.read_exact(&mut block.bytes_mut()[got..got + piece], REFILL_TIMEOUT)?;
            got += piece;
        }
        block.truncate(got);
        Ok(block)
    }
}

impl CaptureStream for RxStream {
    type Block = Block;
    type Stop = Stopper;

    fn stop_handle(&self) -> Stopper {
        self.stopper.clone()
    }

    fn next_block(&self, _timeout: Duration) -> Next<Block> {
        let mut link = lock(&self.inner);
        if self.stopper.is_stopped() {
            return Next::Ended;
        }
        match self.refill(&mut link) {
            Ok(block) if block.is_empty() => Next::Idle,
            Ok(block) => Next::Block(block),
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
        lock(&self.inner).failure()
    }
}

impl Drop for RxStream {
    fn drop(&mut self) {
        // The buffer is closed even when the conversation was already stopped: over usb the
        // command still reaches the radio, and a buffer left open refuses the next one.
        let mut link = lock(&self.inner);
        close_buffer(&mut link, &self.device);
        link.close();
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
                failure.fail(clone_error(&error));
            }
        },
    )
}

/// The same fault, told to every lane. A fault reaches one handler but every lane of a radio
/// that has stopped needs to hear about it.
fn clone_error(error: &DeviceError) -> DeviceError {
    match error {
        DeviceError::NotFound(why) => DeviceError::NotFound(why.clone()),
        DeviceError::Unsupported(why) => DeviceError::Unsupported(why.clone()),
        DeviceError::Disconnected(why) => DeviceError::Disconnected(why.clone()),
        DeviceError::InUse(why) => DeviceError::InUse(why.clone()),
        other => DeviceError::Io(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

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
