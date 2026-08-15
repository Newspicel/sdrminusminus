use num_complex::Complex;
use sdrmm_dsp::{Decimator, SymbolSync};

use super::{carrier::CarrierLoop, params::LinearParams, timing::FeedforwardTiming};

pub const TIMING_BW_CONTINUOUS: f64 = 0.003;

pub const TIMING_BW_BURST: f64 = 0.015;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearTiming {
    pub timing_bw: f64,
    pub power_symbols: f64,
}

impl LinearTiming {
    pub const CONTINUOUS: Self = Self {
        timing_bw: TIMING_BW_CONTINUOUS,
        power_symbols: 1_000.0,
    };

    pub const BURST: Self = Self {
        timing_bw: TIMING_BW_BURST,
        power_symbols: f64::INFINITY,
    };
}

const INITIAL_POWER: f32 = 1.0;

struct SymbolStage {
    carrier: Option<CarrierLoop>,
    params: LinearParams,
    power: f32,
    power_alpha: f32,
    symbols_out: u64,
}

impl SymbolStage {
    fn new(params: &LinearParams, power_symbols: f64, carrier: Option<CarrierLoop>) -> Self {
        Self {
            carrier,
            params: params.clone(),
            power: INITIAL_POWER,
            power_alpha: if power_symbols.is_finite() {
                (1.0 / power_symbols) as f32
            } else {
                0.0
            },
            symbols_out: 0,
        }
    }

    fn push(&mut self, y: Complex<f32>) -> Complex<f32> {
        self.power += self.power_alpha * (y.norm_sqr() - self.power);
        let scale = self.power.max(f32::MIN_POSITIVE).sqrt().recip();
        let mut symbol = y * scale;
        if self.params.rotation_rad() != 0.0 {
            let theta =
                -((self.symbols_out as f64 * self.params.rotation_rad()) % std::f64::consts::TAU);
            symbol *= Complex::new(theta.cos() as f32, theta.sin() as f32);
        }
        if let Some(carrier) = &mut self.carrier {
            symbol = carrier.advance(symbol, self.params.constellation());
        }
        self.symbols_out += 1;
        symbol
    }

    fn reset(&mut self) {
        if let Some(carrier) = &mut self.carrier {
            carrier.reset();
        }
        self.power = INITIAL_POWER;
        self.symbols_out = 0;
    }
}

#[derive(Clone, Debug, Default)]
struct Unstagger {
    line: Vec<f32>,
    at: usize,
}

impl Unstagger {
    fn new(len: usize) -> Self {
        Self {
            line: vec![0.0; len],
            at: 0,
        }
    }

    fn apply(&mut self, iq: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        out.clear();
        out.reserve(iq.len());
        if self.line.is_empty() {
            out.extend_from_slice(iq);
            return;
        }
        let n = self.line.len();
        for &s in iq {
            let delayed = self.line[self.at];
            self.line[self.at] = s.re;
            self.at = (self.at + 1) % n;
            out.push(Complex::new(delayed, s.im));
        }
    }

    fn reset(&mut self) {
        self.line.fill(0.0);
        self.at = 0;
    }
}

pub struct LinearDemod {
    matched: Decimator,
    sync: SymbolSync,
    stage: SymbolStage,
    stagger: Unstagger,
    aligned: Vec<Complex<f32>>,
    filtered: Vec<Complex<f32>>,
    retimed: Vec<Complex<f32>>,
}

impl LinearDemod {
    #[must_use]
    pub fn new(
        params: &LinearParams,
        receive_filter: &[f32],
        timing: LinearTiming,
        carrier: Option<CarrierLoop>,
    ) -> Self {
        assert_unit_energy(receive_filter);
        Self {
            matched: Decimator::new(receive_filter, 1),
            sync: SymbolSync::new(params.sps() as f64, timing.timing_bw),
            stage: SymbolStage::new(params, timing.power_symbols, carrier),
            stagger: Unstagger::new(params.stagger_samples()),
            aligned: Vec::new(),
            filtered: Vec::new(),
            retimed: Vec::new(),
        }
    }

    #[must_use]
    pub fn params(&self) -> &LinearParams {
        &self.stage.params
    }

    #[must_use]
    pub fn carrier_freq_cycles_per_symbol(&self) -> f64 {
        self.stage
            .carrier
            .as_ref()
            .map_or(0.0, CarrierLoop::freq_cycles_per_symbol)
    }

    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        self.stagger.apply(iq, &mut self.aligned);
        self.matched.process(&self.aligned, &mut self.filtered);
        self.retimed.clear();
        self.sync.process(&self.filtered, &mut self.retimed);
        for &y in &self.retimed {
            out.push(self.stage.push(y));
        }
    }

    pub fn reset(&mut self) {
        self.sync.reset();
        self.stage.reset();
        self.stagger.reset();
    }
}

pub struct LinearBurstDemod {
    timing: FeedforwardTiming,
    stage: SymbolStage,
    stagger: Unstagger,
    aligned: Vec<Complex<f32>>,
    retimed: Vec<Complex<f32>>,
}

