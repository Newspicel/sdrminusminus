use num_complex::Complex;

use super::{
    OtfsGrid, OtfsPrecoder,
    estimate::{DelayDopplerFit, Support},
};
use crate::{
    constellation::Constellation,
    ofdm::{Acquisition, MIN_NOISE_VAR, OfdmDemod, OfdmMod, OfdmParams, PilotFit},
};

pub const DEFAULT_ITERATIONS: usize = 8;
pub const LEAD_TAPS: i32 = 2;
pub const DOPPLER_ORDER: usize = 3;

#[derive(Clone)]
pub struct OtfsMod {
    carrier: OfdmMod,
    precoder: OtfsPrecoder,
    tf: Vec<Complex<f32>>,
}

impl OtfsMod {
    #[must_use]
    pub fn new(params: OfdmParams, grid: OtfsGrid) -> Self {
        assert_eq!(
            grid.delay,
            params.data_subcarriers(),
            "the delay axis spans the data subcarriers"
        );
        Self {
            carrier: OfdmMod::new(params),
            precoder: OtfsPrecoder::new(grid),
            tf: vec![Complex::new(0.0, 0.0); grid.points()],
        }
    }

    pub fn frame(&mut self, dd: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        self.precoder.spread(dd, &mut self.tf);
        self.carrier.frame(&self.tf, out);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Data(usize),
    Pilot(usize),
    Silent,
}

#[derive(Clone)]
pub struct OtfsReceiver {
    grid: OtfsGrid,
    carrier: OfdmDemod,
    precoder: OtfsPrecoder,
    table: Constellation,
    fit: DelayDopplerFit,
    slots: Vec<Slot>,
    offsets: Vec<i32>,
    iterations: usize,
    received: Vec<Complex<f32>>,
    known: Vec<Complex<f32>>,
    channel: Vec<Complex<f32>>,
    tf: Vec<Complex<f32>>,
    dd: Vec<Complex<f32>>,
    decided: Vec<Complex<f32>>,
    passes: usize,
    noise_var: f64,
    acquired: bool,
}

impl OtfsReceiver {
    #[must_use]
    pub fn new(params: OfdmParams, grid: OtfsGrid, table: Constellation) -> Self {
        assert_eq!(
            grid.delay,
            params.data_subcarriers(),
            "the delay axis spans the data subcarriers"
        );
        let slots = slots(&params);
        let offsets: Vec<i32> = params.map().occupied().iter().map(|c| c.offset).collect();
        let support = Support {
            first_tap: -LEAD_TAPS,
            last_tap: params.cp() as i32,
            doppler_order: DOPPLER_ORDER,
        };
        let cells = grid.doppler * offsets.len();
        Self {
            fit: DelayDopplerFit::new(&offsets, params.fft(), grid.doppler, support),
            carrier: OfdmDemod::new(params),
            precoder: OtfsPrecoder::new(grid),
            table,
            slots,
            offsets,
            iterations: DEFAULT_ITERATIONS,
            received: vec![Complex::new(0.0, 0.0); cells],
            known: vec![Complex::new(0.0, 0.0); cells],
            channel: vec![Complex::new(0.0, 0.0); cells],
            tf: vec![Complex::new(0.0, 0.0); grid.points()],
            dd: vec![Complex::new(0.0, 0.0); grid.points()],
            decided: vec![Complex::new(0.0, 0.0); grid.points()],
            passes: 0,
            noise_var: MIN_NOISE_VAR,
            acquired: false,
            grid,
        }
    }

    #[must_use]
    pub fn with_iterations(mut self, iterations: usize) -> Self {
        self.iterations = iterations;
        self
    }

    #[must_use]
    pub fn grid(&self) -> OtfsGrid {
        self.grid
    }

    #[must_use]
    pub fn carrier(&self) -> &OfdmDemod {
        &self.carrier
    }

    #[must_use]
    pub fn passes(&self) -> usize {
        self.passes
    }

    #[must_use]
    pub fn noise_var(&self) -> f64 {
        self.noise_var
    }

    #[must_use]
    pub fn channel(&self) -> &[Complex<f32>] {
        &self.channel
    }

    pub fn acquire(&mut self, x: &[Complex<f32>], search: usize) -> Option<Acquisition> {
        let acquisition = self.carrier.acquire(x, search);
        self.acquired = acquisition.is_some();
        acquisition
    }

