use num_complex::Complex;

use super::{
    coding::{self, Coding, Mcs, PILOT_SLOTS, Signal},
    layout::{self, CU},
    signalling::{self, Plh, Protection},
};
use crate::datv::dvbs2::pl;

const CONFIDENCE: f32 = 0.4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub first: u64,
    pub protection: Protection,
    pub pointer: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Burst {
    pub coding: Coding,
    pub frames: usize,
    pub compact: bool,
    pub tracked: bool,
}

#[derive(Clone, Copy, Debug)]
struct Body {
    at: u64,
    header: u64,
    slots: u64,
    mcs: Mcs,
    tsn: u8,
    phase: Complex<f32>,
}

impl Body {
    const fn end(&self) -> u64 {
        self.at + self.header + self.slots
    }

    const fn carries_data(&self) -> bool {
        matches!(self.mcs.signal, Signal::Data(_)) && !coding::is_dummy(self.tsn)
    }
}

#[derive(Clone, Copy, Debug)]
enum State {
    Lost,
    Header(u64),
    Body(Body),
}

pub struct Tracker {
    format: u8,
    buffer: Vec<Complex<f32>>,
    known: Vec<Complex<f32>>,
    base: u64,
    total: u64,
    marks: [Option<Mark>; 2],
    state: State,
    pub dropped: u32,
    pub ended: bool,
    pub tracked: bool,
    pub error: Option<f32>,
}

