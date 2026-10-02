use super::link::{
    IMAGE_VCID, MPDU_DATA, NO_PACKET_START, PACKET_HEADER, PacketHeader, VCDU_BYTES, VcduHeader,
};

const MAX_PACKET: usize = PACKET_HEADER + (1 << 16);
const COUNTER_MASK: u32 = 0xFF_FFFF;
const ZONE_START: usize = VCDU_BYTES - MPDU_DATA;

pub struct Depacketizer {
    zone: [u8; MPDU_DATA],
    zone_len: usize,
    at: usize,
    boundary: Option<usize>,
    packet: Vec<u8>,
    need: usize,
    assembling: bool,
    counter: Option<u32>,
}

impl Depacketizer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            zone: [0; MPDU_DATA],
            zone_len: 0,
            at: 0,
            boundary: None,
            packet: Vec::with_capacity(MAX_PACKET),
            need: PACKET_HEADER,
            assembling: false,
            counter: None,
        }
    }

    pub fn reset(&mut self) {
        self.zone_len = 0;
        self.at = 0;
        self.boundary = None;
        self.assembling = false;
        self.counter = None;
    }

    pub fn load(&mut self, vcdu: &[u8]) {
        self.zone_len = 0;
        self.at = 0;
        self.boundary = None;
        if vcdu.len() < VCDU_BYTES {
            return;
        }
        let header = VcduHeader::parse(vcdu);
        if header.vcid != IMAGE_VCID {
            return;
        }
        let continuous = self
            .counter
            .is_some_and(|previous| header.counter == (previous + 1) & COUNTER_MASK);
        self.counter = Some(header.counter);
        if !continuous {
            self.assembling = false;
        }
        self.zone.copy_from_slice(&vcdu[ZONE_START..VCDU_BYTES]);
        self.zone_len = MPDU_DATA;
        let first = usize::from(header.first_header);
        let pointer =
            (header.first_header != NO_PACKET_START && first < MPDU_DATA).then_some(first);
        match (self.assembling, pointer) {
            (true, pointer) => self.boundary = pointer,
            (false, Some(first)) => self.at = first,
            (false, None) => self.at = MPDU_DATA,
        }
    }

    fn start_packet(&mut self) {
        self.packet.clear();
        self.need = PACKET_HEADER;
        self.assembling = true;
    }

    pub fn next_packet(&mut self) -> Option<&[u8]> {
        loop {
            if self.at >= self.zone_len {
                return None;
            }
            if let Some(boundary) = self.boundary
                && self.at >= boundary
            {
                self.boundary = None;
                self.assembling = false;
            }
            if !self.assembling {
                self.start_packet();
            }
            let limit = self.boundary.unwrap_or(self.zone_len).min(self.zone_len);
            let take = (self.need - self.packet.len()).min(limit - self.at);
            self.packet
                .extend_from_slice(&self.zone[self.at..self.at + take]);
            self.at += take;
            if self.need == PACKET_HEADER && self.packet.len() == PACKET_HEADER {
                self.need = PACKET_HEADER + PacketHeader::parse(&self.packet).length;
            }
            if self.need > PACKET_HEADER && self.packet.len() == self.need {
                self.assembling = false;
                break;
            }
        }
        Some(&self.packet)
    }
}
