use serde_json::Value;

use super::{
    frame::{CODED_BITS, FrameDecoder, FrameHeader, HEADER_BITS, UW},
    satellite::SatelliteResolver,
    state::SuperframeLockStateMachine,
    su::{self, AeroUserData, Reassembler, SU_LEN},
};

const UW_TOLERANCE: u32 = 2;
const FLYWHEEL_TOLERANCE: u32 = 6;
const FLYWHEEL_FIRST: u32 = 31;
const FLYWHEEL_LAST: u32 = 33;

pub(super) type Unit = [u8; SU_LEN];

pub(super) struct DecodedFrame {
    pub header: FrameHeader,
    pub units: Vec<Option<Unit>>,
    pub fec_corrected: u32,
}

impl DecodedFrame {
    pub(super) fn decode(decoder: &mut FrameDecoder, header: FrameHeader, coded: &[f32]) -> Self {
        let units = decoder
            .decode(coded)
            .as_chunks::<SU_LEN>()
            .0
            .iter()
            .map(|unit| su::su_crc_ok(unit).then_some(*unit))
            .collect();
        Self {
            header,
            units,
            fec_corrected: decoder.last_fec_corrected(),
        }
    }

    fn merge(mut self, other: DecodedFrame, prefer_other_header: bool) -> Self {
        for (unit, alternative) in self.units.iter_mut().zip(other.units) {
            if unit.is_none() {
                *unit = alternative;
            }
        }
        if prefer_other_header {
            self.header = other.header;
        }
        self.fec_corrected = self.fec_corrected.min(other.fec_corrected);
        self
    }
}

struct Pending {
    source: usize,
    time: u64,
    frame: DecodedFrame,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct MergeTiming {
    pub window: u64,
    pub settle: u64,
}

pub(super) struct FrameSink {
    reassembler: Reassembler,
    pending: Option<Pending>,
    merge: Option<MergeTiming>,
    pub lock: SuperframeLockStateMachine,
    pub resolver: SatelliteResolver,
    pub su_events: Vec<Value>,
    pub users: Vec<AeroUserData>,
    pub last_header: Option<FrameHeader>,
    pub last_fec_corrected: Option<u32>,
}

impl FrameSink {
    pub(super) fn new(merge: Option<MergeTiming>) -> Self {
        Self {
            reassembler: Reassembler::default(),
            pending: None,
            merge,
            lock: SuperframeLockStateMachine::new(),
            resolver: SatelliteResolver::default(),
            su_events: Vec::new(),
            users: Vec::new(),
            last_header: None,
            last_fec_corrected: None,
        }
    }

    pub(super) fn has_output(&self) -> bool {
        !self.users.is_empty() || !self.su_events.is_empty()
    }

    pub(super) fn offer(&mut self, source: usize, time: u64, frame: DecodedFrame) {
        let Some(merge) = self.merge else {
            self.flush(frame);
            return;
        };
        if let Some(pending) = self.pending.take() {
            if pending.source != source && pending.time.abs_diff(time) < merge.window {
                let preferred = source > pending.source;
                self.flush(pending.frame.merge(frame, preferred));
                return;
            }
            self.flush(pending.frame);
        }
        self.pending = Some(Pending {
            source,
            time,
            frame,
        });
    }

    pub(super) fn expire(&mut self, now: u64) {
        let (Some(merge), Some(pending)) = (self.merge, &self.pending) else {
            return;
        };
        if now.saturating_sub(pending.time) >= merge.settle
            && let Some(pending) = self.pending.take()
        {
            self.flush(pending.frame);
        }
    }

    fn flush(&mut self, frame: DecodedFrame) {
        self.last_header = Some(frame.header);
        self.last_fec_corrected = Some(frame.fec_corrected);
        self.lock.update(frame.header);
        for unit in frame.units.iter().flatten() {
            if let Some(event) = su::parse_p_su(unit) {
                self.resolver.observe(&event);
                self.su_events.push(event);
            }
            if let Some(user) = self.reassembler.push(unit) {
                self.users.push(user);
            }
        }
    }
}

pub(super) struct Framer {
    decoder: FrameDecoder,
    shift: u32,
    buffer: Vec<f32>,
    collecting: bool,
    flywheel: bool,
    since_frame: Option<u32>,
}

impl Framer {
    pub(super) fn new(rate_bps: u32, flywheel: bool) -> Self {
        Self {
            decoder: FrameDecoder::new(rate_bps),
            shift: 0,
            buffer: Vec::with_capacity(HEADER_BITS + CODED_BITS),
            collecting: false,
            flywheel,
            since_frame: None,
        }
    }

    pub(super) fn push(&mut self, soft: f32, hard: u8) -> Option<DecodedFrame> {
        let mut frame = None;
        if self.collecting {
            self.buffer.push(soft);
            if self.buffer.len() == HEADER_BITS + CODED_BITS {
                let header = FrameHeader::from_soft_bits(&self.buffer[..HEADER_BITS]);
                frame = Some(DecodedFrame::decode(
                    &mut self.decoder,
                    header,
                    &self.buffer[HEADER_BITS..],
                ));
                self.buffer.clear();
                self.collecting = false;
                self.since_frame = self.flywheel.then_some(0);
            }
        }
        self.shift = (self.shift << 1) | u32::from(hard);
        if !self.collecting {
            self.collecting = self.hunt();
        }
        frame
    }

    fn hunt(&mut self) -> bool {
        let errors = (self.shift ^ UW).count_ones();
        let since = self.since_frame.map(|bits| bits + 1);
        self.since_frame = since.filter(|&bits| bits < FLYWHEEL_LAST);
        let expected = since.is_some_and(|bits| (FLYWHEEL_FIRST..=FLYWHEEL_LAST).contains(&bits));
        let found = errors <= UW_TOLERANCE || (expected && errors <= FLYWHEEL_TOLERANCE);
        if found {
            self.since_frame = None;
        }
        found
    }
}
