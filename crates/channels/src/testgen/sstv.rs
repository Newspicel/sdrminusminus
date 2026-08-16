use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_wire::SstvMode;

use crate::sstv::modes::{
    BREAK_MS, HIGH_SEPARATOR_HZ, LEADER_HZ, LEADER_MS, PORCH_HZ, Part, SYNC_HZ, Scan, VIS_BIT_MS,
    VIS_ONE_HZ, VIS_ZERO_HZ, level_to_hz, timing,
};

pub struct Frame {
    pub width: u16,
    pub height: u16,
    pub rgb: Vec<u8>,
}

impl Frame {
    #[must_use]
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            rgb: vec![0; usize::from(width) * usize::from(height) * 3],
        }
    }

    #[must_use]
    pub fn pixel(&self, x: u16, y: u16) -> [u8; 3] {
        let base = (usize::from(y) * usize::from(self.width) + usize::from(x)) * 3;
        [self.rgb[base], self.rgb[base + 1], self.rgb[base + 2]]
    }

    pub fn set(&mut self, x: u16, y: u16, rgb: [u8; 3]) {
        let base = (usize::from(y) * usize::from(self.width) + usize::from(x)) * 3;
        self.rgb[base..base + 3].copy_from_slice(&rgb);
    }

    fn component(&self, x: u16, row: u16, scan: Scan, rows: u16) -> u8 {
        let x = x.min(self.width.saturating_sub(1));
        let clamp_row = |y: u16| y.min(self.height.saturating_sub(1));
        match scan {
            Scan::Red | Scan::Green | Scan::Blue | Scan::Luma(_) => {
                let offset = match scan {
                    Scan::Luma(index) => u16::from(index),
                    _ => 0,
                };
                let [r, g, b] = self.pixel(x, clamp_row(row + offset));
                match scan {
                    Scan::Red => r,
                    Scan::Green => g,
                    Scan::Blue => b,
                    _ => luma(r, g, b),
                }
            }
            Scan::ChromaR | Scan::ChromaB => {
                let mut sum = 0u32;
                for offset in 0..rows.max(1) {
                    let [r, g, b] = self.pixel(x, clamp_row(row + offset));
                    sum += u32::from(if scan == Scan::ChromaR {
                        chroma_r(r, g, b)
                    } else {
                        chroma_b(r, g, b)
                    });
                }
                (sum / u32::from(rows.max(1))) as u8
            }
            Scan::ChromaAlternating => 128,
        }
    }
}

fn luma(r: u8, g: u8, b: u8) -> u8 {
    clamp8(0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b))
}

fn chroma_r(r: u8, g: u8, b: u8) -> u8 {
    clamp8(128.0 + 0.5 * f32::from(r) - 0.418_688 * f32::from(g) - 0.081_312 * f32::from(b))
}

fn chroma_b(r: u8, g: u8, b: u8) -> u8 {
    clamp8(128.0 - 0.168_736 * f32::from(r) - 0.331_264 * f32::from(g) + 0.5 * f32::from(b))
}

fn clamp8(value: f32) -> u8 {
    value.clamp(0.0, 255.0) as u8
}

#[must_use]
pub fn bars(mode: SstvMode) -> Frame {
    let (width, height) = mode.size();
    let mut frame = Frame::new(width, height);
    const PALETTE: [[u8; 3]; 8] = [
        [255, 255, 255],
        [255, 255, 0],
        [0, 255, 255],
        [0, 255, 0],
        [255, 0, 255],
        [255, 0, 0],
        [0, 0, 255],
        [0, 0, 0],
    ];
    for y in 0..height {
        for x in 0..width {
            let bar = usize::from(x) * PALETTE.len() / usize::from(width);
            frame.set(x, y, PALETTE[bar.min(PALETTE.len() - 1)]);
        }
    }
    frame
}

struct Writer {
    rate: f64,
    phase: f64,
    carry: f64,
    out: Vec<Complex<f32>>,
}

