use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration,
};

use arc_swap::ArcSwapOption;
use num_complex::Complex;
use rtrb::{Consumer, Producer, RingBuffer};
use sdrmm_recorder::{BYTES_PER_SAMPLE, SigmfError, SigmfWriter};
use sdrmm_wire::PositionFix;

use crate::EngineError;

const FEED_CAPACITY: usize = 1 << 20;
const MARK_CAPACITY: usize = 64;
const IDLE_SLEEP: Duration = Duration::from_millis(2);

#[derive(Clone, Copy, Debug)]
struct CenterMark {
    at: u64,
    center_hz: f64,
}

#[derive(Debug, Default)]
pub(crate) struct TimeMachineShared {
    held: AtomicU64,
    captured: AtomicU64,
    capturing: AtomicBool,
    lost: AtomicU64,
    error: ArcSwapOption<String>,
}

impl TimeMachineShared {
    pub(crate) fn held(&self) -> u64 {
        self.held.load(Ordering::Relaxed)
    }

    pub(crate) fn captured(&self) -> u64 {
        self.captured.load(Ordering::Relaxed)
    }

    pub(crate) fn captured_bytes(&self) -> u64 {
        self.captured() * BYTES_PER_SAMPLE
    }

    pub(crate) fn capturing(&self) -> bool {
        self.capturing.load(Ordering::Acquire)
    }

    pub(crate) fn expect_capture(&self) {
        self.captured.store(0, Ordering::Relaxed);
        self.capturing.store(true, Ordering::Release);
    }

    pub(crate) fn error(&self) -> Option<String> {
        self.error.load_full().map(|error| (*error).clone())
    }

    fn fail(&self, message: String) {
        self.error.store(Some(Arc::new(message)));
    }
}

pub(crate) struct TimeMachineTap {
    samples: Producer<Complex<f32>>,
    marks: Producer<CenterMark>,
    shared: Arc<TimeMachineShared>,
    pushed: u64,
    center_hz: f64,
}

impl TimeMachineTap {
    #[must_use]
    pub(crate) fn push(&mut self, samples: &[Complex<f32>], center_hz: f64) -> bool {
        if samples.is_empty() {
            return true;
        }
        if self.samples.is_abandoned() {
            self.shared.fail("the history keeper stopped".to_owned());
            return false;
        }
        if center_hz != self.center_hz {
            let mark = CenterMark {
                at: self.pushed,
                center_hz,
            };
            if self.marks.push(mark).is_err() {
                self.shared
                    .fail("history retune queue overflow — the keeper is not draining".to_owned());
                return false;
            }
            self.center_hz = center_hz;
        }
        if self.samples.push_entire_slice(samples).is_ok() {
            self.pushed += samples.len() as u64;
            return true;
        }
        let lost = self
            .shared
            .lost
            .fetch_add(samples.len() as u64, Ordering::Relaxed)
            + samples.len() as u64;
        self.shared.fail(format!(
            "history feed overflowed: {lost} samples never reached the buffer"
        ));
        true
    }
}

pub(crate) enum TimeMachineCommand {
    Capture(Box<SigmfWriter>),
    Stop,
    Position(Option<Box<PositionFix>>),
}

#[derive(Clone, Debug)]
pub(crate) struct TimeMachineControl {
    tx: mpsc::Sender<TimeMachineCommand>,
}

impl TimeMachineControl {
    pub(crate) fn send(&self, command: TimeMachineCommand) -> Result<(), EngineError> {
        self.tx
            .send(command)
            .map_err(|_| EngineError::Recording("the history keeper stopped".to_owned()))
    }
}

struct Marks {
    base_hz: f64,
    list: VecDeque<CenterMark>,
}

impl Marks {
    fn new(center_hz: f64) -> Self {
        Self {
            base_hz: center_hz,
            list: VecDeque::new(),
        }
    }

    fn push(&mut self, mark: CenterMark) {
        self.list.push_back(mark);
    }

