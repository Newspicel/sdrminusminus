use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use sdrmm_dsp::Soft;
use sdrmm_wire::DatvCodeRate;

use super::{
    acquire::{self, Timing},
    mapping::{self, Mapping},
    tps::{Parameters, Tps},
};
use crate::datv::dvbs::{DvbsDecoder, DvbsMetrics, PACKET};

pub struct Receiver {
    maps: [Mapping; 2],
    transforms: [Arc<dyn Fft<f32>>; 2],
    scratch: Vec<Complex<f32>>,
    pending: Vec<Complex<f32>>,
    spectrum: Vec<Complex<f32>>,
    estimates: Vec<Complex<f32>>,
    previous_tps: Vec<Complex<f32>>,
    words: Vec<[Soft; 6]>,
    soft: Vec<Soft>,
    tables: [Vec<Complex<f32>>; 9],
    tps: Tps,
    timing: Option<Timing>,
    integer_offset: Option<isize>,
    symbol: usize,
    since_tps: usize,
    losses: usize,
    fec: DvbsDecoder,
    pub parameters: Option<Parameters>,
    pub low_priority: bool,
    pub snr: f32,
    pub frequency: f32,
    pub bad_symbols: u32,
}

impl Receiver {
    pub fn new(low_priority: bool) -> Self {
        let mut planner = FftPlanner::new();
        let transforms = [
            planner.plan_fft_forward(2048),
            planner.plan_fft_forward(8192),
        ];
        let scratch_len = transforms
            .iter()
            .map(|fft| fft.get_inplace_scratch_len())
            .max()
            .unwrap_or(8192);
        Self {
            maps: [Mapping::new(2048), Mapping::new(8192)],
            transforms,
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            pending: Vec::with_capacity(65536),
            spectrum: vec![Complex::new(0.0, 0.0); 8192],
            estimates: vec![Complex::new(0.0, 0.0); 6817],
            previous_tps: vec![Complex::new(0.0, 0.0); 68],
            words: vec![[0; 6]; 6048],
            soft: Vec::with_capacity(6048 * 6),
            tables: std::array::from_fn(|i| {
                let bits = 2 + 2 * (i / 3);
                let alpha = 1 << (i % 3);
                (0..1 << bits)
                    .map(|w| mapping::point(w, bits, alpha))
                    .collect()
            }),
            tps: Tps::default(),
            timing: None,
            integer_offset: None,
            symbol: 0,
            since_tps: 0,
            losses: 0,
            fec: DvbsDecoder::new(DatvCodeRate::Auto, 1_000_000.0),
            parameters: None,
            low_priority,
            snr: 0.0,
            frequency: 0.0,
            bad_symbols: 0,
        }
    }

    pub fn reset(&mut self) {
        self.pending.clear();
        self.timing = None;
        self.integer_offset = None;
        self.parameters = None;
        self.tps = Tps::default();
        self.previous_tps.fill(Complex::new(0.0, 0.0));
        self.fec.reset();
        self.losses = 0;
        self.since_tps = 0;
        self.snr = 0.0;
        self.frequency = 0.0;
    }

    pub fn metrics(&self) -> DvbsMetrics {
        self.fec.metrics()
    }
    pub fn locked(&self) -> bool {
        self.parameters.is_some() && self.fec.running() && self.since_tps < 136
    }

    pub fn push(&mut self, iq: &[Complex<f32>], packets: &mut Vec<[u8; PACKET]>) {
        for block in iq.chunks(4096) {
            self.pending.extend_from_slice(block);
            if self.timing.is_none() {
                if self.pending.len() < 51200 {
                    continue;
                }
                if let Some(timing) = acquire::acquire(&self.pending) {
                    if timing.at >= 16 {
                        self.pending.drain(..timing.at - 16);
                    } else {
                        self.pending.splice(
                            ..0,
                            std::iter::repeat_n(Complex::new(0.0, 0.0), 16 - timing.at),
                        );
                    }
                    self.timing = Some(Timing { at: 16, ..timing });
                } else {
                    self.pending.drain(..16384);
                    continue;
                }
            }
            while let Some(timing) = self.timing {
                if self.pending.len() < timing.fft + timing.guard + 48 {
                    break;
                }
                self.demodulate(timing, packets);
            }
        }
    }

