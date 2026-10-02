use sdrmm_wire::{DecoderEvent, RadiosondeFrame, SondeType};

use super::{
    demod::{Polarity, SymbolClock, SyncMatcher},
    fields::{
        Geodetic, civil_time, empty_frame, finite_f32, plausible, set_position, steinhart_hart,
    },
};

pub(crate) const BAUD: f64 = 2_500.0;
pub(crate) const RAW_HEADER: &[u8; 32] = b"10011010100110010101101001010101";
pub(crate) const CONF_CODEWORDS: usize = 7;
pub(crate) const DAT_CODEWORDS: usize = 13;
pub(crate) const PAYLOAD_BITS: usize = 8 * (CONF_CODEWORDS + 2 * DAT_CODEWORDS);
pub(crate) const SERIAL_MARK: u8 = 0xC;
pub(crate) const THERMISTOR: [f64; 4] = [
    1.096_984_17e-3,
    2.395_646_29e-4,
    2.488_214_37e-6,
    5.843_549_21e-8,
];
pub(crate) const MODE_GPS: u8 = 2;
pub(crate) const MODE_COMPACT: u8 = 3;

const SYNC_TOLERANCE: u32 = 2;
const SMOOTHING: f64 = 0.5;
const CHANNELS: usize = 16;
const PARITY_CHECK: [u8; 8] = [0x7, 0xB, 0xD, 0xE, 0x8, 0x4, 0x2, 0x1];

pub(crate) fn hamming_encode(nibble: u8) -> [bool; 8] {
    let d: [bool; 4] = std::array::from_fn(|k| (nibble >> (3 - k)) & 1 == 1);
    [
        d[0],
        d[1],
        d[2],
        d[3],
        d[1] ^ d[2] ^ d[3],
        d[0] ^ d[2] ^ d[3],
        d[0] ^ d[1] ^ d[3],
        d[0] ^ d[1] ^ d[2],
    ]
}

pub(crate) fn hamming_decode(mut code: [bool; 8]) -> Option<(u8, u32)> {
    let syndrome = code
        .iter()
        .zip(PARITY_CHECK)
        .filter(|(bit, _)| **bit)
        .fold(0u8, |acc, (_, column)| acc ^ column);
    let corrected = if syndrome == 0 {
        0
    } else {
        let position = PARITY_CHECK.iter().position(|&column| column == syndrome)?;
        code[position] = !code[position];
        1
    };
    let nibble = (0..4).fold(0u8, |acc, k| (acc << 1) | u8::from(code[k]));
    Some((nibble, corrected))
}

pub(crate) fn interleaved_index(codewords: usize, codeword: usize, bit: usize) -> usize {
    codewords * bit + codeword
}

