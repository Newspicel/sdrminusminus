use std::{f32::consts::TAU, sync::Arc};

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use super::{DecodeError, p1_tables::*};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preamble {
    pub s1: u8,
    pub s2: u8,
}

impl Preamble {
    pub fn fft(self) -> Result<usize, DecodeError> {
        let lite = self.lite();
        if !matches!(self.s1, 0 | 1 | 3 | 4) || self.s2 > 15 {
            return Err(DecodeError::Parameters);
        }
        match self.s2 >> 1 {
            0 => Ok(2048),
            1 | 6 => Ok(8192),
            2 => Ok(4096),
            3 if !lite => Ok(1024),
            3 | 4 => Ok(16384),
            5 | 7 if !lite => Ok(32768),
            _ => Err(DecodeError::Parameters),
        }
    }

    pub const fn lite(self) -> bool {
        matches!(self.s1, 3 | 4)
    }

    pub const fn miso(self) -> bool {
        matches!(self.s1, 1 | 4)
    }

    pub fn p2_symbols(self) -> Result<usize, DecodeError> {
        Ok((16384 / self.fft()?).max(1))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Detection {
    pub start: usize,
    pub frequency: f32,
    pub confidence: f32,
    pub preamble: Preamble,
}

pub struct Acquisition {
    fft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    spectrum: Vec<Complex<f32>>,
    phase: [Complex<f32>; 1024],
    signs: [f32; 384],
}

impl Default for Acquisition {
    fn default() -> Self {
        let fft = FftPlanner::new().plan_fft_forward(1024);
        let mut state = 0x4e46_u16;
        Self {
            scratch: vec![Complex::default(); fft.get_inplace_scratch_len()],
            spectrum: vec![Complex::default(); 1024],
            fft,
            phase: std::array::from_fn(|i| Complex::from_polar(1.0, TAU * i as f32 / 1024.0)),
            signs: std::array::from_fn(|_| {
                let bit = (state ^ (state >> 1)) & 1;
                state = (state >> 1) | (bit << 14);
                if bit == 0 { 1.0 } else { -1.0 }
            }),
        }
    }
}

impl Acquisition {
    pub fn find(&mut self, iq: &[Complex<f32>]) -> Option<Detection> {
        if iq.len() < 2048 {
            return None;
        }
        let mut c = Complex::<f32>::default();
        let mut b = Complex::<f32>::default();
        let mut power = [0.0_f32; 4];
        for i in 0..542 {
            c += self.correlation_c(iq, i);
            power[0] += iq[i].norm_sqr();
            power[1] += iq[i + 542].norm_sqr();
        }
        for i in 0..482 {
            b += self.correlation_b(iq, i);
            power[2] += iq[i + 1084].norm_sqr();
            power[3] += iq[i + 1566].norm_sqr();
        }
        let mut peak = (0, 0.0_f32, 0.0_f32);
        for at in 0..=iq.len() - 2048 {
            let denominator = power.iter().product::<f32>();
            let quality = if denominator > 1e-20 {
                (c.norm_sqr() * b.norm_sqr() / denominator).sqrt()
            } else {
                0.0
            };
            if quality > 0.45 && quality > peak.1 {
                peak = (at, quality, (c * b).arg() / 1024.0);
            }
            if peak.1 > 0.0 && (at >= peak.0 + 24 || at == iq.len() - 2048) {
                if let Some(mut detected) = self.decode(&iq[peak.0..peak.0 + 2048], peak.2) {
                    detected.start = peak.0;
                    return Some(detected);
                }
                peak.1 = 0.0;
            }
            if at + 2048 < iq.len() {
                c += self.correlation_c(iq, at + 542) - self.correlation_c(iq, at);
                b += self.correlation_b(iq, at + 482) - self.correlation_b(iq, at);
                for (p, (first, last)) in power.iter_mut().zip([
                    (at, at + 542),
                    (at + 542, at + 1084),
                    (at + 1084, at + 1566),
                    (at + 1566, at + 2048),
                ]) {
                    *p += iq[last].norm_sqr() - iq[first].norm_sqr();
                }
            }
        }
        None
    }

    fn correlation_c(&self, iq: &[Complex<f32>], at: usize) -> Complex<f32> {
        iq[at + 542] * iq[at].conj() * self.phase[at % 1024]
    }

    fn correlation_b(&self, iq: &[Complex<f32>], at: usize) -> Complex<f32> {
        iq[at + 1566] * iq[at + 1084].conj() * self.phase[(at + 1566) % 1024].conj()
    }

    pub fn decode(&mut self, iq: &[Complex<f32>], fractional: f32) -> Option<Detection> {
        if iq.len() != 2048
            || !fractional.is_finite()
            || iq.iter().any(|p| !p.norm_sqr().is_finite())
        {
            return None;
        }
        for (i, p) in self.spectrum.iter_mut().enumerate() {
            *p = iq[i + 542] * Complex::from_polar(1.0, -fractional * i as f32);
        }
        self.fft
            .process_with_scratch(&mut self.spectrum, &mut self.scratch);
        let mut best = None;
        let mut confidence = 0.55;
        for offset in -64_isize..=64 {
            let cells: [Complex<f32>; 384] = std::array::from_fn(|i| {
                let bin = (P1_ACTIVE_CARRIERS[i] as isize - 426 + offset).rem_euclid(1024) as usize;
                self.spectrum[bin] * self.signs[i]
            });
            let soft: [f32; 384] = std::array::from_fn(|i| {
                if i == 0 {
                    0.0
                } else {
                    (cells[i] * cells[i - 1].conj()).re
                }
            });
            let (s1, q1) = strongest(&S1_PATTERNS, &soft[1..64], 1, Some(&soft[320..384]));
            let (s2, q2) = strongest(&S2_PATTERNS, &soft[64..320], 0, None);
            let quality = q1.min(q2);
            if quality > confidence {
                confidence = quality;
                best = Some(Detection {
                    start: 0,
                    frequency: fractional + TAU * offset as f32 / 1024.0,
                    confidence: quality,
                    preamble: Preamble {
                        s1: s1 as u8,
                        s2: s2 as u8,
                    },
                });
            }
        }
        best
    }
}

fn strongest<const N: usize, const B: usize>(
    patterns: &[[u8; B]; N],
    soft: &[f32],
    start: usize,
    repeated: Option<&[f32]>,
) -> (usize, f32) {
    let total = soft
        .iter()
        .chain(repeated.unwrap_or(&[]))
        .map(|v| v.abs())
        .sum::<f32>()
        .max(1e-20);
    patterns
        .iter()
        .enumerate()
        .map(|(index, pattern)| {
            let correlate = |values: &[f32], first: usize| {
                values
                    .iter()
                    .enumerate()
                    .map(|(i, value)| {
                        let bit = (pattern[(i + first) / 8] >> (7 - (i + first) % 8)) & 1;
                        value * if bit == 0 { 1.0 } else { -1.0 }
                    })
                    .sum::<f32>()
            };
            (
                index,
                (correlate(soft, start) + repeated.map_or(0.0, |values| correlate(values, 0)))
                    / total,
            )
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or((0, 0.0))
}