impl Tracker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            format: 4,
            buffer: Vec::with_capacity(2 * 1920 * CU),
            known: Vec::with_capacity(5 * 2 * CU),
            base: 0,
            total: 0,
            marks: [None; 2],
            state: State::Lost,
            dropped: 0,
            ended: false,
            tracked: false,
            error: None,
        }
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
        self.base = 0;
        self.total = 0;
        self.marks = [None; 2];
        self.state = State::Lost;
        self.ended = false;
        self.error = None;
    }

    #[must_use]
    pub const fn total(&self) -> u64 {
        self.total
    }

    #[must_use]
    pub const fn lost(&self) -> bool {
        matches!(self.state, State::Lost)
    }

    fn abandon(&mut self) {
        if let State::Body(body) = self.state
            && body.carries_data()
        {
            self.dropped += 1;
        }
    }

    pub fn begin(&mut self, format: u8, mark: Mark) {
        if format != self.format {
            self.abandon();
            self.state = State::Lost;
            self.format = format;
        }
        self.marks = [self.marks[1], Some(mark)];
        self.ended = false;
        if !layout::fragments(format) {
            self.abandon();
            self.state = State::Header(mark.first);
            return;
        }
        let Some(pointer) = mark.pointer else {
            return;
        };
        self.state = match self.state {
            State::Lost => State::Header(pointer),
            State::Header(at) if at >= mark.first && at != pointer => State::Header(pointer),
            State::Body(body)
                if body.end() > pointer || (body.end() >= mark.first && body.end() != pointer) =>
            {
                self.abandon();
                State::Header(pointer)
            }
            state => state,
        };
    }

    pub fn push(&mut self, unit: &[Complex<f32>]) {
        self.buffer.extend_from_slice(unit);
        self.total += 1;
    }

    fn protection(&self, at: u64) -> Protection {
        if self.format == 7 {
            return Protection::Standard;
        }
        self.marks
            .iter()
            .rev()
            .flatten()
            .find(|mark| mark.first <= at)
            .map_or(Protection::Standard, |mark| mark.protection)
    }

    fn discard(&mut self, until: u64) {
        let until = until.min(self.total);
        if until > self.base {
            let count = ((until - self.base) as usize * CU).min(self.buffer.len());
            self.buffer.drain(..count);
            self.base = until;
        }
    }

    fn units(&self, at: u64, count: u64) -> &[Complex<f32>] {
        let start = (at - self.base) as usize * CU;
        &self.buffer[start..start + count as usize * CU]
    }

    fn following(&self, body: Body) -> State {
        let next = body.end();
        match self.marks[1] {
            Some(Mark {
                first,
                pointer: Some(pointer),
                ..
            }) if layout::fragments(self.format)
                && body.at < pointer
                && next >= first
                && next != pointer =>
            {
                if next < pointer {
                    State::Header(pointer)
                } else {
                    State::Lost
                }
            }
            _ => State::Header(next),
        }
    }

    fn header(&mut self, at: u64) -> Option<State> {
        if at < self.base {
            return Some(State::Lost);
        }
        let protection = self.protection(at);
        let units = (protection.symbols() / CU) as u64;
        if self.total < at + units {
            self.discard(at);
            return None;
        }
        let symbols = self.units(at, units);
        let Some((plh, confidence)) = signalling::read_plh(symbols, protection) else {
            return Some(State::Lost);
        };
        let mcs = coding::flexible(self.format, plh.mcs).filter(|_| confidence >= CONFIDENCE);
        let Some(mcs) = mcs else {
            self.dropped += 1;
            return Some(State::Lost);
        };
        let phase = self.measure(at, units, plh, protection);
        self.error = Some(phase.arg());
        Some(State::Body(Body {
            at,
            header: units,
            slots: coding::frame_slots(mcs, plh.tsn) as u64,
            mcs,
            tsn: plh.tsn,
            phase,
        }))
    }

    fn measure(&mut self, at: u64, units: u64, plh: Plh, protection: Protection) -> Complex<f32> {
        let mut known = std::mem::take(&mut self.known);
        known.clear();
        signalling::plh(plh, protection, &mut known);
        let sum = self
            .units(at, units)
            .iter()
            .zip(&known)
            .fold(Complex::new(0.0, 0.0), |sum, (&symbol, &reference)| {
                sum + symbol * reference.conj()
            });
        self.known = known;
        sum
    }

    pub fn poll(&mut self, out: &mut Vec<Complex<f32>>) -> Option<Burst> {
        loop {
            match self.state {
                State::Lost => {
                    self.discard(self.total);
                    return None;
                }
                State::Header(at) => self.state = self.header(at)?,
                State::Body(body) => {
                    if self.total < body.end() {
                        return None;
                    }
                    let burst = self.assemble(body, out);
                    self.discard(body.end());
                    self.state = if body.mcs.last && !layout::fragments(self.format) {
                        self.ended = true;
                        State::Lost
                    } else {
                        self.following(body)
                    };
                    if burst.is_some() {
                        return burst;
                    }
                }
            }
        }
    }

    fn assemble(&self, body: Body, out: &mut Vec<Complex<f32>>) -> Option<Burst> {
        let Signal::Data(coding) = body.mcs.signal else {
            return None;
        };
        if coding::is_dummy(body.tsn) {
            return None;
        }
        let symbols = self.units(body.at + body.header, body.slots);
        out.clear();
        let length = coding.slots() * CU;
        if body.mcs.spread == 1 {
            let turn = if self.tracked {
                Complex::new(1.0, 0.0)
            } else {
                unit(body.phase).conj()
            };
            out.extend(symbols[..length].iter().map(|&symbol| symbol * turn));
        } else {
            combine(symbols, coding, body.mcs.spread, body.phase, out);
        }
        Some(Burst {
            coding,
            frames: 1,
            compact: false,
            tracked: self.tracked || body.mcs.spread > 1,
        })
    }
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}

fn unit(value: Complex<f32>) -> Complex<f32> {
    value / value.norm().max(1e-12)
}

fn combine(
    symbols: &[Complex<f32>],
    coding: Coding,
    spread: usize,
    phase: Complex<f32>,
    out: &mut Vec<Complex<f32>>,
) {
    let slots = coding.slots();
    let copy = coding::copy_slots(coding, spread) * CU;
    out.resize(slots * CU, Complex::new(0.0, 0.0));
    let scale = 1.0 / spread as f32;
    let mut previous = unit(phase);
    for repeat in symbols.chunks_exact(copy).take(spread) {
        for (segment, group) in repeat
            .as_chunks::<{ (PILOT_SLOTS + 1) * CU }>()
            .0
            .iter()
            .enumerate()
        {
            let (data, pilot) = group.split_at(PILOT_SLOTS * CU);
            let anchor = unit(pilot.iter().fold(Complex::new(0.0, 0.0), |sum, &symbol| {
                sum + symbol * pl::pilot_symbol().conj()
            }));
            let turn = unit(previous + anchor).conj() * scale;
            let start = segment * PILOT_SLOTS * CU;
            for (slot, &symbol) in out[start..start + data.len()].iter_mut().zip(data) {
                *slot += symbol * turn;
            }
            previous = anchor;
        }
    }
}
