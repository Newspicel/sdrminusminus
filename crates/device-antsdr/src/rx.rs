use std::{
    net::UdpSocket,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use sdrmm_device::{
    Block, BlockPool, CaptureRadio, CaptureStream, DeviceError, Next, Sample, SampleConverter,
    StopHandle, StreamFailure, lock,
};

use crate::{
    board::{self, Timeline},
    chdr::{self, Kind},
    control::Control,
    link::{self, MTU, Ports, Received},
    radio::{self, RX_STREAM_IDS},
};

pub(crate) const PACKET_SAMPLES: usize = (MTU - 16) / chdr::WORD;
const SAMPLE_BYTES: usize = chdr::WORD;
const RECEIVE_BUFFER: usize = 32 << 20;
const BLOCK_SPAN_S: f64 = 0.02;
const MIN_BLOCK_FRAMES: usize = 4096;
const MAX_BLOCK_FRAMES: usize = 1 << 18;
const WAIT_SLICE: Duration = Duration::from_millis(20);
const RESTART_GUARD: Duration = Duration::from_millis(80);
const ERROR_OVERFLOW: u8 = 0x08;
const ERROR_LATE: u8 = 0x02;
const DISPATCH: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, b'r'];

#[derive(Clone, Debug, Default)]
pub(crate) struct Stopper(Arc<AtomicBool>);