impl LinearBurstDemod {
    #[must_use]
    pub fn new(
        params: &LinearParams,
        receive_filter: &[f32],
        power_symbols: f64,
        carrier: Option<CarrierLoop>,
    ) -> Self {
        Self {
            timing: FeedforwardTiming::new(params, receive_filter),
            stage: SymbolStage::new(params, power_symbols, carrier),
            stagger: Unstagger::new(params.stagger_samples()),
            aligned: Vec::new(),
            retimed: Vec::new(),
        }
    }

    #[must_use]
    pub fn params(&self) -> &LinearParams {
        &self.stage.params
    }

    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<Complex<f32>>) -> f64 {
        self.stagger.apply(iq, &mut self.aligned);
        self.retimed.clear();
        let offset = self.timing.process(&self.aligned, &mut self.retimed);
        for &y in &self.retimed {
            out.push(self.stage.push(y));
        }
        offset
    }

    #[must_use]
    pub fn carrier_freq_cycles_per_symbol(&self) -> f64 {
        self.stage
            .carrier
            .as_ref()
            .map_or(0.0, CarrierLoop::freq_cycles_per_symbol)
    }
}

fn assert_unit_energy(receive_filter: &[f32]) {
    assert!(!receive_filter.is_empty(), "receive filter must have taps");
    let energy: f64 = receive_filter
        .iter()
        .map(|&h| f64::from(h) * f64::from(h))
        .sum();
    assert!(
        (energy - 1.0).abs() < 1e-3,
        "receive filter must be unit-energy (pulse::Norm::Energy), got Σh² = {energy}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ber::{perf::assert_no_alloc, rng::Rng},
        constellation::{Constellation, tables},
        linear::{LinearMod, PhaseDetector},
        pulse::{self, Norm},
    };

    const SPS: usize = 8;

    fn rrc() -> Vec<f32> {
        pulse::root_raised_cosine(SPS as f64, 0.35, 8, Norm::Energy)
    }

    fn min_distance(table: &Constellation) -> f64 {
        let p = table.points();
        let mut min = f64::INFINITY;
        for i in 0..p.len() {
            for j in (i + 1)..p.len() {
                min = min.min(f64::from((p[i] - p[j]).norm()));
            }
        }
        min
    }

    fn labels(n: usize, m: u32, seed: u64) -> Vec<u32> {
        let mut rng = Rng::new(seed);
        (0..n)
            .map(|_| (rng.next_u64() % u64::from(m)) as u32)
            .collect()
    }

    fn settled(
        params: &LinearParams,
        wave: &[Complex<f32>],
        carrier: Option<CarrierLoop>,
    ) -> Vec<Complex<f32>> {
        let mut demod = LinearDemod::new(params, &rrc(), LinearTiming::CONTINUOUS, carrier);
        let mut out = Vec::new();
        demod.process(wave, &mut out);
        let settled_from = out.len() * 3 / 4;
        out.split_off(settled_from)
    }

    #[test]
    fn a_noiseless_loopback_lands_on_the_table() {
        for (name, table) in [
            ("bpsk", tables::pam(2).unwrap()),
            ("qpsk", tables::qam_square(4).unwrap()),
            ("16-qam", tables::qam_square(16).unwrap()),
            ("8-psk", tables::psk(8).unwrap()),
            ("cross-32", tables::qam_cross(32).unwrap()),
        ] {
            let m = table.len() as u32;
            let p = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
            let sent = labels(12_000, m, 0xd0d0);
            let wave = LinearMod::transmission(&p, &sent);
            let got = settled(&p, &wave, None);
            let rms = (got
                .iter()
                .map(|&y| {
                    let l = table.hard_slice(y);
                    let i = table.labels().iter().position(|&x| x == l).unwrap();
                    f64::from((y - table.points()[i]).norm_sqr())
                })
                .sum::<f64>()
                / got.len() as f64)
                .sqrt();
            let margin = min_distance(&table) / 2.0;
            assert!(
                rms < 0.1 * margin,
                "{name}: residual {rms} RMS, slicing margin {margin}"
            );
        }
    }

    #[test]
    fn every_noiseless_symbol_slices_back_to_its_label() {
        let table = tables::qam_square(16).unwrap();
        let p = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
        let sent = labels(4_000, 16, 0x51ce);
        let wave = LinearMod::transmission(&p, &sent);
        let got = settled(&p, &wave, None);
        let offset = sent.len() - got.len();
        let wrong = got
            .iter()
            .zip(&sent[offset..])
            .filter(|&(&y, &want)| table.hard_slice(y) != want)
            .count();
        assert_eq!(wrong, 0, "{wrong} of {} symbols mis-sliced", got.len());
    }

    #[test]
    fn oqpsk_recovers_through_the_unstagger() {
        let table = tables::qam_square(4).unwrap();
        let sent = labels(3_000, 4, 0x0954);
        let plain = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
        let offset = plain.clone().with_offset(true).unwrap();
        for (name, p) in [("qpsk", plain), ("oqpsk", offset)] {
            let wave = LinearMod::transmission(&p, &sent);
            let got = settled(&p, &wave, None);
            let start = sent.len() - got.len();
            let wrong = got
                .iter()
                .zip(&sent[start..])
                .filter(|&(&y, &want)| table.hard_slice(y) != want)
                .count();
            assert_eq!(wrong, 0, "{name}: {wrong} mis-sliced");
        }
    }

    #[test]
    fn ignoring_the_stagger_wrecks_the_constellation() {
        let table = tables::qam_square(4).unwrap();
        let sent = labels(2_000, 4, 0x0954);
        let staggered = LinearParams::new(table.clone(), rrc(), SPS)
            .unwrap()
            .with_offset(true)
            .unwrap();
        let wave = LinearMod::transmission(&staggered, &sent);
        let plain = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
        let got = settled(&plain, &wave, None);
        let start = sent.len() - got.len();
        let wrong = got
            .iter()
            .zip(&sent[start..])
            .filter(|&(&y, &want)| table.hard_slice(y) != want)
            .count();
        assert!(
            wrong > got.len() / 5,
            "only {wrong} of {} mis-sliced without the unstagger",
            got.len()
        );
    }

    #[test]
    fn the_rotation_schedule_is_undone_at_the_receiver() {
        let table = tables::pam(2).unwrap();
        let p = LinearParams::new(table.clone(), rrc(), SPS)
            .unwrap()
            .with_rotation(tables::PI_2_ROTATION)
            .unwrap();
        let sent = labels(3_000, 2, 0x9020);
        let wave = LinearMod::transmission(&p, &sent);
        let got = settled(&p, &wave, None);
        let start = sent.len() - got.len();
        let wrong = got
            .iter()
            .zip(&sent[start..])
            .filter(|&(&y, &want)| table.hard_slice(y) != want)
            .count();
        assert_eq!(wrong, 0, "{wrong} mis-sliced");
    }

    #[test]
    fn the_carrier_loop_recovers_an_offset_the_open_chain_cannot() {
        let table = tables::psk(4).unwrap();
        let p = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
        let sent = labels(6_000, 4, 0x0ff5);
        let mut wave = LinearMod::transmission(&p, &sent);
        for (n, s) in wave.iter_mut().enumerate() {
            let theta = std::f64::consts::TAU * 1e-4 * n as f64;
            *s *= Complex::new(theta.cos() as f32, theta.sin() as f32);
        }
        let spread = |symbols: &[Complex<f32>]| -> f64 {
            symbols
                .iter()
                .map(|&y| {
                    let l = table.hard_slice(y);
                    let i = table.labels().iter().position(|&x| x == l).unwrap();
                    f64::from((y - table.points()[i]).norm())
                })
                .sum::<f64>()
                / symbols.len() as f64
        };
        let open = settled(&p, &wave, None);
        let locked = settled(
            &p,
            &wave,
            Some(CarrierLoop::new(PhaseDetector::MthPower { m: 4 }, 0.01)),
        );
        assert!(spread(&locked) < 0.05, "locked spread {}", spread(&locked));
        assert!(spread(&open) > 0.3, "open-loop spread {}", spread(&open));
    }

    #[test]
    fn any_block_split_gives_the_same_symbols() {
        let table = tables::qam_square(16).unwrap();
        let p = LinearParams::new(table, rrc(), SPS)
            .unwrap()
            .with_offset(true)
            .unwrap();
        let wave = LinearMod::transmission(&p, &labels(1_500, 16, 0x5b17));
        let carrier = || Some(CarrierLoop::new(PhaseDetector::DecisionDirected, 0.01));
        let mut whole = LinearDemod::new(&p, &rrc(), LinearTiming::CONTINUOUS, carrier());
        let mut a = Vec::new();
        whole.process(&wave, &mut a);
        let mut split = LinearDemod::new(&p, &rrc(), LinearTiming::CONTINUOUS, carrier());
        let mut b = Vec::new();
        for chunk in wave.chunks(311) {
            split.process(chunk, &mut b);
        }
        assert_eq!(a, b);
    }

    #[test]
    fn steady_state_allocates_nothing() {
        for (name, offset) in [("plain", false), ("staggered", true)] {
            let p = LinearParams::new(tables::qam_square(16).unwrap(), rrc(), SPS)
                .unwrap()
                .with_offset(offset)
                .unwrap();
            let wave = LinearMod::transmission(&p, &labels(2_048, 16, 0x0a11));
            let mut demod = LinearDemod::new(
                &p,
                &rrc(),
                LinearTiming::CONTINUOUS,
                Some(CarrierLoop::new(PhaseDetector::DecisionDirected, 0.01)),
            );
            let mut out = Vec::with_capacity(wave.len());
            demod.process(&wave, &mut out);
            out.clear();
            demod.process(&wave, &mut out);
            out.clear();
            assert_no_alloc(&format!("LinearDemod::process ({name})"), || {
                demod.process(&wave, &mut out);
            });
            assert!(
                !out.is_empty(),
                "{name}: the measured call recovered nothing"
            );
        }
    }
}