    fn demodulate(&mut self, mut timing: Timing, packets: &mut Vec<[u8; PACKET]>) {
        let index = usize::from(timing.fft == 8192);
        let mut best = (0.0, 0.0, 16);
        for at in 8..=24 {
            let (quality, frequency) =
                acquire::correlation(&self.pending, timing.fft, timing.guard, at);
            if quality > best.0 {
                best = (quality, frequency, at);
            }
        }
        let (quality, frequency, at) = best;
        if quality < 0.5 {
            self.losses += 1;
            self.bad_symbols = self.bad_symbols.saturating_add(1);
        } else {
            self.losses = 0;
        }
        if self.losses >= 4 || self.since_tps > 272 {
            self.reset();
            return;
        }
        timing.offset += 0.1 * (frequency - timing.offset);
        self.snr = (-10.0 * ((1.0 - quality) / quality.max(1e-6)).max(1e-6).log10()).max(0.0);
        let early = timing.guard / 4;
        let window = at + timing.guard - early;
        for (i, value) in self.spectrum[..timing.fft].iter_mut().enumerate() {
            *value = self.pending[window + i] * Complex::from_polar(1.0, -timing.offset * i as f32);
        }
        self.transforms[index]
            .process_with_scratch(&mut self.spectrum[..timing.fft], &mut self.scratch);
        for (bin, value) in self.spectrum[..timing.fft].iter_mut().enumerate() {
            let carrier = if bin < timing.fft / 2 {
                bin as isize
            } else {
                bin as isize - timing.fft as isize
            };
            *value *= Complex::from_polar(
                1.0,
                std::f32::consts::TAU * carrier as f32 * early as f32 / timing.fft as f32,
            );
        }
        if self.integer_offset.is_none() {
            self.integer_offset = Some(self.carrier_offset(index));
        }
        let offset = self.integer_offset.unwrap_or(0);
        self.frequency = timing.offset + offset as f32 * std::f32::consts::TAU / timing.fft as f32;
        let phase = self.pilot_phase(index, offset);
        self.equalize(index, phase, offset);
        self.read_tps(index, offset, timing);
        if let Some(params) = self.parameters {
            if self.symbol % 4 != phase {
                self.fec.reset();
                self.bad_symbols = self.bad_symbols.saturating_add(1);
            } else {
                self.decode(index, phase, offset, params, packets);
            }
        }
        self.symbol = (self.symbol + 1) % 68;
        self.since_tps += 1;
        let period = timing.fft + timing.guard;
        self.pending.drain(..period + at - 16);
        self.timing = Some(timing);
    }

    fn carrier_offset(&self, index: usize) -> isize {
        let map = &self.maps[index];
        let mut best = (f32::NEG_INFINITY, 0);
        for offset in -64..=64 {
            let mut score = 0.0;
            for pair in map
                .continual
                .windows(2)
                .filter(|pair| pair[1] - pair[0] < 50)
            {
                let a = self.spectrum[map.bin(pair[0], offset)] * map.reference[pair[0]];
                let b = self.spectrum[map.bin(pair[1], offset)] * map.reference[pair[1]];
                score += (b * a.conj()).re;
            }
            if score > best.0 {
                best = (score, offset);
            }
        }
        best.1
    }

    fn pilot_phase(&self, index: usize, offset: isize) -> usize {
        let map = &self.maps[index];
        let mut best = (f32::NEG_INFINITY, 0);
        for phase in 0..4 {
            let mut score = 0.0;
            for k in (3 * phase..map.carriers - 12).step_by(12) {
                let a = self.spectrum[map.bin(k, offset)] * map.reference[k];
                let b = self.spectrum[map.bin(k + 12, offset)] * map.reference[k + 12];
                score += (b * a.conj()).re;
            }
            if score > best.0 {
                best = (score, phase);
            }
        }
        best.1
    }

    fn equalize(&mut self, index: usize, phase: usize, offset: isize) {
        let map = &self.maps[index];
        for &k in &map.pilots[phase] {
            self.estimates[k] = self.spectrum[map.bin(k, offset)] * (0.75 * map.reference[k]);
        }
        for pair in map.pilots[phase].windows(2) {
            let a = self.estimates[pair[0]];
            let delta = (self.estimates[pair[1]] - a) / (pair[1] - pair[0]) as f32;
            for k in pair[0] + 1..pair[1] {
                self.estimates[k] = a + delta * (k - pair[0]) as f32;
            }
        }
    }

    fn read_tps(&mut self, index: usize, offset: isize, timing: Timing) {
        let map = &self.maps[index];
        let mut differential = 0.0;
        for (i, &k) in map.tps.iter().enumerate() {
            let h = self.estimates[k];
            let point = self.spectrum[map.bin(k, offset)] * h.conj() / h.norm_sqr().max(1e-12);
            differential += (point * self.previous_tps[i].conj()).re;
            self.previous_tps[i] = point;
        }
        if let Some(params) = self.tps.push(differential < 0.0) {
            if params.fft != timing.fft || params.guard != timing.guard {
                return;
            }
            if self
                .parameters
                .is_none_or(|current| !current.same_modulation(params))
            {
                self.fec.reset();
            }
            self.parameters = Some(params);
            self.symbol = 67;
            self.since_tps = 0;
        }
    }

    fn decode(
        &mut self,
        index: usize,
        phase: usize,
        offset: isize,
        params: Parameters,
        packets: &mut Vec<[u8; PACKET]>,
    ) {
        let map = &self.maps[index];
        let table = &self.tables[(params.bits / 2 - 1) * 3 + params.alpha.ilog2() as usize];
        for (i, &k) in map.data[phase].iter().enumerate() {
            let h = self.estimates[k];
            let point = self.spectrum[map.bin(k, offset)] * h.conj() / h.norm_sqr().max(1e-12);
            self.words[i] = mapping::soften(point, table, params.bits);
        }
        let mut reordered = [[0; 6]; 6048];
        for (i, &p) in map.permutation.iter().enumerate() {
            if self.symbol.is_multiple_of(2) {
                reordered[i] = self.words[p];
            } else {
                reordered[p] = self.words[i];
            }
        }
        mapping::deinterleave(
            &reordered[..map.permutation.len()],
            params.bits,
            params.hierarchical,
            self.low_priority,
            &mut self.soft,
        );
        let rate = if params.hierarchical && self.low_priority {
            params.low_rate
        } else {
            params.high_rate
        };
        self.fec.push_soft(&self.soft, rate, packets);
    }
}
