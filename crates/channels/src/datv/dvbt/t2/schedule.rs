use num_complex::Complex;

use super::{
    Coding, Constellation, DecodeError, Frame, Rate,
    bicm::Decoder,
    interleave,
    signalling::{Plp, Post, Pre},
    transport::Transport,
    transport_clock::Sink,
};
use crate::datv::dvbs::PACKET;

const TIME_CAPACITY: usize = 1 << 19;

#[derive(Clone, Copy, Debug, Default)]
pub struct Report {
    pub blocks: u32,
    pub packets: u32,
    pub errors: u32,
    pub corrected_bits: u32,
    pub last_error: Option<DecodeError>,
}

pub struct Scheduler {
    decoders: Vec<(Frame, Rate, Decoder)>,
    selected: Option<Plp>,
    next_frame: Option<usize>,
    buffered: Vec<Complex<f32>>,
    deinterleaved: Vec<Complex<f32>>,
    word: Vec<bool>,
    transport: Transport,
    slices: usize,
    interval: usize,
    slice_length: usize,
    received: usize,
    time_block: usize,
    decoder: usize,
    active: bool,
    report: Report,
    generation: u64,
}

impl Scheduler {
    pub fn new() -> Result<Self, DecodeError> {
        let mut decoders = Vec::with_capacity(14);
        for frame in [Frame::Short, Frame::Normal] {
            for rate in [
                Rate::R1_2,
                Rate::R3_5,
                Rate::R2_3,
                Rate::R3_4,
                Rate::R4_5,
                Rate::R5_6,
                Rate::R1_3,
                Rate::R2_5,
            ] {
                let lite = matches!(rate, Rate::R1_3 | Rate::R2_5);
                if lite && frame != Frame::Short {
                    continue;
                }
                let coding = Coding {
                    frame,
                    rate,
                    constellation: Constellation::Qpsk,
                    rotated: false,
                    lite,
                };
                decoders.push((frame, rate, Decoder::new(coding)?));
            }
        }
        Ok(Self {
            decoders,
            selected: None,
            next_frame: None,
            buffered: Vec::with_capacity(TIME_CAPACITY),
            deinterleaved: vec![Complex::default(); TIME_CAPACITY],
            word: vec![false; 54000],
            transport: Transport::default(),
            slices: 0,
            interval: 0,
            slice_length: 0,
            received: 0,
            time_block: 0,
            decoder: 0,
            active: false,
            report: Report::default(),
            generation: 0,
        })
    }