    pub fn demodulate(&mut self, x: &[Complex<f32>], out: &mut Vec<Complex<f32>>) -> usize {
        if !self.acquired {
            return 0;
        }
        self.observe(x);
        self.passes = 0;
        loop {
            self.equalise();
            self.precoder.despread(&self.tf, &mut self.dd);
            self.passes += 1;
            if self.passes > self.iterations || !self.decide() {
                break;
            }
            self.refine();
        }
        out.extend_from_slice(&self.dd);
        self.dd.len()
    }

    fn decide(&mut self) -> bool {
        let mut changed = self.passes == 1;
        for (slot, &soft) in self.decided.iter_mut().zip(&self.dd) {
            let hard = self.table.nearest(soft);
            changed |= *slot != hard;
            *slot = hard;
        }
        changed
    }

    fn observe(&mut self, x: &[Complex<f32>]) {
        let o = self.offsets.len();
        for symbol in 0..self.grid.doppler {
            let row = symbol * o..(symbol + 1) * o;
            let fit = self
                .carrier
                .observe(x, symbol, &mut self.received[row.clone()]);
            self.initial_row(symbol, fit);
        }
        self.noise_var = self.carrier.channel().noise_var().max(MIN_NOISE_VAR);
    }

    fn initial_row(&mut self, symbol: usize, fit: PilotFit) {
        let o = self.offsets.len();
        let occupied = self.carrier.params().map().occupied();
        for (k, c) in occupied.iter().enumerate() {
            let turn = Complex::from_polar(1.0f32, fit.phase_at(c.offset) as f32);
            self.channel[symbol * o + k] = self.carrier.channel().h(c.bin) * turn;
        }
    }

    fn equalise(&mut self) {
        let o = self.offsets.len();
        let data = self.grid.delay;
        let nv = self.noise_var as f32;
        let mut bias = 0.0f64;
        for symbol in 0..self.grid.doppler {
            for (k, slot) in self.slots.iter().enumerate() {
                let Slot::Data(index) = *slot else { continue };
                let h = self.channel[symbol * o + k];
                let gain = h.norm_sqr();
                let y = self.received[symbol * o + k];
                self.tf[symbol * data + index] = y * h.conj() / (gain + nv);
                bias += f64::from(gain / (gain + nv));
            }
        }
        let mean = (bias / self.tf.len() as f64).max(f64::MIN_POSITIVE) as f32;
        for v in &mut self.tf {
            *v /= mean;
        }
    }

    fn refine(&mut self) {
        self.precoder.spread(&self.decided, &mut self.tf);
        self.fill_known();
        self.noise_var = self
            .fit
            .fit(
                &self.received,
                &self.known,
                self.noise_var,
                &mut self.channel,
            )
            .max(MIN_NOISE_VAR);
    }