impl Writer {
    fn new(rate: f64) -> Self {
        Self {
            rate,
            phase: 0.0,
            carry: 0.0,
            out: Vec::new(),
        }
    }

    fn tone(&mut self, hz: f64, ms: f64) {
        self.carry += ms * self.rate / 1_000.0;
        let count = self.carry.round() as usize;
        self.carry -= count as f64;
        self.emit(hz, count);
    }

    fn emit(&mut self, hz: f64, count: usize) {
        let step = TAU * hz / self.rate;
        for _ in 0..count {
            self.out.push(Complex::from_polar(1.0, self.phase as f32));
            self.phase += step;
            if self.phase > TAU {
                self.phase -= TAU;
            }
        }
    }

    fn sweep(&mut self, levels: &[u8], ms: f64) {
        let total = ms * self.rate / 1_000.0 + self.carry;
        let count = total.round() as usize;
        self.carry = total - count as f64;
        for index in 0..count {
            let pixel = index * levels.len() / count.max(1);
            let hz = level_to_hz(levels[pixel.min(levels.len() - 1)]);
            self.emit(hz, 1);
        }
    }
}

fn vis_bits(mode: SstvMode) -> [bool; 8] {
    let code = mode.vis();
    let mut bits = [false; 8];
    for (index, bit) in bits.iter_mut().enumerate().take(7) {
        *bit = code & (1 << index) != 0;
    }
    bits[7] = bits[..7].iter().filter(|&&bit| bit).count() % 2 == 1;
    bits
}

#[must_use]
pub fn header(mode: SstvMode, rate: f64) -> Vec<Complex<f32>> {
    let mut writer = Writer::new(rate);
    write_header(&mut writer, mode);
    writer.out
}

fn write_header(writer: &mut Writer, mode: SstvMode) {
    writer.tone(LEADER_HZ, LEADER_MS);
    writer.tone(SYNC_HZ, BREAK_MS);
    writer.tone(LEADER_HZ, LEADER_MS);
    writer.tone(SYNC_HZ, VIS_BIT_MS);
    for bit in vis_bits(mode) {
        writer.tone(if bit { VIS_ONE_HZ } else { VIS_ZERO_HZ }, VIS_BIT_MS);
    }
    writer.tone(SYNC_HZ, VIS_BIT_MS);
}

#[must_use]
pub fn transmission(mode: SstvMode, frame: &Frame, rate: f64) -> Vec<Complex<f32>> {
    let plan = timing(mode);
    let mut writer = Writer::new(rate);
    write_header(&mut writer, mode);
    if plan.lead_in_ms > 0.0 {
        writer.tone(SYNC_HZ, plan.lead_in_ms);
    }
    let width = usize::from(frame.width);
    let mut levels = vec![0u8; width];
    for line in 0..plan.lines() {
        let row = line * plan.rows_per_line;
        for segment in plan.segments {
            match segment.part {
                Part::Sync => writer.tone(SYNC_HZ, segment.ms),
                Part::Gap(hz) => writer.tone(hz, segment.ms),
                Part::AlternatingGap => {
                    let hz = if line.is_multiple_of(2) {
                        PORCH_HZ
                    } else {
                        HIGH_SEPARATOR_HZ
                    };
                    writer.tone(hz, segment.ms);
                }
                Part::Scan(scan) => {
                    let scan = resolve(scan, line);
                    for (x, level) in levels.iter_mut().enumerate() {
                        *level = frame.component(x as u16, row, scan, plan.rows_per_line);
                    }
                    writer.sweep(&levels, segment.ms);
                }
            }
        }
    }
    writer.out
}

fn resolve(scan: Scan, line: u16) -> Scan {
    match scan {
        Scan::ChromaAlternating if line.is_multiple_of(2) => Scan::ChromaR,
        Scan::ChromaAlternating => Scan::ChromaB,
        other => other,
    }
}
