use sdrmm_wire::{DecoderEvent, RadiosondeFrame};

use super::{
    demod::{SymbolClock, SyncMatcher},
    m10, m20,
};

pub(crate) const BAUD: f64 = 9_608.0;
pub(crate) const RAW_HEADER: &[u8; 32] = b"10011001100110010100110010011001";
pub(crate) const MAX_FRAME: usize = 0x78;
pub(crate) const MIN_LEN_BYTE: u8 = 0x40;

const SYNC_TOLERANCE: u32 = 1;
const SMOOTHING: f64 = 0.5;

pub(crate) fn update_checksum(sum: u16, byte: u8) -> u16 {
    let sum = u32::from(sum);
    let mut byte = u32::from(byte);
    byte = ((byte >> 1) | ((byte & 1) << 7)) & 0xFF;
    byte ^= byte >> 2;
    let t6 = (sum & 1) ^ ((sum >> 2) & 1) ^ ((sum >> 4) & 1);
    let t7 = ((sum >> 1) & 1) ^ ((sum >> 3) & 1) ^ ((sum >> 5) & 1);
    let low = (sum & 0x3F) | (t6 << 6) | (t7 << 7);
    let mut high = (sum >> 7) & 0xFF;
    high ^= high >> 2;
    ((((sum & 0xFF) << 8) | (byte ^ low ^ high)) & 0xFFFF) as u16
}

pub(crate) fn checksum(data: &[u8]) -> u16 {
    data.iter().fold(0, |sum, &byte| update_checksum(sum, byte))
}

pub(crate) fn frame_check_ok(frame: &[u8]) -> bool {
    let len = usize::from(frame[0]);
    len < frame.len()
        && len >= 2
        && checksum(&frame[..len - 1]) == u16::from_be_bytes([frame[len - 1], frame[len]])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Accept {
    pub m10: bool,
    pub m20: bool,
}

pub(crate) struct MeteomodemFrames {
    rejected: u32,
}

impl MeteomodemFrames {
    pub(crate) fn new() -> Self {
        Self { rejected: 0 }
    }

    pub(crate) fn rejected(&self) -> u32 {
        self.rejected
    }

    pub(crate) fn decode(&mut self, frame: &[u8], accept: Accept) -> Option<RadiosondeFrame> {
        if !frame_check_ok(frame) {
            self.rejected += 1;
            return None;
        }
        let decoded = match frame[1] {
            m10::TYPE_TRIMBLE | m10::TYPE_GTOP if accept.m10 => m10::decode(frame),
            m20::TYPE if accept.m20 => m20::decode(frame),
            _ => return None,
        };
        if decoded.is_none() {
            self.rejected += 1;
        }
        decoded
    }
}

enum Receive {
    Hunt,
    Collect {
        len: usize,
        expected: usize,
        byte: u8,
        bits: u8,
        previous: bool,
        first: Option<f32>,
    },
}

pub(crate) struct Meteomodem {
    clock: SymbolClock,
    sync: SyncMatcher,
    receive: Receive,
    frame: [u8; MAX_FRAME],
    frames: MeteomodemFrames,
    accept: Accept,
}

impl Meteomodem {
    pub(crate) fn new(accept: Accept) -> Self {
        Self {
            clock: SymbolClock::new(BAUD, SMOOTHING),
            sync: SyncMatcher::from_bits(RAW_HEADER, SYNC_TOLERANCE),
            receive: Receive::Hunt,
            frame: [0; MAX_FRAME],
            frames: MeteomodemFrames::new(),
            accept,
        }
    }

    pub(crate) fn set_accept(&mut self, accept: Accept) {
        self.accept = accept;
    }

    pub(crate) fn rejected(&self) -> u32 {
        self.frames.rejected()
    }

    pub(crate) fn push(&mut self, audio: &[f32], out: &mut Vec<DecoderEvent>) {
        for &sample in audio {
            if let Some(symbol) = self.clock.push(sample) {
                self.symbol(symbol, out);
            }
        }
    }

    fn symbol(&mut self, symbol: f32, out: &mut Vec<DecoderEvent>) {
        let Receive::Collect {
            len,
            expected,
            byte,
            bits,
            previous,
            first,
        } = &mut self.receive
        else {
            if self.sync.push(symbol > 0.0).is_some() {
                let tail = self.sync.last_bits(2);
                self.receive = Receive::Collect {
                    len: 0,
                    expected: MAX_FRAME,
                    byte: 0,
                    bits: 0,
                    previous: tail == 0b10,
                    first: None,
                };
            }
            return;
        };
        let Some(early) = first.take() else {
            *first = Some(symbol);
            return;
        };
        let manchester = early > symbol;
        *byte = (*byte << 1) | u8::from(manchester == *previous);
        *previous = manchester;
        *bits += 1;
        if *bits < 8 {
            return;
        }
        self.frame[*len] = *byte;
        *len += 1;
        *bits = 0;
        *byte = 0;
        if *len == 1 {
            if self.frame[0] < MIN_LEN_BYTE || usize::from(self.frame[0]) >= MAX_FRAME {
                self.restart();
                return;
            }
            *expected = usize::from(self.frame[0]) + 1;
        }
        if *len == *expected {
            let len = *len;
            self.restart();
            if let Some(mut frame) = self.frames.decode(&self.frame[..len], self.accept) {
                frame.rejected = self.frames.rejected();
                out.push(DecoderEvent::Radiosonde(frame));
            }
        }
    }

    fn restart(&mut self) {
        self.receive = Receive::Hunt;
        self.sync.clear();
    }
}
