use num_complex::Complex;
use sdrmm_channels::array_processor::{CorrectionView, MAX_LANES};
use sdrmm_dsp::array_sync::{FastConvolver, design_correction};

use super::align::ALIGN_BLOCK;

mod stage;

pub(crate) use stage::{CorrectionStage, Label, StageJob, correction_stage, spawn_stage};

pub(crate) const CORR_FFT: usize = 4_096;
pub(crate) const CORR_TAPS: usize = 129;
pub(crate) const CORR_DELAY: usize = (CORR_TAPS - 1) / 2;
pub(crate) const CORR_BETA: f32 = 8.0;
pub(crate) const CORR_HOP: usize = CORR_FFT - CORR_TAPS + 1;

pub(crate) struct CorrectionSet {
    pub(crate) spectra: Vec<Vec<Complex<f32>>>,
    pub(crate) generation: u32,
}

impl CorrectionSet {
    pub(crate) fn identity(lanes: usize) -> Self {
        let mut spectrum = Vec::with_capacity(CORR_FFT);
        design_correction(
            CORR_FFT,
            CORR_TAPS,
            CORR_BETA,
            0.0,
            Complex::new(1.0, 0.0),
            None,
            &mut spectrum,
        );
        Self {
            spectra: vec![spectrum; lanes],
            generation: 0,
        }
    }

    pub(crate) fn copy_from(&mut self, other: &Self) {
        for (mine, theirs) in self.spectra.iter_mut().zip(&other.spectra) {
            if mine.len() == theirs.len() {
                mine.copy_from_slice(theirs);
            }
        }
        self.generation = other.generation;
    }

    pub(crate) fn fits(&self, lanes: usize) -> bool {
        self.spectra.len() == lanes && self.spectra.iter().all(|lane| lane.len() == CORR_FFT)
    }
}

pub(crate) struct Corrector {
    lanes: Vec<FastConvolver>,
    active: Box<CorrectionSet>,
    identity: Box<CorrectionSet>,
    out: Vec<Vec<Complex<f32>>>,
    in_index: Option<u64>,
    next_input: u64,
    produced: u64,
    transient: usize,
}

impl Corrector {
    pub(crate) fn new(lanes: usize) -> Self {
        let lanes = lanes.min(MAX_LANES);
        Self {
            lanes: (0..lanes)
                .map(|_| FastConvolver::new(CORR_FFT, CORR_TAPS))
                .collect(),
            active: Box::new(CorrectionSet::identity(lanes)),
            identity: Box::new(CorrectionSet::identity(lanes)),
            out: (0..lanes)
                .map(|_| Vec::with_capacity(ALIGN_BLOCK + CORR_FFT))
                .collect(),
            in_index: None,
            next_input: 0,
            produced: 0,
            transient: CORR_TAPS,
        }
    }

    pub(crate) fn lanes(&self) -> usize {
        self.lanes.len()
    }

    pub(crate) fn swap(
        &mut self,
        next: Box<CorrectionSet>,
    ) -> Result<Box<CorrectionSet>, Box<CorrectionSet>> {
        if !next.fits(self.lanes.len()) {
            return Err(next);
        }
        for (convolver, spectrum) in self.lanes.iter_mut().zip(&next.spectra) {
            if convolver.set_response(spectrum).is_err() {
                return Err(next);
            }
        }
        Ok(std::mem::replace(&mut self.active, next))
    }

    pub(crate) fn load(&mut self, set: &CorrectionSet) -> bool {
        if !set.fits(self.lanes.len()) {
            return false;
        }
        self.active.copy_from(set);
        self.lanes
            .iter_mut()
            .zip(&self.active.spectra)
            .all(|(convolver, spectrum)| convolver.set_response(spectrum).is_ok())
    }

    pub(crate) fn clear_to_identity(&mut self, generation: u32) {
        self.active.copy_from(&self.identity);
        self.active.generation = generation;
        for (convolver, spectrum) in self.lanes.iter_mut().zip(&self.active.spectra) {
            let _ = convolver.set_response(spectrum);
        }
    }

    pub(crate) fn set_generation(&mut self, generation: u32) {
        self.active.generation = generation;
    }

    pub(crate) fn active(&self) -> &CorrectionSet {
        &self.active
    }

    pub(crate) fn view(&self, sample_rate: f64) -> CorrectionView<'_> {
        CorrectionView::new(self.active.generation, sample_rate, &self.active.spectra)
    }

    pub(crate) fn reset(&mut self) {
        for convolver in &mut self.lanes {
            convolver.reset();
        }
        for lane in &mut self.out {
            lane.clear();
        }
        self.in_index = None;
        self.produced = 0;
        self.transient = CORR_TAPS;
    }

    pub(crate) fn push(&mut self, raw: &[&[Complex<f32>]], index: u64) -> usize {
        if self.in_index.is_some() && index != self.next_input {
            self.reset();
        }
        if self.in_index.is_none() {
            self.in_index = Some(index);
        }
        let count = raw.first().map_or(0, |lane| lane.len());
        self.next_input = index + count as u64;
        for ((convolver, lane), out) in self.lanes.iter_mut().zip(raw).zip(&mut self.out) {
            convolver.push(lane, out);
        }
        self.ready()
    }

    pub(crate) fn ready(&self) -> usize {
        self.out.iter().map(Vec::len).min().unwrap_or(0)
    }

    fn first_label(&self) -> i128 {
        i128::from(self.in_index.unwrap_or(0)) + i128::from(self.produced) - CORR_DELAY as i128
    }

    fn lead(&self) -> usize {
        usize::try_from(-self.first_label())
            .unwrap_or(0)
            .min(self.ready())
    }

    pub(crate) fn first_index(&self) -> u64 {
        u64::try_from(self.first_label().max(0)).unwrap_or(u64::MAX)
    }

    pub(crate) fn transient(&self) -> usize {
        self.transient.saturating_sub(self.lead())
    }

    pub(crate) fn with_corrected<R>(&self, f: impl FnOnce(&[&[Complex<f32>]], u64) -> R) -> R {
        let count = self.ready();
        let lead = self.lead();
        let mut view: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
        for (slot, lane) in view.iter_mut().zip(&self.out) {
            *slot = &lane[lead..count];
        }
        f(&view[..self.out.len()], self.first_index())
    }

    pub(crate) fn consume(&mut self) {
        let count = self.ready();
        self.produced += count as u64;
        self.transient = self.transient.saturating_sub(count);
        for lane in &mut self.out {
            lane.clear();
        }
    }
}

#[cfg(test)]
mod tests;