    fn center_at(&self, at: u64) -> f64 {
        self.list
            .iter()
            .rev()
            .find(|mark| mark.at <= at)
            .map_or(self.base_hz, |mark| mark.center_hz)
    }

    fn next_after(&self, at: u64) -> Option<u64> {
        self.list
            .iter()
            .find(|mark| mark.at > at)
            .map(|mark| mark.at)
    }

    fn trim(&mut self, before: u64) {
        while self.list.front().is_some_and(|mark| mark.at <= before) {
            if let Some(mark) = self.list.pop_front() {
                self.base_hz = mark.center_hz;
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Epoch {
    at: u64,
    time: jiff::Timestamp,
    rate: f64,
}

impl Epoch {
    fn time_of(&self, at: u64) -> String {
        let offset = (at as f64 - self.at as f64) / self.rate;
        let nanos = (offset * 1e9).clamp(i64::MIN as f64, i64::MAX as f64) as i64;
        self.time
            .checked_add(jiff::SignedDuration::from_nanos(nanos))
            .unwrap_or(self.time)
            .to_string()
    }
}

struct Capture {
    writer: SigmfWriter,
    center_hz: f64,
}

pub(crate) struct TimeMachineHandle {
    tap: Option<TimeMachineTap>,
    control: TimeMachineControl,
    keeper: Option<JoinHandle<()>>,
    shared: Arc<TimeMachineShared>,
    capacity: u64,
}

impl TimeMachineHandle {
    pub(crate) fn take_tap(&mut self) -> Option<TimeMachineTap> {
        self.tap.take()
    }

    pub(crate) fn control(&self) -> TimeMachineControl {
        self.control.clone()
    }

    pub(crate) fn shared(&self) -> &Arc<TimeMachineShared> {
        &self.shared
    }

    pub(crate) fn capacity(&self) -> u64 {
        self.capacity
    }

    pub(crate) fn join(mut self) {
        drop(self.tap.take());
        if let Some(keeper) = self.keeper.take()
            && keeper.join().is_err()
        {
            tracing::error!("time machine keeper thread panicked");
        }
    }
}

impl Drop for TimeMachineHandle {
    fn drop(&mut self) {
        drop(self.tap.take());
        if let Some(keeper) = self.keeper.take() {
            let _ = keeper.join();
        }
    }
}

pub(crate) fn start(
    capacity: u64,
    sample_rate: f64,
    center_hz: f64,
) -> Result<TimeMachineHandle, EngineError> {
    let held = usize::try_from(capacity).map_err(|_| {
        EngineError::Recording("the requested history does not fit in this machine".to_owned())
    })?;
    if held == 0 {
        return Err(EngineError::Recording(
            "a history of no samples holds nothing to capture".to_owned(),
        ));
    }
    let mut ring: Vec<Complex<f32>> = Vec::new();
    ring.try_reserve_exact(held).map_err(|_| {
        EngineError::Recording(format!(
            "cannot hold {} MiB of history: the allocation was refused",
            capacity * BYTES_PER_SAMPLE / (1 << 20)
        ))
    })?;
    ring.resize(held, Complex::new(0.0, 0.0));

    let (samples_tx, mut samples_rx) = RingBuffer::<Complex<f32>>::new(FEED_CAPACITY);
    let (marks_tx, mut marks_rx) = RingBuffer::<CenterMark>::new(MARK_CAPACITY);
    let (control_tx, control_rx) = mpsc::channel();
    let shared = Arc::new(TimeMachineShared::default());
    let tap = TimeMachineTap {
        samples: samples_tx,
        marks: marks_tx,
        shared: shared.clone(),
        pushed: 0,
        center_hz,
    };
    let keeper_shared = shared.clone();
    let keeper = std::thread::Builder::new()
        .name("sdrmm-timemachine".to_owned())
        .spawn(move || {
            Keeper {
                ring,
                write: 0,
                filled: 0,
                next: 0,
                marks: Marks::new(center_hz),
                epoch: Epoch {
                    at: 0,
                    time: jiff::Timestamp::now(),
                    rate: sample_rate,
                },
                capture: None,
                shared: keeper_shared,
                position: None,
            }
            .run(&mut samples_rx, &mut marks_rx, &control_rx);
        })
        .map_err(|error| {
            EngineError::Recording(format!("spawn time machine keeper thread: {error}"))
        })?;
    Ok(TimeMachineHandle {
        tap: Some(tap),
        control: TimeMachineControl { tx: control_tx },
        keeper: Some(keeper),
        shared,
        capacity,
    })
}

struct Keeper {
    ring: Vec<Complex<f32>>,
    write: usize,
    filled: usize,
    next: u64,
    marks: Marks,
    epoch: Epoch,
    capture: Option<Capture>,
    shared: Arc<TimeMachineShared>,
    position: Option<PositionFix>,
}

impl Keeper {
    fn run(
        mut self,
        samples: &mut Consumer<Complex<f32>>,
        marks: &mut Consumer<CenterMark>,
        control: &mpsc::Receiver<TimeMachineCommand>,
    ) {
        loop {
            self.drain_control(control);
            self.drain_marks(marks);
            if self.drain_samples(samples) {
                continue;
            }
            if samples.is_abandoned() {
                self.close();
                return;
            }
            std::thread::sleep(IDLE_SLEEP);
        }
    }

    fn drain_control(&mut self, control: &mpsc::Receiver<TimeMachineCommand>) {
        while let Ok(command) = control.try_recv() {
            match command {
                TimeMachineCommand::Capture(writer) => self.begin(*writer),
                TimeMachineCommand::Stop => self.close(),
                TimeMachineCommand::Position(fix) => {
                    self.position = fix.map(|fix| *fix);
                    if let Some(capture) = self.capture.as_mut() {
                        capture.writer.set_position(self.position.as_ref());
                    }
                }
            }
        }
    }

    fn drain_marks(&mut self, marks: &mut Consumer<CenterMark>) {
        while let Ok(mark) = marks.pop() {
            self.marks.push(mark);
        }
    }

    fn drain_samples(&mut self, samples: &mut Consumer<Complex<f32>>) -> bool {
        let available = samples.slots();
        if available == 0 {
            return false;
        }
        let Ok(chunk) = samples.read_chunk(available) else {
            return false;
        };
        let (first, second) = chunk.as_slices();
        let mut written = Ok(());
        for slice in [first, second] {
            if slice.is_empty() {
                continue;
            }
            let at = self.next;
            append(
                &mut self.ring,
                &mut self.write,
                &mut self.filled,
                &mut self.next,
                slice,
            );
            self.marks.trim(self.next - self.filled as u64);
            if written.is_ok()
                && let Some(capture) = self.capture.as_mut()
            {
                written = emit(capture, &self.marks, &self.epoch, at, slice);
            }
        }
        chunk.commit_all();
        self.epoch.at = self.next;
        self.epoch.time = jiff::Timestamp::now();
        self.shared
            .held
            .store(self.filled as u64, Ordering::Relaxed);
        if let Some(capture) = self.capture.as_ref() {
            self.shared
                .captured
                .store(capture.writer.samples_written(), Ordering::Relaxed);
        }
        if let Err(error) = written {
            self.fail(&error);
        }
        true
    }

    fn begin(&mut self, writer: SigmfWriter) {
        self.close();
        let first = self.next - self.filled as u64;
        let mut capture = Capture {
            writer,
            center_hz: self.marks.center_at(first),
        };
        capture.writer.stamp_capture(&self.epoch.time_of(first));
        capture.writer.set_position(self.position.as_ref());
        let (head, tail) = window(&self.ring, self.write, self.filled);
        let written = emit(&mut capture, &self.marks, &self.epoch, first, head).and_then(|()| {
            emit(
                &mut capture,
                &self.marks,
                &self.epoch,
                first + head.len() as u64,
                tail,
            )
        });
        self.shared
            .captured
            .store(capture.writer.samples_written(), Ordering::Relaxed);
        self.shared.capturing.store(true, Ordering::Release);
        self.capture = Some(capture);
        if let Err(error) = written {
            self.fail(&error);
        }
    }

    fn fail(&mut self, error: &SigmfError) {
        self.shared
            .fail(format!("time machine capture write failed: {error}"));
        self.close();
    }

    fn close(&mut self) {
        let Some(capture) = self.capture.take() else {
            return;
        };
        if let Err(error) = capture.writer.finalize() {
            self.shared
                .fail(format!("time machine capture finalize failed: {error}"));
        }
        self.shared.capturing.store(false, Ordering::Release);
    }
}

fn append(
    ring: &mut [Complex<f32>],
    write: &mut usize,
    filled: &mut usize,
    next: &mut u64,
    samples: &[Complex<f32>],
) {
    let capacity = ring.len();
    let tail = &samples[samples.len().saturating_sub(capacity)..];
    for &sample in tail {
        ring[*write] = sample;
        *write = (*write + 1) % capacity;
    }
    *filled = (*filled + tail.len()).min(capacity);
    *next += samples.len() as u64;
}

fn window(
    ring: &[Complex<f32>],
    write: usize,
    filled: usize,
) -> (&[Complex<f32>], &[Complex<f32>]) {
    let capacity = ring.len();
    let start = (write + capacity - filled) % capacity;
    if start + filled <= capacity {
        (&ring[start..start + filled], &[])
    } else {
        let head = capacity - start;
        (&ring[start..], &ring[..filled - head])
    }
}

fn emit(
    capture: &mut Capture,
    marks: &Marks,
    epoch: &Epoch,
    at: u64,
    samples: &[Complex<f32>],
) -> Result<(), SigmfError> {
    let mut offset = 0usize;
    while offset < samples.len() {
        let position = at + offset as u64;
        let center_hz = marks.center_at(position);
        if center_hz != capture.center_hz {
            capture.writer.add_capture(center_hz);
            capture.writer.stamp_capture(&epoch.time_of(position));
            capture.center_hz = center_hz;
        }
        let until = marks.next_after(position).unwrap_or(u64::MAX);
        let take = ((until - position) as usize).min(samples.len() - offset);
        capture
            .writer
            .write_block(&samples[offset..offset + take])?;
        offset += take;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use sdrmm_recorder::SigmfReader;
    use tempfile::TempDir;

    use super::*;

    const RATE: f64 = 48_000.0;

    fn ramp(from: u64, len: usize) -> Vec<Complex<f32>> {
        (0..len)
            .map(|i| Complex::new((from + i as u64) as f32, 0.0))
            .collect()
    }

    fn settle(handle: &TimeMachineHandle, held: u64) {
        for _ in 0..500 {
            if handle.shared().held() >= held {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!(
            "the keeper never took {held} samples, it holds {}",
            handle.shared().held()
        );
    }

    fn wait_capture(handle: &TimeMachineHandle, samples: u64) {
        for _ in 0..500 {
            if handle.shared().captured() >= samples {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!(
            "the capture never reached {samples} samples, it wrote {}",
            handle.shared().captured()
        );
    }

    #[test]
    fn the_window_keeps_only_the_last_n_samples() {
        let mut handle = start(1_024, RATE, 100e6).expect("armed");
        let mut tap = handle.take_tap().expect("tap");
        assert!(tap.push(&ramp(0, 4_096), 100e6));
        settle(&handle, 1_024);
        assert_eq!(handle.shared().held(), 1_024);
        assert_eq!(handle.capacity(), 1_024);
        drop(tap);
        handle.join();
    }

    #[test]
    fn a_capture_lays_down_the_buffered_past_then_keeps_recording() {
        let dir = TempDir::new().expect("tempdir");
        let stem = dir.path().join("tm");
        let mut handle = start(1_024, RATE, 100e6).expect("armed");
        let mut tap = handle.take_tap().expect("tap");
        let control = handle.control();

        assert!(tap.push(&ramp(0, 1_024), 100e6));
        settle(&handle, 1_024);
        let writer = SigmfWriter::create(&stem, RATE, 100e6, "test").expect("writer");
        control
            .send(TimeMachineCommand::Capture(Box::new(writer)))
            .expect("capture");
        wait_capture(&handle, 1_024);
        assert!(handle.shared().capturing());

        assert!(tap.push(&ramp(1_024, 512), 100e6));
        wait_capture(&handle, 1_536);
        control.send(TimeMachineCommand::Stop).expect("stop");
        drop(tap);
        handle.join();

        let reader = SigmfReader::open(&stem).expect("read back");
        assert_eq!(reader.total_samples(), 1_536);
        assert_eq!(reader.meta().captures.len(), 1);
    }

    #[test]
    fn a_retune_inside_the_window_lands_as_its_own_capture_segment() {
        let dir = TempDir::new().expect("tempdir");
        let stem = dir.path().join("retuned");
        let mut handle = start(1_024, RATE, 100e6).expect("armed");
        let mut tap = handle.take_tap().expect("tap");
        let control = handle.control();

        assert!(tap.push(&ramp(0, 256), 100e6));
        assert!(tap.push(&ramp(256, 256), 101e6));
        settle(&handle, 512);
        let writer = SigmfWriter::create(&stem, RATE, 100e6, "test").expect("writer");
        control
            .send(TimeMachineCommand::Capture(Box::new(writer)))
            .expect("capture");
        wait_capture(&handle, 512);
        control.send(TimeMachineCommand::Stop).expect("stop");
        drop(tap);
        handle.join();

        let reader = SigmfReader::open(&stem).expect("read back");
        let captures = &reader.meta().captures;
        assert_eq!(captures.len(), 2);
        assert_eq!(captures[0].frequency, Some(100e6));
        assert_eq!(captures[1].sample_start, 256);
        assert_eq!(captures[1].frequency, Some(101e6));
        assert!(captures[0].datetime.is_some(), "the past is stamped");
    }

    #[test]
    fn the_oldest_sample_is_stamped_before_the_capture_was_asked_for() {
        let dir = TempDir::new().expect("tempdir");
        let stem = dir.path().join("stamped");
        let mut handle = start(48_000, RATE, 100e6).expect("armed");
        let mut tap = handle.take_tap().expect("tap");
        let control = handle.control();
        assert!(tap.push(&ramp(0, 48_000), 100e6));
        settle(&handle, 48_000);
        let asked = jiff::Timestamp::now();
        let writer = SigmfWriter::create(&stem, RATE, 100e6, "test").expect("writer");
        control
            .send(TimeMachineCommand::Capture(Box::new(writer)))
            .expect("capture");
        wait_capture(&handle, 48_000);
        control.send(TimeMachineCommand::Stop).expect("stop");
        drop(tap);
        handle.join();

        let reader = SigmfReader::open(&stem).expect("read back");
        let stamp: jiff::Timestamp = reader.meta().captures[0]
            .datetime
            .as_deref()
            .expect("stamped")
            .parse()
            .expect("a timestamp");
        let back = (asked - stamp).total(jiff::Unit::Second).expect("seconds");
        assert!(
            (0.5..2.0).contains(&back),
            "one second of history was stamped {back} s before the press"
        );
    }

    #[test]
    fn a_window_wider_than_the_buffer_keeps_the_newest_samples() {
        let mut ring = vec![Complex::new(0.0, 0.0); 4];
        let (mut write, mut filled, mut next) = (0usize, 0usize, 0u64);
        append(&mut ring, &mut write, &mut filled, &mut next, &ramp(0, 10));
        assert_eq!((filled, next), (4, 10));
        let (head, tail) = window(&ring, write, filled);
        let held: Vec<f32> = head.iter().chain(tail).map(|sample| sample.re).collect();
        assert_eq!(held, vec![6.0, 7.0, 8.0, 9.0]);
    }
}
