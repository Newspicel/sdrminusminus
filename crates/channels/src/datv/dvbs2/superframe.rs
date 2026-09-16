use num_complex::Complex;

use super::pl;

pub const LENGTH: usize = 612_540;
const SOSF: usize = 270;
const HEADER: usize = 720;
const PILOT_START: usize = 1440;
const PILOT_PERIOD: usize = 1476;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    Plain,
    Legacy,
    Extended,
    Unsupported(u8),
}

#[derive(Clone, Copy)]
struct Active {
    content: Content,
    position: usize,
    pilots: bool,
    phase: f32,
    frequency: f32,
}

pub struct Container {
    pending: Vec<Complex<f32>>,
    offset: usize,
    active: Option<Active>,
    reference: Vec<Complex<f32>>,
}

fn sequence() -> Vec<Complex<f32>> {
    let mut x = vec![0u8; (1 << 20) - 1];
    let mut y = vec![1u8; x.len()];
    x[0] = 1;
    for i in 20..x.len() {
        x[i] = x[i - 17] ^ x[i - 20];
        y[i] = y[i - 3] ^ y[i - 9] ^ y[i - 18] ^ y[i - 20];
    }
    (0..LENGTH)
        .map(|i| {
            let shifted = (i + 524_288) % x.len();
            pl::rotate(
                pl::pilot_symbol(),
                2 * (x[shifted] ^ y[shifted]) + (x[i] ^ y[i]),
            )
        })
        .collect()
}

fn fit(symbols: &[Complex<f32>], reference: &[Complex<f32>]) -> Option<(f32, f32)> {
    let mut lag = Complex::new(0.0, 0.0);
    let mut last = Complex::new(0.0, 0.0);
    let mut power = 0.0;
    for (&symbol, &known) in symbols.iter().zip(reference) {
        let value = symbol * known.conj();
        lag += value * last.conj();
        last = value;
        power += symbol.norm_sqr();
    }
    if lag.norm_sqr() < 0.45 * power * power {
        return None;
    }
    let frequency = lag.arg();
    let sum: Complex<f32> = symbols
        .iter()
        .zip(reference)
        .enumerate()
        .map(|(i, (&symbol, &known))| {
            symbol * known.conj() * Complex::from_polar(1.0, -frequency * i as f32)
        })
        .sum();
    let coherence = sum.norm_sqr() / (power * symbols.len() as f32).max(1e-12);
    (coherence > 0.65).then(|| (sum.arg(), frequency))
}

fn format(
    symbols: &[Complex<f32>],
    reference: &[Complex<f32>],
    phase: f32,
    frequency: f32,
) -> Option<u8> {
    let mut bits = [0.0f32; 15];
    for (i, (&symbol, &known)) in symbols
        .iter()
        .zip(reference)
        .enumerate()
        .skip(SOSF)
        .take(450)
    {
        bits[(i - SOSF) / 30] +=
            (symbol * known.conj() * Complex::from_polar(1.0, -phase - frequency * i as f32)).re;
    }
    let total: f32 = bits.iter().map(|v| v.abs()).sum();
    let mut best = (0, f32::NEG_INFINITY);
    for code in 0..16u8 {
        let score: f32 = bits
            .iter()
            .enumerate()
            .map(|(i, bit)| {
                if (code & (i + 1) as u8).count_ones().is_multiple_of(2) {
                    *bit
                } else {
                    -*bit
                }
            })
            .sum();
        if score > best.1 {
            best = (code, score);
        }
    }
    (best.1 > total * 0.8 && total > 1e-6).then_some(best.0)
}

impl Container {
    pub fn new() -> Self {
        Self {
            pending: Vec::with_capacity(8192),
            offset: 0,
            active: None,
            reference: sequence(),
        }
    }

    pub fn reset(&mut self) {
        self.pending.clear();
        self.offset = 0;
        self.active = None;
    }

    pub fn push(&mut self, symbols: &[Complex<f32>]) {
        if self.offset > 0 {
            self.pending.drain(..self.offset);
            self.offset = 0;
        }
        self.pending.extend_from_slice(symbols);
    }

    pub fn next(&mut self, out: &mut Vec<Complex<f32>>) -> Option<Content> {
        out.clear();
        if self.active.is_some() {
            return self.extract(out);
        }
        let available = self.pending.len() - self.offset;
        if available < HEADER {
            return None;
        }
        let mut position = self.offset;
        while position + HEADER <= self.pending.len() {
            let samples = &self.pending[position..];
            if fit(&samples[..32], &self.reference[..32]).is_some()
                && let Some((phase, frequency)) = fit(&samples[..SOSF], &self.reference[..SOSF])
                && let Some(code) = format(
                    &samples[..HEADER],
                    &self.reference[..HEADER],
                    phase,
                    frequency,
                )
            {
                if position > self.offset {
                    out.extend_from_slice(&self.pending[self.offset..position]);
                    self.offset = position;
                    return Some(Content::Plain);
                }
                if available < PILOT_START + pl::PILOT_LENGTH {
                    return None;
                }
                let pilot = &samples[PILOT_START..PILOT_START + pl::PILOT_LENGTH];
                let pilots = fit(
                    pilot,
                    &self.reference[PILOT_START..PILOT_START + pl::PILOT_LENGTH],
                )
                .is_some();
                let content = match code {
                    0 => Content::Extended,
                    1 => Content::Legacy,
                    other => Content::Unsupported(other),
                };
                self.active = Some(Active {
                    content,
                    position: HEADER,
                    pilots,
                    phase,
                    frequency,
                });
                self.offset += HEADER;
                return self.extract(out);
            }
            position += 1;
        }
        if position > self.offset {
            out.extend_from_slice(&self.pending[self.offset..position]);
            self.offset = position;
            Some(Content::Plain)
        } else {
            None
        }
    }

    fn extract(&mut self, out: &mut Vec<Complex<f32>>) -> Option<Content> {
        let active = self.active.as_mut()?;
        let count = (self.pending.len() - self.offset).min(LENGTH - active.position);
        if count == 0 {
            return None;
        }
        let unsupported = matches!(active.content, Content::Unsupported(_));
        for i in 0..count {
            let position = active.position + i;
            let pilot = active.pilots
                && position >= PILOT_START
                && (position - PILOT_START) % PILOT_PERIOD < pl::PILOT_LENGTH;
            if !pilot && !unsupported {
                out.push(
                    self.pending[self.offset + i]
                        * Complex::from_polar(
                            1.0,
                            -active.phase - active.frequency * position as f32,
                        ),
                );
            }
        }
        let content = active.content;
        active.position += count;
        self.offset += count;
        if active.position == LENGTH {
            self.active = None;
        }
        Some(content)
    }
}

#[cfg(any(test, feature = "test-signals"))]
pub fn wrap(payload: &[Complex<f32>], code: u8, pilots: bool, count: usize) -> Vec<Complex<f32>> {
    let reference = sequence();
    let mut output = Vec::with_capacity(LENGTH * count);
    let mut cursor = 0;
    for _ in 0..count {
        for (position, &known) in reference.iter().enumerate() {
            let value = if position < SOSF {
                known
            } else if position < HEADER {
                let column = (position - SOSF) / 30 + 1;
                if (code & column as u8).count_ones().is_multiple_of(2) {
                    known
                } else {
                    -known
                }
            } else if pilots
                && position >= PILOT_START
                && (position - PILOT_START) % PILOT_PERIOD < pl::PILOT_LENGTH
            {
                known
            } else {
                let value = payload[cursor % payload.len()];
                cursor += 1;
                value
            };
            output.push(value);
        }
    }
    output
}

#[cfg(test)]
mod tests;
