use std::collections::VecDeque;

use num_complex::Complex;

use super::{
    DecodeError,
    schedule::{Report, Scheduler},
    signalling::{Plp, Post, Pre},
    transport_clock::{Clock, TimedPacket},
};
use crate::datv::dvbs::PACKET;

#[derive(Clone, Copy, Default)]
struct Timing {
    counter: Option<(i64, u32, u8)>,
    presentation: Option<(i64, u64)>,
    period: Option<(i64, i64)>,
}

impl Timing {
    fn observe(&mut self, ordinal: i64, clock: Option<Clock>) {
        match clock {
            Some(Clock::Counter { value, bits }) => {
                if let Some((previous, reference, width)) = self.counter
                    && width == bits
                    && ordinal > previous
                {
                    let elapsed = (value.wrapping_sub(reference)) & ((1 << bits) - 1);
                    if elapsed > 0 {
                        self.period = Some((i64::from(elapsed), ordinal - previous));
                    }
                }
                self.counter = Some((ordinal, value, bits));
            }
            Some(Clock::Presentation(value)) => {
                if self.period.is_none()
                    && let Some((previous, reference)) = self.presentation
                    && value > reference
                    && ordinal > previous
                {
                    self.period = Some(((value - reference) as i64, ordinal - previous));
                }
                self.presentation = Some((ordinal, value));
            }
            None => {}
        }
    }
}

struct Merge {
    pending: [VecDeque<(i64, [u8; PACKET])>; 2],
    next: [i64; 2],
    timing: [Timing; 2],
    offset: Option<i64>,
}

impl Default for Merge {
    fn default() -> Self {
        Self {
            pending: std::array::from_fn(|_| VecDeque::with_capacity(8192)),
            next: [0; 2],
            timing: [Timing::default(); 2],
            offset: None,
        }
    }
}

impl Merge {
    fn reset(&mut self) {
        for queue in &mut self.pending {
            queue.clear();
        }
        self.next = [0; 2];
        self.timing = [Timing::default(); 2];
        self.offset = None;
    }

    fn append(&mut self, stream: usize, packets: &[TimedPacket]) -> Result<(), DecodeError> {
        for packet in packets {
            if self.pending[stream].len() == self.pending[stream].capacity() {
                return Err(DecodeError::CommonPlp);
            }
            let ordinal = self.next[stream];
            self.timing[stream].observe(ordinal, packet.clock);
            self.pending[stream].push_back((ordinal, packet.data));
            self.next[stream] += 1;
        }
        Ok(())
    }

    fn align(&self) -> Option<i64> {
        let [data, common] = self.timing;
        let (ticks, slots) = data.period.or(common.period)?;
        let (a, b, elapsed) = if let (Some((a, ta)), Some((b, tb))) =
            (data.presentation, common.presentation)
        {
            (a, b, ta as i64 - tb as i64)
        } else if let (Some((a, ta, bits)), Some((b, tb, width))) = (data.counter, common.counter) {
            if bits != width {
                return None;
            }
            let period = 1_i64 << bits;
            let elapsed =
                (i64::from(ta) - i64::from(tb) + period / 2).rem_euclid(period) - period / 2;
            (a, b, elapsed)
        } else {
            return None;
        };
        let numerator = elapsed * slots;
        let rounded = (numerator.abs() + ticks / 2) / ticks * numerator.signum();
        Some(b - a + rounded)
    }

    fn drain(&mut self, output: &mut Vec<[u8; PACKET]>) -> Result<(), DecodeError> {
        if self.offset.is_none() {
            self.offset = self.align();
        }
        let Some(offset) = self.offset else {
            return Ok(());
        };
        while let (Some(&(data_index, data)), Some(&(common_index, common))) =
            (self.pending[0].front(), self.pending[1].front())
        {
            match (data_index + offset).cmp(&common_index) {
                std::cmp::Ordering::Less => {
                    self.pending[0].pop_front();
                }
                std::cmp::Ordering::Greater => {
                    self.pending[1].pop_front();
                }
                std::cmp::Ordering::Equal => {
                    if output.len() == output.capacity() {
                        return Err(DecodeError::Capacity);
                    }
                    let null = data[1] & 0x1f == 0x1f && data[2] == 0xff;
                    output.push(if null { common } else { data });
                    self.pending[0].pop_front();
                    self.pending[1].pop_front();
                }
            }
        }
        Ok(())
    }
}

pub(super) struct Multiplex {
    data: Scheduler,
    common: Scheduler,
    merged: Merge,
    packets: [Vec<TimedPacket>; 2],
    group: Option<(u8, u8)>,
    emitted: u32,
}

impl Multiplex {
    pub fn new() -> Result<Self, DecodeError> {
        Ok(Self {
            data: Scheduler::new()?,
            common: Scheduler::new()?,
            merged: Merge::default(),
            packets: std::array::from_fn(|_| Vec::with_capacity(8192)),
            group: None,
            emitted: 0,
        })
    }

    pub fn reset(&mut self) {
        self.data.reset();
        self.common.reset();
        self.merged.reset();
        self.group = None;
    }

    pub fn selected(&self) -> Option<Plp> {
        self.data.selected()
    }

    pub fn report(&self) -> Report {
        let mut report = self.data.report();
        let common = self.common.report();
        report.packets = self.emitted;
        report.errors = report.errors.saturating_add(common.errors);
        if self.group.is_some() {
            report.last_error = report.last_error.or(common.last_error);
        }
        report
    }

    pub fn begin(
        &mut self,
        pre: Pre,
        post: &Post,
        id: Option<u8>,
        origin: u64,
    ) -> Result<(), DecodeError> {
        if pre.rf_count != 1 {
            return Err(DecodeError::Stream);
        }
        let data = post.select(id).ok_or(DecodeError::Plp)?;
        let common = post.plps[..post.count]
            .iter()
            .flatten()
            .find(|p| p.kind == 0 && p.group == data.group)
            .copied();
        let group = common.map(|p| (data.id, p.id));
        if group != self.group {
            self.reset();
        }
        self.group = group;
        let generation = (self.data.generation(), self.common.generation());
        self.data.begin_plp(pre, post, data, origin)?;
        if let Some(plp) = common {
            if plp.payload != 3 {
                return Err(DecodeError::Stream);
            }
            self.common.begin_plp(pre, post, plp, origin)?;
        }
        if generation != (self.data.generation(), self.common.generation()) {
            self.merged.reset();
        }
        Ok(())
    }

    pub fn push(
        &mut self,
        address: usize,
        cells: &[Complex<f32>],
        noise: f32,
        output: &mut Vec<[u8; PACKET]>,
    ) -> Result<(), DecodeError> {
        let before = output.len();
        if self.group.is_none() {
            self.data.push(address, cells, noise, output)?;
        } else {
            let errors = self.data.report().errors + self.common.report().errors;
            self.packets.iter_mut().for_each(Vec::clear);
            self.data
                .push_to(address, cells, noise, &mut self.packets[0])?;
            self.common
                .push_to(address, cells, noise, &mut self.packets[1])?;
            if errors != self.data.report().errors + self.common.report().errors {
                self.merged.reset();
                return Err(DecodeError::Discontinuity);
            }
            for stream in 0..2 {
                self.merged.append(stream, &self.packets[stream])?;
            }
            self.merged.drain(output)?;
        }
        self.emitted = self.emitted.saturating_add((output.len() - before) as u32);
        Ok(())
    }

    pub fn end(&mut self) -> Result<(), DecodeError> {
        self.data.end()?;
        if self.group.is_some() {
            self.common.end()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
