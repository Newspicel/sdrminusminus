use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::crc16_msb;
use sdrmm_wire::{DecoderEvent, RadiosondeFrame, SondeType};

use super::{
    demod::RATE,
    fields::{Geodetic, Motion, empty_frame, plausible, set_motion, set_position, today_at},
};

pub(crate) const BAUD: f64 = 1_200.0;
pub(crate) const MARK_HZ: f64 = 1_200.0;
pub(crate) const SPACE_HZ: f64 = 2_200.0;
pub(crate) const SOH: u8 = 0x01;
pub(crate) const PACKET_PTU: u8 = 0x01;
pub(crate) const PACKET_GPS: u8 = 0x02;
pub(crate) const PACKET_XDATA: u8 = 0x03;
pub(crate) const PACKET_EXTENDED_PTU: u8 = 0x04;
pub(crate) const PACKET_EXTENDED_GPS: u8 = 0x05;
pub(crate) const ALTITUDE_OFFSET_M: f64 = 5_000.0;
pub(crate) const CRC_INIT: u16 = 0x1D0F;

const CRC_POLY: u16 = 0x1021;
const BIT_SAMPLES: usize = 40;
const TONE_TABLE: usize = 240;
const BUFFER: usize = 64;
const SECONDS_PER_DAY: i64 = 86_400;
const FNV_OFFSET: u32 = 0x811C_9DC5;
const FNV_PRIME: u32 = 0x0100_0193;

pub(crate) fn crc(data: &[u8]) -> u16 {
    crc16_msb(CRC_POLY, CRC_INIT, data)
}

pub(crate) fn packet_len(id: u8, third: Option<u8>) -> Option<usize> {
    match id {
        PACKET_PTU => Some(14),
        PACKET_GPS => Some(18),
        PACKET_EXTENDED_PTU => Some(20),
        PACKET_EXTENDED_GPS => Some(30),
        PACKET_XDATA => third.map(|n| usize::from(n) + 5),
        _ => None,
    }
}

pub(crate) fn serial_for(power_on_second_of_day: i64) -> String {
    let hash = power_on_second_of_day
        .to_le_bytes()
        .iter()
        .fold(FNV_OFFSET, |acc, &byte| {
            (acc ^ u32::from(byte)).wrapping_mul(FNV_PRIME)
        });
    format!("iMet-{hash:08X}")
}

struct Afsk {
    mark: [Complex<f32>; TONE_TABLE],
    space: [Complex<f32>; TONE_TABLE],
    history_mark: [Complex<f32>; BIT_SAMPLES],
    history_space: [Complex<f32>; BIT_SAMPLES],
    sum_mark: Complex<f64>,
    sum_space: Complex<f64>,
    phase: usize,
    slot: usize,
}

fn tone_table(freq_hz: f64) -> [Complex<f32>; TONE_TABLE] {
    std::array::from_fn(|n| {
        let angle = -TAU * freq_hz * n as f64 / RATE;
        Complex::new(angle.cos() as f32, angle.sin() as f32)
    })
}

fn widen(z: Complex<f32>) -> Complex<f64> {
    Complex::new(f64::from(z.re), f64::from(z.im))
}

impl Afsk {
    fn new() -> Self {
        Self {
            mark: tone_table(MARK_HZ),
            space: tone_table(SPACE_HZ),
            history_mark: [Complex::new(0.0, 0.0); BIT_SAMPLES],
            history_space: [Complex::new(0.0, 0.0); BIT_SAMPLES],
            sum_mark: Complex::new(0.0, 0.0),
            sum_space: Complex::new(0.0, 0.0),
            phase: 0,
            slot: 0,
        }
    }

    fn push(&mut self, x: f32) -> bool {
        let mark = self.mark[self.phase] * x;
        let space = self.space[self.phase] * x;
        self.sum_mark += widen(mark) - widen(self.history_mark[self.slot]);
        self.sum_space += widen(space) - widen(self.history_space[self.slot]);
        self.history_mark[self.slot] = mark;
        self.history_space[self.slot] = space;
        self.slot = (self.slot + 1) % BIT_SAMPLES;
        self.phase = (self.phase + 1) % TONE_TABLE;
        self.sum_mark.norm_sqr() > self.sum_space.norm_sqr()
    }
}

enum Uart {
    Idle { previous: bool },
    Byte { wait: usize, index: u8, value: u8 },
}

impl Uart {
    fn push(&mut self, mark: bool) -> Option<u8> {
        match self {
            Self::Idle { previous } => {
                if *previous && !mark {
                    *self = Self::Byte {
                        wait: BIT_SAMPLES / 2,
                        index: 0,
                        value: 0,
                    };
                } else {
                    *previous = mark;
                }
                None
            }
            Self::Byte { wait, index, value } => {
                *wait -= 1;
                if *wait > 0 {
                    return None;
                }
                *wait = BIT_SAMPLES;
                match *index {
                    0 if mark => {
                        *self = Self::Idle { previous: true };
                        return None;
                    }
                    1..=8 => *value |= u8::from(mark) << (*index - 1),
                    9 => {
                        let byte = *value;
                        *self = Self::Idle { previous: mark };
                        return mark.then_some(byte);
                    }
                    _ => {}
                }
                *index += 1;
                None
            }
        }
    }
}

#[derive(Clone, Copy)]
struct GpsFix {
    position: Geodetic,
    motion: Option<Motion>,
    satellites: u8,
    clock: [u8; 3],
}

fn f32_le(bytes: &[u8], at: usize) -> f64 {
    f64::from(f32::from_le_bytes([
        bytes[at],
        bytes[at + 1],
        bytes[at + 2],
        bytes[at + 3],
    ]))
}

