use std::{f32::consts::TAU, sync::Arc};

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use super::{
    DecodeError,
    acquire::{Acquisition, Preamble},
    common::Multiplex,
    equalize::Equalizer,
    mapping::Mapping,
    schedule::Report,
    signalling::{Pre, Signalling},
};
use crate::datv::dvbs::PACKET;

struct Mode {
    fft: Arc<dyn Fft<f32>>,
    map: Mapping,
}

#[derive(Clone, Copy)]
enum State {
    Search,
    Skip(usize),
    P2(Preamble),
    Data {
        pre: Pre,
        symbol: usize,
        address: usize,
    },
}

pub struct Receiver {
    modes: Vec<Mode>,
    acquisition: Acquisition,
    equalizer: Equalizer,
    signalling: Signalling,
    scheduler: Multiplex,
    pending: Vec<Complex<f32>>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    equalized: Vec<Complex<f32>>,
    cells: Vec<Complex<f32>>,
    p2: Vec<Complex<f32>>,
    l1: Vec<Complex<f32>>,
    state: State,
    phase: f32,
    selected: Option<u8>,
    pub parameters: Option<Pre>,
    pub frequency: f32,
    pub errors: u32,
    pub last_error: Option<DecodeError>,
    pub frames: u32,
    locked: bool,
    fef: usize,
    consumed: u64,
    origin: u64,
}

impl Receiver {
    pub fn new(selected: Option<u8>) -> Result<Self, DecodeError> {
        let mut planner = FftPlanner::new();
        let mut modes = Vec::with_capacity(6);
        for size in [1024, 2048, 4096, 8192, 16384, 32768] {
            modes.push(Mode {
                fft: planner.plan_fft_forward(size),
                map: Mapping::new(size)?,
            });
        }
        let scratch = modes
            .iter()
            .map(|m| m.fft.get_inplace_scratch_len())
            .max()
            .unwrap_or(32768);
        Ok(Self {
            modes,
            acquisition: Acquisition::default(),
            equalizer: Equalizer::default(),
            signalling: Signalling::new()?,
            scheduler: Multiplex::new()?,
            pending: Vec::with_capacity(131072),
            spectrum: vec![Complex::default(); 32768],
            scratch: vec![Complex::default(); scratch],
            equalized: vec![Complex::default(); 27841],
            cells: vec![Complex::default(); 27841],
            p2: vec![Complex::default(); 22432],
            l1: vec![Complex::default(); 22432],
            state: State::Search,
            phase: 0.0,
            selected,
            parameters: None,
            frequency: 0.0,
            errors: 0,
            last_error: None,
            frames: 0,
            locked: false,
            fef: 0,
            consumed: 0,
            origin: 0,
        })
    }

    pub fn reset(&mut self) {
        self.pending.clear();
        self.state = State::Search;
        self.parameters = None;
        self.phase = 0.0;
        self.frequency = 0.0;
        self.locked = false;
        self.equalizer.reset();
        self.scheduler.reset();
    }

    pub fn select(&mut self, selected: Option<u8>) {
        if self.selected != selected {
            self.reset();
            self.selected = selected;
        }
    }

    pub const fn locked(&self) -> bool {
        self.locked
    }
    pub fn report(&self) -> Report {
        self.scheduler.report()
    }
    pub fn plp(&self) -> Option<super::signalling::Plp> {
        self.scheduler.selected()
    }
    pub fn snr(&self) -> f32 {
        -10.0 * self.equalizer.noise.max(1e-6).log10()
    }

    pub fn push(&mut self, iq: &[Complex<f32>], packets: &mut Vec<[u8; PACKET]>) {
        for block in iq.chunks(4096) {
            if block.iter().any(|p| !p.norm_sqr().is_finite()) {
                self.fail(DecodeError::NonFinite);
                self.pending.clear();
                continue;
            }
            if self.pending.len() + block.len() > self.pending.capacity() {
                self.fail(DecodeError::Capacity);
                self.pending.clear();
            }
            self.pending.extend_from_slice(block);
            loop {
                match self.step(packets) {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(error) => {
                        self.fail(error);
                        if !self.pending.is_empty() {
                            self.consume(1);
                        }
                    }
                }
            }
        }
    }

    fn fail(&mut self, error: DecodeError) {
        self.errors = self.errors.saturating_add(1);
        self.last_error = Some(error);
        self.locked = false;
        self.parameters = None;
        self.state = State::Search;
        self.scheduler.reset();
    }

