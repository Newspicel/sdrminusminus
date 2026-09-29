use num_complex::Complex;
use rtrb::Consumer;
use sdrmm_channels::array_processor::MAX_LANES;
use sdrmm_wire::{ArrayCalSource, CalSourceKind};

use super::{correct::CorrectionSet, window::Windows};

pub(crate) const CAPTURE_MAX: usize = 131_072;
pub(crate) const CAPTURE_POOL: usize = 2;
pub(crate) const MAX_DECIMATION: usize = 1_024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureKind {
    Coarse,
    Solve,
    Check,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureStart {
    Now,
    NoiseWindow,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CaptureRequest {
    pub(crate) id: u32,
    pub(crate) kind: CaptureKind,
    pub(crate) start: CaptureStart,
    pub(crate) len: usize,
    pub(crate) decimation: usize,
    pub(crate) source: ArrayCalSource,
    pub(crate) equaliser: bool,
}

pub(crate) struct CaptureBuffers {
    pub(crate) lanes: Vec<Vec<Complex<f32>>>,
    pub(crate) first_index: u64,
    pub(crate) generation: u32,
    pub(crate) sample_rate: f64,
    pub(crate) decimation: usize,
    pub(crate) offsets: [i64; MAX_LANES],
    pub(crate) centers_hz: [f64; MAX_LANES],
    pub(crate) devices: [u8; MAX_LANES],
}

impl CaptureBuffers {
    pub(crate) fn new(lanes: usize, capacity: usize) -> Self {
        Self {
            lanes: (0..lanes.min(MAX_LANES))
                .map(|_| Vec::with_capacity(capacity))
                .collect(),
            first_index: 0,
            generation: 0,
            sample_rate: 0.0,
            decimation: 1,
            offsets: [0; MAX_LANES],
            centers_hz: [0.0; MAX_LANES],
            devices: [0; MAX_LANES],
        }
    }

    fn clear(&mut self) {
        for lane in &mut self.lanes {
            lane.clear();
        }
    }

    fn filled(&self) -> usize {
        self.lanes.iter().map(Vec::len).min().unwrap_or(0)
    }
}

pub(crate) struct CaptureJob {
    pub(crate) request: CaptureRequest,
    pub(crate) buffers: Box<CaptureBuffers>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SolveSummary {
    pub(crate) lanes: u8,
    pub(crate) delay: [f32; MAX_LANES],
    pub(crate) phase_deg: [f32; MAX_LANES],
    pub(crate) gain_db: [f32; MAX_LANES],
    pub(crate) coherence: [f32; MAX_LANES],
    pub(crate) cfo_hz: [f32; MAX_LANES],
    pub(crate) purity: f32,
    pub(crate) phase_ready: bool,
    pub(crate) gain_ready: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SolveFailure {
    NoPeak { lane: u8 },
    Ambiguous { lane: u8 },
    Short,
    LowCoherence { lane: u8, coherence: f32 },
    FewBins,
    Clipped { lane: u8 },
    Drift { ppm: f32 },
    Refused,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct CalQuality {
    pub(crate) source: Option<CalSourceKind>,
    pub(crate) phase_sigma_deg: f32,
    pub(crate) gain_sigma_db: f32,
    pub(crate) valid_hz: Option<(f64, f64)>,
}

pub(crate) struct Solution {
    pub(crate) id: u32,
    pub(crate) offsets: Option<[i64; MAX_LANES]>,
    pub(crate) correction: Option<Box<CorrectionSet>>,
    pub(crate) outcome: Result<SolveSummary, SolveFailure>,
    pub(crate) quality: CalQuality,
}

pub(crate) struct FillContext<'a> {
    pub(crate) windows: &'a Windows,
    pub(crate) discontinuous: bool,
    pub(crate) generation: u32,
    pub(crate) sample_rate: f64,
    pub(crate) offsets: &'a [i64],
    pub(crate) centers_hz: &'a [f64],
    pub(crate) devices: &'a [u8],
}

pub(crate) enum Filled {
    Waiting,
    Done(CaptureJob),
    Aborted(u32),
}

struct Armed {
    request: CaptureRequest,
    buffers: Box<CaptureBuffers>,
    next: Option<u64>,
    acc: [Complex<f32>; MAX_LANES],
    summed: usize,
}

pub(crate) struct CaptureSlot {
    armed: Option<Armed>,
    spare: Option<Box<CaptureBuffers>>,
    lanes: usize,
}

impl CaptureSlot {
    pub(crate) const fn new(lanes: usize) -> Self {
        Self {
            armed: None,
            spare: None,
            lanes,
        }
    }

    pub(crate) const fn armed(&self) -> bool {
        self.armed.is_some()
    }

    pub(crate) fn arm(
        &mut self,
        request: CaptureRequest,
        free: &mut Consumer<Box<CaptureBuffers>>,
    ) -> bool {
        if self.armed.is_some()
            || request.len == 0
            || !(1..=MAX_DECIMATION).contains(&request.decimation)
        {
            return false;
        }
        let Some(mut buffers) = self.spare.take().or_else(|| free.pop().ok()) else {
            return false;
        };
        if buffers.lanes.len() != self.lanes
            || buffers
                .lanes
                .iter()
                .any(|lane| lane.capacity() < request.len)
        {
            self.spare = Some(buffers);
            return false;
        }
        buffers.clear();
        self.armed = Some(Armed {
            request,
            buffers,
            next: None,
            acc: [Complex::default(); MAX_LANES],
            summed: 0,
        });
        true
    }

    pub(crate) fn keep(&mut self, buffers: Box<CaptureBuffers>) -> Option<Box<CaptureBuffers>> {
        if self.spare.is_none() {
            self.spare = Some(buffers);
            None
        } else {
            Some(buffers)
        }
    }

    pub(crate) fn fill(
        &mut self,
        lanes: &[&[Complex<f32>]],
        index: u64,
        context: &FillContext<'_>,
    ) -> Filled {
        let Some(armed) = self.armed.as_mut() else {
            return Filled::Waiting;
        };
        let count = lanes.first().map_or(0, |lane| lane.len()) as u64;
        let end = index + count;
        let started = armed.next.is_some();
        if started && (context.discontinuous || armed.next != Some(index)) {
            return self.abort();
        }
        let Some((from, until)) = usable(armed.request.start, context.windows, index, end) else {
            return if started {
                self.abort()
            } else {
                Filled::Waiting
            };
        };
        if started && from != index {
            return self.abort();
        }
        if until < end && until - from < armed.remaining() {
            return if started {
                self.abort()
            } else {
                Filled::Waiting
            };
        }
        if !started {
            let buffers = &mut armed.buffers;
            buffers.first_index = from;
            buffers.generation = context.generation;
            buffers.sample_rate = context.sample_rate;
            buffers.decimation = armed.request.decimation;
            buffers.offsets = [0; MAX_LANES];
            buffers.offsets[..context.offsets.len()].copy_from_slice(context.offsets);
            buffers.centers_hz = [0.0; MAX_LANES];
            let centers = context.centers_hz.len().min(MAX_LANES);
            buffers.centers_hz[..centers].copy_from_slice(&context.centers_hz[..centers]);
            buffers.devices = [0; MAX_LANES];
            let devices = context.devices.len().min(MAX_LANES);
            buffers.devices[..devices].copy_from_slice(&context.devices[..devices]);
        }
        let start = (from - index) as usize;
        let stop = (until - index) as usize;
        armed.decimate(lanes, start, stop);
        armed.next = Some(until);
        if armed.buffers.filled() >= armed.request.len {
            return self.finish();
        }
        Filled::Waiting
    }

    fn finish(&mut self) -> Filled {
        match self.armed.take() {
            Some(armed) => {
                let mut buffers = armed.buffers;
                for lane in &mut buffers.lanes {
                    lane.truncate(armed.request.len);
                }
                Filled::Done(CaptureJob {
                    request: armed.request,
                    buffers,
                })
            }
            None => Filled::Waiting,
        }
    }

    fn abort(&mut self) -> Filled {
        match self.armed.take() {
            Some(armed) => {
                let id = armed.request.id;
                self.spare = Some(armed.buffers);
                Filled::Aborted(id)
            }
            None => Filled::Waiting,
        }
    }

    pub(crate) fn cancel(&mut self) -> Option<u32> {
        match self.abort() {
            Filled::Aborted(id) => Some(id),
            _ => None,
        }
    }
}

impl Armed {
    fn remaining(&self) -> u64 {
        let owed = self.request.len.saturating_sub(self.buffers.filled()) * self.request.decimation;
        owed.saturating_sub(self.summed) as u64
    }

    fn decimate(&mut self, lanes: &[&[Complex<f32>]], start: usize, stop: usize) {
        let factor = self.request.decimation;
        let scale = 1.0 / factor as f32;
        let want = self.request.len;
        let mut at = start;
        while at < stop && self.buffers.filled() < want {
            let take = (factor - self.summed).min(stop - at);
            for (acc, lane) in self.acc.iter_mut().zip(lanes) {
                *acc += lane[at..at + take].iter().sum::<Complex<f32>>();
            }
            self.summed += take;
            at += take;
            if self.summed == factor {
                for (acc, out) in self.acc.iter_mut().zip(&mut self.buffers.lanes) {
                    out.push(*acc * scale);
                    *acc = Complex::default();
                }
                self.summed = 0;
            }
        }
    }
}

fn usable(start: CaptureStart, windows: &Windows, index: u64, end: u64) -> Option<(u64, u64)> {
    match start {
        CaptureStart::Now => {
            if windows.gate_at(index).is_some() {
                return None;
            }
            let until = windows
                .next_change(index)
                .map_or(end, |change| change.min(end));
            Some((index, until))
        }
        CaptureStart::NoiseWindow => {
            let (from, until) = windows.reference()?;
            let from = from.max(index);
            let until = until.min(end);
            (from < until).then_some((from, until))
        }
    }
}

#[cfg(test)]
mod tests;