fn gps_packet(packet: &[u8]) -> Option<GpsFix> {
    let extended = packet[1] == PACKET_EXTENDED_GPS;
    let altitude = f64::from(u16::from_le_bytes([packet[10], packet[11]])) - ALTITUDE_OFFSET_M;
    let position = Geodetic {
        lat: f32_le(packet, 2),
        lon: f32_le(packet, 6),
        alt: altitude,
    };
    let clock_at = if extended { 25 } else { 13 };
    let clock = [packet[clock_at], packet[clock_at + 1], packet[clock_at + 2]];
    if !plausible(position) || clock[0] > 23 || clock[1] > 59 || clock[2] > 59 {
        return None;
    }
    let motion = extended
        .then(|| Motion::from_enu(f32_le(packet, 13), f32_le(packet, 17), f32_le(packet, 21)));
    Some(GpsFix {
        position,
        motion,
        satellites: packet[12],
        clock,
    })
}

pub(crate) struct ImetPackets {
    fresh_gps: Option<GpsFix>,
    serial: Option<String>,
    rejected: u32,
}

impl ImetPackets {
    pub(crate) fn new() -> Self {
        Self {
            fresh_gps: None,
            serial: None,
            rejected: 0,
        }
    }

    pub(crate) fn rejected(&self) -> u32 {
        self.rejected
    }

    pub(crate) fn reject(&mut self) {
        self.rejected += 1;
    }

    pub(crate) fn packet(&mut self, packet: &[u8]) -> Option<RadiosondeFrame> {
        match packet[1] {
            PACKET_GPS | PACKET_EXTENDED_GPS => {
                self.fresh_gps = gps_packet(packet);
                if self.fresh_gps.is_none() {
                    self.rejected += 1;
                }
                None
            }
            PACKET_PTU | PACKET_EXTENDED_PTU => self.ptu(packet),
            _ => None,
        }
    }

    fn ptu(&mut self, packet: &[u8]) -> Option<RadiosondeFrame> {
        let number = u16::from_le_bytes([packet[2], packet[3]]);
        let fix = self.fresh_gps.take();
        if let Some(fix) = fix {
            let [hour, minute, second] = fix.clock.map(i64::from);
            let now = hour * 3_600 + minute * 60 + second;
            self.serial = Some(serial_for(
                (now - i64::from(number)).rem_euclid(SECONDS_PER_DAY),
            ));
        }
        let mut out = empty_frame(SondeType::Imet4, self.serial.clone()?);
        out.frame = Some(u32::from(number));
        let pressure = u32::from_le_bytes([packet[4], packet[5], packet[6], 0]);
        out.pressure_hpa = Some(pressure as f32 / 100.0);
        out.temperature_c = Some(f32::from(i16::from_le_bytes([packet[7], packet[8]])) / 100.0);
        out.humidity_pct = Some(f32::from(u16::from_le_bytes([packet[9], packet[10]])) / 100.0);
        out.battery_v = Some(f32::from(packet[11]) / 10.0);
        if let Some(fix) = fix {
            set_position(&mut out, fix.position);
            if let Some(motion) = fix.motion {
                set_motion(&mut out, motion);
            }
            out.satellites = Some(fix.satellites);
            out.time = today_at(fix.clock[0], fix.clock[1], fix.clock[2]);
        }
        Some(out)
    }
}

pub(crate) struct Imet {
    afsk: Afsk,
    uart: Uart,
    buffer: [u8; BUFFER],
    len: usize,
    packets: ImetPackets,
}

impl Imet {
    pub(crate) fn new() -> Self {
        Self {
            afsk: Afsk::new(),
            uart: Uart::Idle { previous: true },
            buffer: [0; BUFFER],
            len: 0,
            packets: ImetPackets::new(),
        }
    }

    pub(crate) fn rejected(&self) -> u32 {
        self.packets.rejected()
    }

    pub(crate) fn push(&mut self, audio: &[f32], out: &mut Vec<DecoderEvent>) {
        for &sample in audio {
            let mark = self.afsk.push(sample);
            if let Some(byte) = self.uart.push(mark) {
                self.byte(byte, out);
            }
        }
    }

    fn byte(&mut self, byte: u8, out: &mut Vec<DecoderEvent>) {
        if self.len == BUFFER {
            self.consume(1);
        }
        self.buffer[self.len] = byte;
        self.len += 1;
        while let Some(step) = self.scan(out) {
            self.consume(step);
        }
    }

    fn scan(&mut self, out: &mut Vec<DecoderEvent>) -> Option<usize> {
        if self.len == 0 {
            return None;
        }
        if self.buffer[0] != SOH {
            return Some(1);
        }
        if self.len < 3 {
            return None;
        }
        let Some(need) = packet_len(self.buffer[1], Some(self.buffer[2])) else {
            return Some(1);
        };
        if need > BUFFER {
            return Some(1);
        }
        if self.len < need {
            return None;
        }
        let packet = &self.buffer[..need];
        let stored = u16::from_be_bytes([packet[need - 2], packet[need - 1]]);
        if crc(&packet[..need - 2]) != stored {
            self.packets.reject();
            return Some(1);
        }
        if let Some(mut frame) = self.packets.packet(packet) {
            frame.rejected = self.packets.rejected();
            out.push(DecoderEvent::Radiosonde(frame));
        }
        Some(need)
    }

    fn consume(&mut self, count: usize) {
        self.buffer.copy_within(count..self.len, 0);
        self.len -= count;
    }
}