    fn step(&mut self, packets: &mut Vec<[u8; PACKET]>) -> Result<bool, DecodeError> {
        match self.state {
            State::Skip(remaining) => {
                let consumed = self.pending.len().min(remaining);
                self.consume(consumed);
                self.state = if consumed == remaining {
                    State::Search
                } else {
                    State::Skip(remaining - consumed)
                };
                Ok(consumed > 0)
            }
            State::Search => {
                if self.pending.len() < 8192 {
                    return Ok(false);
                }
                if let Some(detection) = self.acquisition.find(&self.pending) {
                    self.origin = self.consumed + detection.start as u64;
                    self.frequency = detection.frequency;
                    self.phase = 0.0;
                    self.consume(detection.start + 2048);
                    if detection.preamble.fft().is_ok() {
                        self.state = State::P2(detection.preamble);
                    }
                    Ok(true)
                } else {
                    let consumed = self.pending.len() - 2047;
                    self.consume(consumed);
                    self.locked = false;
                    Ok(false)
                }
            }
            State::P2(preamble) => self.preamble(preamble, packets),
            State::Data {
                pre,
                symbol,
                address,
            } => self.data(pre, symbol, address, packets),
        }
    }

    fn preamble(
        &mut self,
        preamble: Preamble,
        packets: &mut Vec<[u8; PACKET]>,
    ) -> Result<bool, DecodeError> {
        let fft = preamble.fft()?;
        let count = preamble.p2_symbols()?;
        if self.pending.len() < count * (fft + fft / 4) {
            return Ok(false);
        }
        let mut last = DecodeError::Acquisition;
        for code in [0, 1, 2, 3, 4, 5, 6] {
            let guard = match code {
                0 => fft / 32,
                1 => fft / 16,
                2 => fft / 8,
                3 => fft / 4,
                4 => fft / 128,
                5 => fft * 19 / 128,
                _ => fft * 19 / 256,
            };
            if !guard_allowed(preamble, code) {
                continue;
            }
            let (quality, residual) = prefix(&self.pending, 0, fft, guard, self.frequency);
            if quality < 0.3 {
                continue;
            }
            let frequency = self.frequency;
            self.frequency += residual;
            match self.decode_preamble(preamble, guard, code, packets) {
                Ok(pre) => {
                    self.parameters = Some(pre);
                    self.frames = self.frames.saturating_add(1);
                    let used = count * (fft + guard);
                    self.consume(used);
                    let p2_count = self.modes[fft.ilog2() as usize - 10].map.data;
                    let address = count * p2_count - 1840 - pre.post_cells;
                    self.state = State::Data {
                        pre,
                        symbol: count,
                        address,
                    };
                    return Ok(true);
                }
                Err(error) => {
                    last = error;
                    self.frequency = frequency;
                }
            }
        }
        Err(last)
    }

    fn decode_preamble(
        &mut self,
        preamble: Preamble,
        guard: usize,
        code: u8,
        packets: &mut Vec<[u8; PACKET]>,
    ) -> Result<Pre, DecodeError> {
        let fft = preamble.fft()?;
        let mode = fft.ilog2() as usize - 10;
        let count = preamble.p2_symbols()?;
        self.equalizer.reset();
        for symbol in 0..count {
            self.modes[mode].map.p2(preamble, symbol)?;
            self.transform(mode, symbol * (fft + guard) + guard);
            self.equalizer.decode(
                &self.spectrum[..fft],
                &self.modes[mode].map,
                preamble.miso(),
                0,
                &mut self.equalized,
            )?;
            let map = &self.modes[mode].map;
            map.deinterleave(
                &self.equalized[..map.data],
                symbol,
                &mut self.p2[symbol * map.data..(symbol + 1) * map.data],
            )?;
        }
        let capacity = self.modes[mode].map.data;
        for i in 0..1840 {
            self.l1[i] = self.p2[i % count * capacity + i / count];
        }
        let pre = self.signalling.pre(&self.l1[..1840], preamble)?;
        if pre.guard_code != code
            || 1840 + pre.post_cells > capacity * count
            || !pre.post_cells.is_multiple_of(count)
        {
            return Err(DecodeError::Signalling);
        }
        for i in 0..pre.post_cells {
            self.l1[i] = self.p2[i % count * capacity + (1840 + i) / count];
        }
        let post = self.signalling.post(&self.l1[..pre.post_cells], pre)?;
        self.scheduler
            .begin(pre, &post, self.selected, self.origin)?;
        self.fef = if post.fef_interval > 0 && (post.frame + 1).is_multiple_of(post.fef_interval) {
            post.fef_length
        } else {
            0
        };
        let start = (1840 + pre.post_cells) / count;
        let mut address = 0;
        let previous = self.scheduler.report().packets;
        for symbol in 0..count {
            self.scheduler.push(
                address,
                &self.p2[symbol * capacity + start..(symbol + 1) * capacity],
                self.equalizer.noise,
                packets,
            )?;
            address += capacity - start;
        }
        if self.scheduler.report().packets > previous {
            self.locked = true;
        }
        self.last_error = None;
        Ok(pre)
    }