pub(crate) fn float24(value: u32) -> f64 {
    let exponent = (value >> 20) & 0xF;
    f64::from(value & 0xF_FFFF) / f64::from(1u32 << exponent)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ThermistorLayout {
    pub feedback_ohm: f64,
    pub channels: [usize; 3],
    pub battery: usize,
}

pub(crate) fn layout(kind: u8, meas: &[f64; CHANNELS]) -> Option<ThermistorLayout> {
    let dfm09 = |channels, battery| ThermistorLayout {
        feedback_ohm: 220e3,
        channels,
        battery,
    };
    let dfm17 = |channels, battery| ThermistorLayout {
        feedback_ohm: 332e3,
        channels,
        battery,
    };
    match kind {
        0xA => Some(dfm09([0, 3, 4], 5)),
        0xB => Some(dfm17([0, 3, 4], 5)),
        0xC if meas[6] < 220e3 => Some(dfm09([1, 5, 6], 7)),
        0xC => Some(dfm17([0, 3, 4], 5)),
        0xD => Some(dfm17([1, 5, 6], 7)),
        _ => None,
    }
}

fn bits_of(nibbles: &[u8]) -> u64 {
    nibbles
        .iter()
        .fold(0u64, |acc, &nibble| (acc << 4) | u64::from(nibble))
}

fn field(bits: u64, total: u32, start: u32, len: u32) -> u32 {
    ((bits >> (total - start - len)) & ((1u64 << len) - 1)) as u32
}

#[derive(Default)]
struct Config {
    meas: [f64; CHANNELS],
    raw: [u32; CHANNELS],
    have: u16,
    serial_channel: Option<u8>,
    halves: [Option<u16>; 2],
    last_assembled: Option<u32>,
    serial: Option<u32>,
}

impl Config {
    fn push(&mut self, nibbles: &[u8; CONF_CODEWORDS]) {
        let id = usize::from(nibbles[0]);
        let value = (bits_of(&nibbles[1..]) & 0xFF_FFFF) as u32;
        self.meas[id] = float24(value);
        self.raw[id] = value;
        self.have |= 1 << id;
        if id > 5 && nibbles[1] == SERIAL_MARK {
            self.serial_half(nibbles[0], value & 0xF_FFFF);
        }
    }

    fn serial_half(&mut self, channel: u8, value: u32) {
        let half = (value & 0xF) as usize;
        if half > 1 {
            return;
        }
        if self.serial_channel != Some(channel) {
            self.halves = [None; 2];
            self.serial_channel = Some(channel);
        }
        self.halves[half] = Some((value >> 4) as u16);
        let [Some(high), Some(low)] = self.halves else {
            return;
        };
        self.halves = [None; 2];
        let assembled = (u32::from(high) << 16) | u32::from(low);
        let confirmed = self.last_assembled.is_none_or(|last| last == assembled);
        self.last_assembled = Some(assembled);
        if confirmed {
            self.serial = Some(assembled);
        } else {
            self.serial = None;
            self.have = 0;
        }
    }

    fn has(&self, channel: usize) -> bool {
        self.have & (1 << channel) != 0
    }

    fn layout(&self) -> Option<ThermistorLayout> {
        layout(self.serial_channel?, &self.meas)
    }

    fn temperature(&self) -> Option<f64> {
        let layout = self.layout()?;
        if !layout.channels.iter().all(|&c| self.has(c)) {
            return None;
        }
        let [signal, low, high] = layout.channels.map(|c| self.meas[c]);
        let gain = high / layout.feedback_ohm;
        let celsius = steinhart_hart(THERMISTOR, (signal - low) / gain)?;
        (-120.0..=80.0).contains(&celsius).then_some(celsius)
    }

    fn battery(&self) -> Option<f64> {
        let layout = self.layout()?;
        self.has(layout.battery)
            .then(|| f64::from((self.raw[layout.battery] >> 4) & 0xFFFF) / 1_000.0)
    }
}

#[derive(Default)]
struct Cycle {
    mode: u8,
    frame: Option<u32>,
    millis: Option<u32>,
    lat: Option<f64>,
    lon: Option<f64>,
    alt: Option<f64>,
    speed: Option<f64>,
    heading: Option<f64>,
    climb: Option<f64>,
    corrected: u32,
}

impl Cycle {
    fn gps(&mut self, id: u8, bits: u64) {
        let at = |start, len| field(bits, 52, start, len);
        let signed16 = |start| f64::from(at(start, 16) as u16 as i16) / 100.0;
        let signed32 = |start| f64::from(at(start, 32) as i32);
        let compact = self.mode >= MODE_COMPACT;
        match (compact, id) {
            (_, 0) => {
                let mode = at(16, 8) as u8;
                self.mode = if (MODE_GPS..=4).contains(&mode) {
                    mode
                } else {
                    MODE_GPS
                };
                if self.mode >= MODE_COMPACT {
                    self.millis = Some(at(0, 16));
                    self.speed = Some(signed16(32));
                } else {
                    self.frame = Some(at(24, 8));
                }
            }
            (false, 1) => self.millis = Some(at(32, 16)),
            (false, 2) | (true, 1) => {
                self.lat = Some(signed32(0) / 1e7);
                if compact {
                    self.heading = Some(f64::from(at(32, 16)) / 100.0);
                } else {
                    self.speed = Some(signed16(32));
                }
            }
            (false, 3) | (true, 2) => {
                self.lon = Some(signed32(0) / 1e7);
                if compact {
                    self.climb = Some(signed16(32));
                } else {
                    self.heading = Some(f64::from(at(32, 16)) / 100.0);
                }
            }
            (false, 4) | (true, 3) => {
                self.alt = Some(signed32(0) / 100.0);
                if !compact {
                    self.climb = Some(signed16(32));
                }
            }
            _ => {}
        }
    }

    fn time(&self, bits: u64) -> Option<String> {
        let at = |start, len| field(bits, 52, start, len);
        let seconds = self.millis? / 1_000;
        civil_time(
            i16::try_from(at(0, 12)).ok()?,
            at(12, 4) as i8,
            at(16, 5) as i8,
            at(21, 5) as i8,
            at(26, 6) as i8,
            i8::try_from(seconds).ok()?,
        )
    }

    fn position(&self) -> Option<Geodetic> {
        let position = Geodetic {
            lat: self.lat?,
            lon: self.lon?,
            alt: self.alt?,
        };
        plausible(position).then_some(position)
    }
}

pub(crate) struct DfmFrames {
    config: Config,
    cycle: Cycle,
    rejected: u32,
}

impl DfmFrames {
    pub(crate) fn new() -> Self {
        Self {
            config: Config::default(),
            cycle: Cycle {
                mode: MODE_GPS,
                ..Cycle::default()
            },
            rejected: 0,
        }
    }

    pub(crate) fn rejected(&self) -> u32 {
        self.rejected
    }

    pub(crate) fn decode(&mut self, payload: &[bool; PAYLOAD_BITS]) -> Option<RadiosondeFrame> {
        let conf_end = 8 * CONF_CODEWORDS;
        let dat_len = 8 * DAT_CODEWORDS;
        if let Some((nibbles, corrected)) = self.section::<CONF_CODEWORDS>(&payload[..conf_end]) {
            self.cycle.corrected += corrected;
            self.config.push(&nibbles);
        }
        let mut emitted = None;
        for start in [conf_end, conf_end + dat_len] {
            let Some((nibbles, corrected)) =
                self.section::<DAT_CODEWORDS>(&payload[start..start + dat_len])
            else {
                continue;
            };
            self.cycle.corrected += corrected;
            if let Some(frame) = self.data(&nibbles) {
                emitted = Some(frame);
            }
        }
        emitted
    }

    fn section<const N: usize>(&mut self, bits: &[bool]) -> Option<([u8; N], u32)> {
        let mut nibbles = [0u8; N];
        let mut corrected = 0;
        for (codeword, nibble) in nibbles.iter_mut().enumerate() {
            let code = std::array::from_fn(|bit| bits[interleaved_index(N, codeword, bit)]);
            let Some((value, fixed)) = hamming_decode(code) else {
                self.rejected += 1;
                return None;
            };
            *nibble = value;
            corrected += fixed;
        }
        Some((nibbles, corrected))
    }

    fn data(&mut self, nibbles: &[u8; DAT_CODEWORDS]) -> Option<RadiosondeFrame> {
        let bits = bits_of(nibbles);
        let id = nibbles[12];
        if id != 8 {
            self.cycle.gps(id, bits);
            return None;
        }
        let mode = self.cycle.mode;
        let cycle = std::mem::replace(
            &mut self.cycle,
            Cycle {
                mode,
                ..Cycle::default()
            },
        );
        let serial = self.config.serial?;
        let position = cycle.position()?;
        let mut out = empty_frame(SondeType::Dfm, serial.to_string());
        set_position(&mut out, position);
        out.frame = cycle.frame;
        out.time = cycle.time(bits);
        out.speed_ms = cycle.speed;
        out.heading_deg = cycle.heading;
        out.climb_ms = cycle.climb;
        out.satellites = Some(field(bits, 52, 32, 8) as u8);
        out.temperature_c = self.config.temperature().and_then(finite_f32);
        out.battery_v = self.config.battery().and_then(finite_f32);
        out.errors_corrected = cycle.corrected;
        Some(out)
    }
}

enum Receive {
    Hunt,
    Collect { polarity: Polarity, symbols: usize },
}

pub(crate) struct Dfm {
    clock: SymbolClock,
    sync: SyncMatcher,
    receive: Receive,
    raw: [f32; 2 * PAYLOAD_BITS],
    payload: [bool; PAYLOAD_BITS],
    frames: DfmFrames,
}

impl Dfm {
    pub(crate) fn new() -> Self {
        Self {
            clock: SymbolClock::new(BAUD, SMOOTHING),
            sync: SyncMatcher::from_bits(RAW_HEADER, SYNC_TOLERANCE),
            receive: Receive::Hunt,
            raw: [0.0; 2 * PAYLOAD_BITS],
            payload: [false; PAYLOAD_BITS],
            frames: DfmFrames::new(),
        }
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
        match &mut self.receive {
            Receive::Hunt => {
                if let Some(polarity) = self.sync.push(symbol > 0.0) {
                    self.receive = Receive::Collect {
                        polarity,
                        symbols: 0,
                    };
                }
            }
            Receive::Collect { polarity, symbols } => {
                self.raw[*symbols] = symbol;
                *symbols += 1;
                if *symbols < self.raw.len() {
                    return;
                }
                let polarity = *polarity;
                self.receive = Receive::Hunt;
                self.sync.clear();
                for (bit, pair) in self.payload.iter_mut().zip(self.raw.as_chunks::<2>().0) {
                    *bit = polarity.apply(pair[1] > pair[0]);
                }
                if let Some(mut frame) = self.frames.decode(&self.payload) {
                    frame.rejected = self.frames.rejected();
                    out.push(DecoderEvent::Radiosonde(frame));
                }
            }
        }
    }
}