    fn fill_known(&mut self) {
        let o = self.offsets.len();
        let data = self.grid.delay;
        let pattern = self.carrier.params().pilot_pattern();
        for symbol in 0..self.grid.doppler {
            for (k, slot) in self.slots.iter().enumerate() {
                self.known[symbol * o + k] = match *slot {
                    Slot::Data(index) => self.tf[symbol * data + index],
                    Slot::Pilot(index) => pattern.value(index, symbol),
                    Slot::Silent => Complex::new(0.0, 0.0),
                };
            }
        }
    }
}

fn slots(params: &OfdmParams) -> Vec<Slot> {
    let map = params.map();
    map.occupied()
        .iter()
        .map(|c| {
            map.data()
                .iter()
                .position(|d| d.bin == c.bin)
                .map(Slot::Data)
                .or_else(|| {
                    map.pilots()
                        .iter()
                        .position(|p| p.bin == c.bin)
                        .map(Slot::Pilot)
                })
                .unwrap_or(Slot::Silent)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use sdrmm_modem_test_support::ber::{
        impair::{Awgn, Cfo, Impairment},
        perf::assert_no_alloc,
        rng::Rng,
    };

    use super::*;
    use crate::constellation::tables;

    const SYMBOLS: usize = 16;

    fn params() -> OfdmParams {
        OfdmParams::wifi_like()
    }

    fn grid() -> OtfsGrid {
        OtfsGrid::new(params().data_subcarriers(), SYMBOLS)
    }

    fn qpsk() -> Constellation {
        tables::qam_square(4).unwrap()
    }

    fn payload(seed: u32) -> Vec<Complex<f32>> {
        let table = qpsk();
        let mut state = seed | 1;
        (0..grid().points())
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                table.points()[(state % 4) as usize]
            })
            .collect()
    }

    fn burst(lead: usize, seed: u32) -> (Vec<Complex<f32>>, Vec<Complex<f32>>) {
        let sent = payload(seed);
        let mut wave = vec![Complex::new(0.0, 0.0); lead];
        OtfsMod::new(params(), grid()).frame(&sent, &mut wave);
        wave.resize(wave.len() + 128, Complex::new(0.0, 0.0));
        (sent, wave)
    }

    fn paths(wave: &[Complex<f32>], taps: &[(usize, f32, f64, f64)]) -> Vec<Complex<f32>> {
        (0..wave.len())
            .map(|n| {
                taps.iter()
                    .filter(|&&(delay, ..)| n >= delay)
                    .map(|&(delay, gain, phase, doppler)| {
                        let angle = phase + TAU * doppler * n as f64;
                        wave[n - delay] * Complex::from_polar(gain, angle as f32)
                    })
                    .sum()
            })
            .collect()
    }

    fn errors(sent: &[Complex<f32>], got: &[Complex<f32>]) -> usize {
        let table = qpsk();
        let wrong = sent
            .iter()
            .zip(got)
            .filter(|(a, b)| table.hard_slice(**a) != table.hard_slice(**b))
            .count();
        wrong + sent.len().saturating_sub(got.len())
    }

    fn decode(rx: &mut OtfsReceiver, wave: &[Complex<f32>]) -> Vec<Complex<f32>> {
        assert!(rx.acquire(wave, 200).is_some(), "no preamble found");
        let mut out = Vec::new();
        assert_eq!(rx.demodulate(wave, &mut out), grid().points());
        out
    }

    fn receiver() -> OtfsReceiver {
        OtfsReceiver::new(params(), grid(), qpsk())
    }

    #[test]
    fn a_clean_frame_decodes_from_an_unknown_start() {
        for lead in [0usize, 9, 77, 150] {
            let (sent, wave) = burst(lead, 0x71);
            let mut rx = receiver();
            let got = decode(&mut rx, &wave);
            assert_eq!(errors(&sent, &got), 0, "lead {lead}");
            assert!(
                rx.passes() <= 2,
                "a clean frame took {} passes",
                rx.passes()
            );
        }
    }

    #[test]
    fn a_carrier_offset_is_acquired_and_removed() {
        for cfo in [-0.025, -0.004, 0.0007, 0.013, 0.028] {
            let (sent, mut wave) = burst(21, 0x72);
            Cfo::from_cycles_per_sample(cfo).apply(&mut wave, &mut Rng::new(0));
            let mut rx = receiver();
            let got = decode(&mut rx, &wave);
            assert_eq!(errors(&sent, &got), 0, "cfo {cfo}");
            assert!((rx.carrier().cfo() - cfo).abs() < 1e-4);
        }
    }

    #[test]
    fn multipath_inside_the_prefix_is_equalised() {
        let (sent, wave) = burst(31, 0x73);
        let wave = paths(
            &wave,
            &[(0, 0.8, 0.0, 0.0), (3, 0.5, 1.1, 0.0), (9, 0.3, -2.0, 0.0)],
        );
        let got = decode(&mut receiver(), &wave);
        assert_eq!(errors(&sent, &got), 0);
    }

    #[test]
    fn paths_with_opposite_doppler_are_tracked_through_the_frame() {
        let (sent, clean) = burst(17, 0x74);
        let wave = paths(&clean, &[(0, 0.8, 0.3, 1.5e-4), (5, 0.6, -1.2, -1.5e-4)]);
        let mut noisy = wave.clone();
        Awgn::with_sigma(0.05).apply(&mut noisy, &mut Rng::new(0x74));
        let tracked = errors(&sent, &decode(&mut receiver(), &noisy));
        let static_only = errors(&sent, &decode(&mut receiver().with_iterations(0), &noisy));
        assert_eq!(tracked, 0);
        assert!(
            static_only > sent.len() / 50,
            "the preamble estimate alone should not survive: {static_only}"
        );
    }

    #[test]
    fn the_refined_noise_estimate_reads_the_channel_noise() {
        let (_, mut wave) = burst(12, 0x75);
        Awgn::with_sigma(0.15).apply(&mut wave, &mut Rng::new(0x75));
        let mut rx = receiver();
        let _ = decode(&mut rx, &wave);
        let want = 2.0 * 0.15 * 0.15;
        assert!(
            (rx.noise_var() / want - 1.0).abs() < 0.3,
            "noise {} want {want}",
            rx.noise_var()
        );
    }

    #[test]
    fn demodulation_does_not_allocate() {
        let (_, wave) = burst(5, 0x76);
        let mut rx = receiver();
        assert!(rx.acquire(&wave, 200).is_some());
        let mut out = Vec::with_capacity(grid().points());
        assert_no_alloc("otfs demodulate", || {
            rx.demodulate(&wave, &mut out);
        });
    }
}