impl Stopper {
    fn stopped(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

impl StopHandle for Stopper {
    fn stop(&self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(crate) fn block_frames(rate: f64) -> usize {
    let wanted = if rate.is_finite() && rate > 0.0 {
        (rate * BLOCK_SPAN_S) as usize
    } else {
        MIN_BLOCK_FRAMES
    };
    wanted
        .clamp(MIN_BLOCK_FRAMES, MAX_BLOCK_FRAMES)
        .div_ceil(PACKET_SAMPLES)
        * PACKET_SAMPLES
}

pub(crate) struct RxRadio {
    control: Arc<Control>,
    host: String,
    ports: Ports,
    lanes: usize,
    frames: usize,
    timeline: Arc<Timeline>,
    pool: BlockPool,
    armed: Mutex<Option<Stopper>>,
}

impl std::fmt::Debug for RxRadio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RxRadio")
            .field("host", &self.host)
            .field("lanes", &self.lanes)
            .finish_non_exhaustive()
    }
}

impl RxRadio {
    pub(crate) fn new(
        control: Arc<Control>,
        host: String,
        ports: Ports,
        lanes: usize,
        rate: f64,
        timeline: Arc<Timeline>,
    ) -> Self {
        Self {
            control,
            host,
            ports,
            lanes,
            frames: block_frames(rate),
            timeline,
            pool: BlockPool::default(),
            armed: Mutex::new(None),
        }
    }

    fn lanes(&self) -> Vec<usize> {
        (0..self.lanes).collect()
    }
}

impl CaptureRadio for RxRadio {
    type Stream = RxStream;

    fn arm(&self) -> Result<RxStream, DeviceError> {
        let socket = link::connect(&self.host, self.ports.rx(), RECEIVE_BUFFER)?;
        board::stop_lanes(&self.control, self.lanes)?;
        for lane in 0..self.lanes {
            radio::frame_rx(&self.control, lane, PACKET_SAMPLES)?;
        }
        socket.send(&DISPATCH).map_err(link::io)?;
        link::flush(&socket);
        board::start_lanes(&self.control, &self.lanes(), &self.timeline)?;
        let stopper = Stopper::default();
        *lock(&self.armed) = Some(stopper.clone());
        tracing::debug!(
            host = self.host,
            lanes = self.lanes,
            "antsdr receive stream started"
        );
        Ok(RxStream {
            socket,
            control: self.control.clone(),
            lanes: self.lanes,
            timeline: self.timeline.clone(),
            stopper,
            pool: self.pool.clone(),
            capacity: self.frames * self.lanes * SAMPLE_BYTES,
            assembly: Mutex::new(Assembly::new(self.lanes, self.timeline.generation())),
            gap: AtomicU64::new(0),
            failure: Mutex::new(None),
        })
    }

    fn disarm(&self) {
        if let Some(stopper) = lock(&self.armed).take() {
            stopper.stop();
        }
        if let Err(e) = board::stop_lanes(&self.control, self.lanes) {
            tracing::debug!("antsdr receive stop: {e}");
        }
    }
}

pub(crate) struct RxStream {
    socket: UdpSocket,
    control: Arc<Control>,
    lanes: usize,
    timeline: Arc<Timeline>,
    stopper: Stopper,
    pool: BlockPool,
    capacity: usize,
    assembly: Mutex<Assembly>,
    gap: AtomicU64,
    failure: Mutex<Option<StreamFailure>>,
}

impl std::fmt::Debug for RxStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RxStream")
            .field("lanes", &self.lanes)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
struct Held {
    time: Option<u64>,
    bytes: Vec<u8>,
    present: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Accepted {
    Nothing,
    Group { time: Option<u64>, frames: usize },
    Restart,
}

#[derive(Debug)]
struct Assembly {
    lanes: usize,
    datagram: Vec<u8>,
    held: [Held; 2],
    group: Vec<u8>,
    carry: Vec<u8>,
    carry_gap: u64,
    expected: Option<u64>,
    generation: u32,
    restarted: Option<Instant>,
    overflows: u64,
}

impl Assembly {
    fn new(lanes: usize, generation: u32) -> Self {
        Self {
            lanes,
            datagram: vec![0; MTU + 64],
            held: Default::default(),
            group: Vec::with_capacity(PACKET_SAMPLES * 2 * SAMPLE_BYTES),
            carry: Vec::new(),
            carry_gap: 0,
            expected: None,
            generation,
            restarted: None,
            overflows: 0,
        }
    }

    fn accept(&mut self, n: usize) -> Result<Accepted, DeviceError> {
        let (header, length) = {
            let packet = chdr::read(&self.datagram[..n])?;
            (packet.header, packet.payload.len())
        };
        let Some(lane) = RX_STREAM_IDS
            .iter()
            .take(self.lanes)
            .position(|sid| *sid == header.sid)
        else {
            tracing::trace!(sid = header.sid, "antsdr packet for no lane");
            return Ok(Accepted::Nothing);
        };
        let start = header.bytes();
        let payload = &self.datagram[start..start + length / SAMPLE_BYTES * SAMPLE_BYTES];
        if header.kind == Kind::Context {
            let word = chdr::word(payload, 0);
            let code = (word | word.swap_bytes()) as u8;
            return Ok(self.error(lane, code));
        }
        if self.lanes == 1 {
            self.group.clear();
            self.group.extend_from_slice(payload);
            return Ok(Accepted::Group {
                time: header.time,
                frames: payload.len() / SAMPLE_BYTES,
            });
        }
        let held = &mut self.held[lane];
        held.bytes.clear();
        held.bytes.extend_from_slice(payload);
        held.time = header.time;
        held.present = true;
        Ok(self.pair())
    }

    fn pair(&mut self) -> Accepted {
        let [first, second] = &mut self.held;
        if !(first.present && second.present) {
            return Accepted::Nothing;
        }
        match (first.time, second.time) {
            (Some(a), Some(b)) if a < b => {
                first.present = false;
                return Accepted::Nothing;
            }
            (Some(a), Some(b)) if b < a => {
                second.present = false;
                return Accepted::Nothing;
            }
            _ => {}
        }
        let frames = first.bytes.len().min(second.bytes.len()) / SAMPLE_BYTES;
        self.group.clear();
        for frame in 0..frames {
            let at = frame * SAMPLE_BYTES;
            self.group
                .extend_from_slice(&first.bytes[at..at + SAMPLE_BYTES]);
            self.group
                .extend_from_slice(&second.bytes[at..at + SAMPLE_BYTES]);
        }
        first.present = false;
        second.present = false;
        Accepted::Group {
            time: first.time,
            frames,
        }
    }

    fn error(&mut self, lane: usize, code: u8) -> Accepted {
        let restart = code & (ERROR_OVERFLOW | ERROR_LATE) != 0;
        if !restart {
            tracing::debug!(lane, code, "antsdr receive error");
            return Accepted::Nothing;
        }
        if self
            .restarted
            .is_some_and(|at| at.elapsed() < RESTART_GUARD)
        {
            return Accepted::Nothing;
        }
        if code & ERROR_OVERFLOW != 0 {
            self.overflows += 1;
            if self.overflows == 1 {
                tracing::warn!(
                    lane,
                    "the radio overflowed; lower the rate or the lanes if this repeats"
                );
            } else {
                tracing::debug!(lane, overflows = self.overflows, "the radio overflowed");
            }
        } else {
            tracing::debug!(lane, "a timed start arrived late");
        }
        for held in &mut self.held {
            held.present = false;
        }
        self.restarted = Some(Instant::now());
        Accepted::Restart
    }

    fn lost_before(&mut self, time: Option<u64>, frames: usize, timeline: &Timeline) -> u64 {
        let generation = timeline.generation();
        if generation != self.generation {
            self.generation = generation;
            self.expected = None;
        }
        let per_sample = timeline.ticks_per_sample();
        let lost = match (time, self.expected) {
            (Some(now), Some(due)) if now > due => (now - due) / per_sample,
            _ => 0,
        };
        self.expected = time.map(|now| now + frames as u64 * per_sample);
        lost
    }
}

impl RxStream {
    fn fail(&self, reason: String) -> Next<Block> {
        *lock(&self.failure) = Some(StreamFailure {
            reason,
            gone: false,
        });
        Next::Ended
    }

    fn restart(&self) -> Result<(), DeviceError> {
        let lanes: Vec<usize> = (0..self.lanes).collect();
        if self.lanes > 1 {
            board::stop_lanes(&self.control, self.lanes)?;
        }
        board::start_lanes(&self.control, &lanes, &self.timeline)
    }
}

impl CaptureStream for RxStream {
    type Block = Block;
    type Stop = Stopper;

    fn stop_handle(&self) -> Stopper {
        self.stopper.clone()
    }

    fn next_block(&self, timeout: Duration) -> Next<Block> {
        if self.stopper.stopped() {
            return Next::Ended;
        }
        let mut assembly = lock(&self.assembly);
        let assembly = &mut *assembly;
        let deadline = Instant::now() + timeout;
        let group_room = PACKET_SAMPLES * self.lanes * SAMPLE_BYTES;
        let mut block = self.pool.take(self.capacity.max(group_room));
        let mut filled = 0;
        let mut gap = 0;
        if !assembly.carry.is_empty() {
            let carried = assembly.carry.len();
            block.bytes_mut()[..carried].copy_from_slice(&assembly.carry);
            assembly.carry.clear();
            filled = carried;
            gap = std::mem::take(&mut assembly.carry_gap);
        }
        while filled + group_room <= block.len() {
            if self.stopper.stopped() {
                return Next::Ended;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let n = match link::receive(&self.socket, &mut assembly.datagram, left.min(WAIT_SLICE))
            {
                Ok(Received::Got(n)) => n,
                Ok(Received::Quiet) => continue,
                Err(e) => return self.fail(e.to_string()),
            };
            let (time, frames) = match assembly.accept(n) {
                Ok(Accepted::Group { time, frames }) => (time, frames),
                Ok(Accepted::Nothing) => continue,
                Ok(Accepted::Restart) => {
                    if let Err(e) = self.restart() {
                        return self.fail(format!("restarting after an overflow: {e}"));
                    }
                    continue;
                }
                Err(e) => {
                    tracing::debug!("antsdr packet: {e}");
                    continue;
                }
            };
            let lost = assembly.lost_before(time, frames, &self.timeline);
            if lost > 0 && filled > 0 {
                assembly.carry.extend_from_slice(&assembly.group);
                assembly.carry_gap = lost;
                break;
            }
            gap += lost;
            let bytes = assembly.group.len();
            block.bytes_mut()[filled..filled + bytes].copy_from_slice(&assembly.group);
            filled += bytes;
        }
        if filled == 0 {
            if gap > 0 {
                assembly.carry_gap += gap;
            }
            return Next::Idle;
        }
        block.truncate(filled);
        self.gap.store(gap * self.lanes as u64, Ordering::Release);
        Next::Block(block)
    }

    fn dropped(&self) -> u64 {
        0
    }

    fn block_gap(&self, _block: &Block) -> Option<u64> {
        Some(self.gap.swap(0, Ordering::AcqRel))
    }

    fn failure(&self) -> StreamFailure {
        lock(&self.failure)
            .clone()
            .unwrap_or_else(|| StreamFailure {
                reason: "the receive stream stopped".to_string(),
                gone: false,
            })
    }
}

pub(crate) struct IqConverter {
    timeline: Arc<Timeline>,
    out: Vec<Sample>,
}

impl IqConverter {
    pub(crate) fn new(timeline: Arc<Timeline>, samples: usize) -> Self {
        Self {
            timeline,
            out: Vec::with_capacity(samples),
        }
    }
}

pub(crate) fn decode(word: [u8; 4], scale: f32) -> Sample {
    let word = u32::from_le_bytes(word);
    let i = (word >> 16) as u16 as i16;
    let q = word as u16 as i16;
    Sample::new(f32::from(i) * scale, f32::from(q) * scale)
}

impl SampleConverter for IqConverter {
    fn convert(&mut self, bytes: &[u8]) -> &[Sample] {
        let scale = self.timeline.rx_scale();
        self.out.clear();
        let (words, _) = bytes.as_chunks::<4>();
        self.out
            .extend(words.iter().map(|word| decode(*word, scale)));
        &self.out
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests;
