use serde_json::Value;

use super::{
    frame::{CODED_BITS, FrameDecoder, FrameHeader, HEADER_BITS, UW},
    satellite::SatelliteResolver,
    state::SuperframeLockStateMachine,
    su::{self, AeroUserData, Reassembler},
};

const UW_TOLERANCE: u32 = 2;

pub(super) struct FrameSink {
    decoder: FrameDecoder,
    reassembler: Reassembler,
    pub resolver: SatelliteResolver,
    pub su_events: Vec<Value>,
    pub last_header: Option<FrameHeader>,
    pub last_fec_corrected: Option<u32>,
}

impl FrameSink {
    pub(super) fn new(rate_bps: u32) -> Self {
        Self {
            decoder: FrameDecoder::new(rate_bps),
            reassembler: Reassembler::default(),
            resolver: SatelliteResolver::default(),
            su_events: Vec::new(),
            last_header: None,
            last_fec_corrected: None,
        }
    }

    pub(super) fn decode(
        &mut self,
        header: FrameHeader,
        coded: &[f32],
        out: &mut Vec<AeroUserData>,
    ) {
        self.last_header = Some(header);
        let bytes = self.decoder.decode(coded);
        self.last_fec_corrected = Some(self.decoder.last_fec_corrected());
        for unit in bytes.chunks_exact(su::SU_LEN) {
            if !su::su_crc_ok(unit) {
                continue;
            }
            if let Some(event) = su::parse_p_su(unit) {
                self.resolver.observe(&event);
                self.su_events.push(event);
            }
            if let Some(user) = self.reassembler.push(unit) {
                out.push(user);
            }
        }
    }
}

pub(super) struct Framer {
    pub sink: FrameSink,
    pub lock: SuperframeLockStateMachine,
    shift: u32,
    buffer: Vec<f32>,
    collecting: bool,
}

impl Framer {
    pub(super) fn new(rate_bps: u32) -> Self {
        Self {
            sink: FrameSink::new(rate_bps),
            lock: SuperframeLockStateMachine::new(),
            shift: 0,
            buffer: Vec::with_capacity(HEADER_BITS + CODED_BITS),
            collecting: false,
        }
    }

    pub(super) fn push(&mut self, soft: f32, hard: u8, out: &mut Vec<AeroUserData>) {
        if self.collecting {
            self.buffer.push(soft);
            if self.buffer.len() == HEADER_BITS + CODED_BITS {
                let header = FrameHeader::from_soft_bits(&self.buffer[..HEADER_BITS]);
                self.lock.update(header);
                self.sink.decode(header, &self.buffer[HEADER_BITS..], out);
                self.buffer.clear();
                self.collecting = false;
            }
        }
        self.shift = (self.shift << 1) | u32::from(hard);
        if !self.collecting && (self.shift ^ UW).count_ones() <= UW_TOLERANCE {
            self.collecting = true;
        }
    }
}