    fn data(
        &mut self,
        pre: Pre,
        symbol: usize,
        address: usize,
        packets: &mut Vec<[u8; PACKET]>,
    ) -> Result<bool, DecodeError> {
        let fft = pre.preamble.fft()?;
        let guard = pre.guard()?;
        let length = fft + guard;
        if self.pending.len() < length + 16 {
            return Ok(false);
        }
        let (mut quality, mut residual) = prefix(&self.pending, 0, fft, guard, self.frequency);
        let mut adjustment = 0_isize;
        let margin = (guard / 8).clamp(1, 8) as isize - 1;
        for offset in -margin..=margin {
            let (candidate, error) = prefix(&self.pending, offset, fft, guard, self.frequency);
            if candidate > quality + 0.002 {
                quality = candidate;
                residual = error;
                adjustment = offset;
            }
        }
        if quality < 0.12 {
            return Err(DecodeError::Acquisition);
        }
        self.frequency += residual * 0.2;
        let mode = fft.ilog2() as usize - 10;
        self.modes[mode].map.data(pre, symbol)?;
        self.transform(mode, (guard as isize + adjustment) as usize);
        let map = &self.modes[mode].map;
        let history = match pre.pilots {
            1 | 3 | 5 | 7 => 3,
            8 => 15,
            _ => 1,
        };
        self.equalizer.decode(
            &self.spectrum[..fft],
            map,
            pre.preamble.miso(),
            history,
            &mut self.equalized,
        )?;
        map.deinterleave(&self.equalized[..map.data], symbol, &mut self.cells)?;
        let active = map.active;
        let previous = self.scheduler.report().packets;
        self.scheduler.push(
            address,
            &self.cells[..active],
            self.equalizer.noise,
            packets,
        )?;
        if self.scheduler.report().packets > previous {
            self.locked = true;
        }
        self.consume((length as isize + adjustment) as usize);
        if symbol + 1 == pre.preamble.p2_symbols()? + pre.data_symbols {
            self.scheduler.end()?;
            self.state = if self.fef > 0 {
                State::Skip(self.fef)
            } else {
                State::Search
            };
        } else {
            self.state = State::Data {
                pre,
                symbol: symbol + 1,
                address: address + active,
            };
        }
        Ok(true)
    }

    fn transform(&mut self, mode: usize, start: usize) {
        let fft = self.modes[mode].map.fft;
        for (i, p) in self.spectrum[..fft].iter_mut().enumerate() {
            *p = self.pending[start + i]
                * Complex::from_polar(1.0, -self.phase - self.frequency * (start + i) as f32);
        }
        self.modes[mode]
            .fft
            .process_with_scratch(&mut self.spectrum[..fft], &mut self.scratch);
    }

    fn consume(&mut self, count: usize) {
        self.consumed = self.consumed.saturating_add(count as u64);
        self.pending.drain(..count);
        self.phase = (self.phase + self.frequency * count as f32).rem_euclid(TAU);
    }
}

fn guard_allowed(preamble: Preamble, code: u8) -> bool {
    let field = preamble.s2 >> 1;
    let special = matches!(field, 6 | 7) || (preamble.lite() && field == 3);
    if matches!(field, 1 | 5 | 6 | 7) || (preamble.lite() && matches!(field, 3 | 4)) {
        return (code >= 4) == special && !(field == 5 && code == 3);
    }
    if preamble.fft().is_ok_and(|fft| fft <= 4096) {
        code <= 3 && !(field == 3 && code == 0)
    } else {
        true
    }
}

fn prefix(
    iq: &[Complex<f32>],
    start: isize,
    fft: usize,
    guard: usize,
    frequency: f32,
) -> (f32, f32) {
    let mut correlation = Complex::<f32>::default();
    let mut power = [0.0_f32; 2];
    let edge = (guard / 8).max(1);
    for i in (start + edge as isize) as usize..(start + (guard - edge) as isize) as usize {
        correlation += iq[i + fft] * iq[i].conj();
        power[0] += iq[i].norm_sqr();
        power[1] += iq[i + fft].norm_sqr();
    }
    let quality = correlation.norm_sqr() / (power[0] * power[1]).max(1e-20);
    let residual =
        (correlation * Complex::from_polar(1.0, -frequency * fft as f32)).arg() / fft as f32;
    (quality, residual)
}