    pub fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.selected = None;
        self.next_frame = None;
        self.buffered.clear();
        self.transport.reset();
        self.active = false;
    }

    pub(super) const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn selected(&self) -> Option<Plp> {
        self.selected
    }
    pub const fn report(&self) -> Report {
        self.report
    }

    pub fn begin(&mut self, pre: Pre, post: &Post, id: Option<u8>) -> Result<(), DecodeError> {
        self.active = false;
        if pre.rf_count != 1 {
            return Err(DecodeError::Stream);
        }
        let plp = post.select(id).ok_or(DecodeError::Plp)?;
        self.begin_plp(pre, post, plp, 0)
    }

    pub(super) fn begin_plp(
        &mut self,
        pre: Pre,
        post: &Post,
        plp: Plp,
        origin: u64,
    ) -> Result<(), DecodeError> {
        self.active = false;
        if post.frame < plp.first_frame
            || !(post.frame - plp.first_frame).is_multiple_of(plp.frame_interval)
        {
            return Ok(());
        }
        let pi = plp.interleaving_frames();
        let position = (post.frame - plp.first_frame) / plp.frame_interval % pi;
        let changed = self.selected.is_none_or(|old| {
            old.id != plp.id
                || old.coding != plp.coding
                || old.time_length != plp.time_length
                || old.time_across_frames != plp.time_across_frames
                || old.frame_interval != plp.frame_interval
        });
        let discontinuity = self.next_frame.is_some_and(|frame| frame != post.frame);
        if changed || discontinuity {
            self.reset();
        }
        if position != 0
            && (self.selected.is_none()
                || self.selected.is_some_and(|old| old.blocks != plp.blocks))
        {
            return Err(DecodeError::Discontinuity);
        }
        if position == 0 {
            self.transport.set_origin(origin);
            if !self.buffered.is_empty() {
                return Err(DecodeError::Discontinuity);
            }
            self.time_block =
                if plp.time_length > 0 && !plp.time_across_frames && plp.blocks < plp.time_length {
                    plp.time_length - plp.blocks
                } else {
                    0
                };
        }
        self.decoder = self
            .decoders
            .iter()
            .position(|(frame, rate, _)| *frame == plp.coding.frame && *rate == plp.coding.rate)
            .ok_or(DecodeError::Parameters)?;
        self.decoders[self.decoder].2.configure(plp.coding)?;
        self.slices = if plp.kind == 2 { post.subslices } else { 1 };
        self.interval = if plp.kind == 2 {
            post.subslice_interval
        } else {
            0
        };
        let total = plp.blocks * plp.coding.cells();
        if !total.is_multiple_of(pi * self.slices) {
            return Err(DecodeError::Signalling);
        }
        self.slice_length = total / pi / self.slices;
        if self.slices > 1 && self.interval < self.slice_length {
            return Err(DecodeError::Signalling);
        }
        let blocks = if plp.time_across_frames {
            plp.blocks
        } else {
            plp.blocks.div_ceil(plp.time_length.max(1))
        };
        if plp.time_length > 0 && blocks * plp.coding.cells() > TIME_CAPACITY {
            return Err(DecodeError::Capacity);
        }
        self.received = 0;
        self.next_frame = Some((post.frame + plp.frame_interval) % pre.frames);
        self.selected = Some(plp);
        self.active = true;
        Ok(())
    }

    pub fn push(
        &mut self,
        address: usize,
        cells: &[Complex<f32>],
        noise: f32,
        packets: &mut Vec<[u8; PACKET]>,
    ) -> Result<(), DecodeError> {
        self.push_to(address, cells, noise, packets)
    }

    pub(super) fn push_to<S: Sink>(
        &mut self,
        address: usize,
        cells: &[Complex<f32>],
        noise: f32,
        packets: &mut S,
    ) -> Result<(), DecodeError> {
        if !self.active || self.slice_length == 0 {
            return Ok(());
        }
        let plp = self.selected.ok_or(DecodeError::Plp)?;
        for slice in 0..self.slices {
            let start = plp.start + slice * self.interval;
            let end = start + self.slice_length;
            let first = address.max(start);
            let last = (address + cells.len()).min(end);
            if first >= last {
                continue;
            }
            let expected = slice * self.slice_length + first - start;
            if expected != self.received {
                return Err(DecodeError::Discontinuity);
            }
            for &cell in &cells[first - address..last - address] {
                if self.buffered.len() == self.buffered.capacity() {
                    return Err(DecodeError::Capacity);
                }
                self.buffered.push(cell);
                self.received += 1;
                let block_size = self.block_size(plp)?;
                if self.buffered.len() == block_size * plp.coding.cells() {
                    self.decode_block(plp, block_size, noise, packets)?;
                    self.buffered.clear();
                    self.time_block += 1;
                }
            }
        }
        Ok(())
    }

    fn block_size(&self, plp: Plp) -> Result<usize, DecodeError> {
        if plp.time_length == 0 {
            return Ok(1);
        }
        if plp.time_across_frames {
            return Ok(plp.blocks);
        }
        interleave::time_block_size(plp.blocks, plp.time_length, self.time_block)
    }

    fn decode_block<S: Sink>(
        &mut self,
        plp: Plp,
        blocks: usize,
        noise: f32,
        packets: &mut S,
    ) -> Result<(), DecodeError> {
        let count = self.buffered.len();
        if plp.time_length > 0 {
            interleave::time_deinterleave(
                &self.buffered,
                &mut self.deinterleaved[..count],
                plp.coding.cells(),
            )?;
        } else {
            self.deinterleaved[..count].copy_from_slice(&self.buffered);
        }
        for index in 0..blocks {
            let first = index * plp.coding.cells();
            let result = self.decoders[self.decoder].2.decode(
                &self.deinterleaved[first..first + plp.coding.cells()],
                if plp.time_length == 0 {
                    self.time_block
                } else {
                    index
                },
                noise.max(0.0001),
                &mut self.word,
            );
            let result = match result {
                Ok(decoded) => {
                    self.report.blocks = self.report.blocks.saturating_add(1);
                    self.report.corrected_bits = self
                        .report
                        .corrected_bits
                        .saturating_add(decoded.corrected_bits as u32);
                    self.transport.push_to(&self.word[..decoded.bits], packets)
                }
                Err(error) => Err(error),
            };
            match result {
                Ok(report) => {
                    self.report.packets = self.report.packets.saturating_add(report.packets as u32);
                    self.report.errors = self
                        .report
                        .errors
                        .saturating_add((report.crc_errors + report.discontinuities) as u32);
                    if report.crc_errors > 0 {
                        self.report.last_error = Some(DecodeError::Header);
                    } else if report.discontinuities > 0 {
                        self.report.last_error = Some(DecodeError::Discontinuity);
                    } else {
                        self.report.last_error = None;
                    }
                }
                Err(error) => {
                    self.transport.reset();
                    self.report.errors = self.report.errors.saturating_add(1);
                    self.report.last_error = Some(error);
                }
            }
        }
        Ok(())
    }

    pub fn end(&mut self) -> Result<(), DecodeError> {
        if self.active && self.received != self.slices * self.slice_length {
            self.reset();
            return Err(DecodeError::Discontinuity);
        }
        self.active = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
