use num_complex::Complex;
use sdrmm_modem_test_support::ber::{
    impair::{Cfo, ClockError, Impairment, TimingOffset},
    rng::Rng,
};

use super::{CssDemod, CssMod, CssParams};

const PREAMBLE: usize = 16;

const PAYLOAD: usize = 160;

const LEAD: usize = 23;

const EU_CARRIER_HZ: f64 = 868.1e6;

const BANDWIDTH_HZ: f64 = 125e3;

fn symbols(n: usize, count: usize, seed: u64) -> Vec<u32> {
    let mut rng = Rng::new(seed);
    (0..count)
        .map(|_| (rng.next_u64() % n as u64) as u32)
        .collect()
}

struct Burst {
    params: CssParams,
    preamble: Vec<u32>,
    payload: Vec<u32>,
    wave: Vec<Complex<f32>>,
}

impl Burst {
    fn new(spreading_factor: u32, seed: u64) -> Self {
        let params = CssParams::new(spreading_factor);
        let n = params.chips();
        let preamble = symbols(n, PREAMBLE, seed);
        let payload = symbols(n, PAYLOAD, seed ^ 0xa5a5);
        let mut wave = vec![Complex::new(0.0, 0.0); LEAD];
        CssMod::new(params.clone()).frame(&preamble, &payload, &mut wave);
        wave.resize(wave.len() + LEAD, Complex::new(0.0, 0.0));
        Self {
            params,
            preamble,
            payload,
            wave,
        }
    }

    fn impair(mut self, impairment: &impl Impairment) -> Self {
        impairment.apply(&mut self.wave, &mut Rng::new(1));
        self
    }

    fn noisy(mut self, es_n0_db: f64, seed: u64) -> Self {
        let n = self.params.chips() as f64;
        let sigma = (0.5 / (n * 10f64.powf(es_n0_db / 10.0))).sqrt();
        let mut rng = Rng::new(seed);
        for s in &mut self.wave {
            *s += Complex::new((rng.normal() * sigma) as f32, (rng.normal() * sigma) as f32);
        }
        self
    }

    fn receive(&self) -> (CssDemod, usize, usize) {
        let mut demod = CssDemod::new(self.params.clone());
        let origin = demod.estimate_origin(&self.wave, &self.preamble);
        let mut got = Vec::new();
        demod.demodulate(
            &self.wave,
            origin + PREAMBLE * self.params.chips(),
            self.payload.len(),
            &mut got,
        );
        let errors = got
            .iter()
            .zip(&self.payload)
            .filter(|(a, b)| a != b)
            .count();
        (demod, origin, errors)
    }
}

fn shared_crystal(ppm: f64) -> Cfo {
    Cfo::from_hz(ppm * 1e-6 * EU_CARRIER_HZ, BANDWIDTH_HZ)
}

#[test]
fn a_half_sample_timing_offset_keeps_every_symbol() {
    for sf in [7, 10] {
        let burst = Burst::new(sf, 0x7a11 + u64::from(sf))
            .impair(&TimingOffset::new(0.5))
            .noisy(14.0, 0x7a12);
        let (_, origin, errors) = burst.receive();
        assert!(origin.abs_diff(LEAD) <= 1, "SF{sf} origin {origin}");
        assert_eq!(errors, 0, "SF{sf}");
    }
}

#[test]
fn a_carrier_offset_near_half_the_band_is_told_apart_from_timing() {
    let n = 128.0;
    for bins in [-60.3, -21.5, 37.8, 61.2] {
        let burst = Burst::new(7, 0xc0f0)
            .impair(&TimingOffset::new(0.3))
            .impair(&Cfo::from_cycles_per_sample(bins / n))
            .noisy(16.0, 0xc0f1);
        let (demod, origin, errors) = burst.receive();
        assert_eq!(origin, LEAD, "{bins} bins");
        assert_eq!(errors, 0, "{bins} bins");
        assert!(
            (demod.offset_bins() - bins).abs() < 0.1,
            "{bins} bins read as {}",
            demod.offset_bins()
        );
    }
}

#[test]
fn a_clock_far_off_any_crystal_is_measured_and_tracked() {
    for (sf, ppm) in [
        (7, 1_000.0),
        (9, -1_000.0),
        (10, 500.0),
        (11, -300.0),
        (12, 300.0),
    ] {
        let burst = Burst::new(sf, 0xc10c + u64::from(sf))
            .impair(&ClockError::new(ppm))
            .noisy(18.0, 0xc10d);
        let (demod, _, errors) = burst.receive();
        assert_eq!(errors, 0, "SF{sf} at {ppm} ppm");
        assert!(
            (demod.clock_ppm() - ppm).abs() < 0.05 * ppm.abs(),
            "SF{sf} at {ppm} ppm read {}",
            demod.clock_ppm()
        );
    }
}

#[test]
fn one_crystal_moving_carrier_and_clock_together_is_followed() {
    for (sf, ppm) in [(7, 70.0), (12, -70.0), (12, 50.0)] {
        let burst = Burst::new(sf, 0x5c75 + u64::from(sf))
            .impair(&ClockError::new(ppm))
            .impair(&shared_crystal(ppm))
            .noisy(18.0, 0x5c76);
        let (demod, _, errors) = burst.receive();
        assert_eq!(errors, 0, "SF{sf} at {ppm} ppm");
        assert!(
            (demod.clock_ppm() - ppm).abs() < 5.0,
            "SF{sf} at {ppm} ppm read {}",
            demod.clock_ppm()
        );
    }
}

#[test]
fn a_clock_offset_is_tracked_again_on_a_second_pass() {
    let burst = Burst::new(12, 0x2e2e).impair(&ClockError::new(-200.0));
    let (mut demod, origin, errors) = burst.receive();
    assert_eq!(errors, 0);
    let mut again = Vec::new();
    demod.demodulate(
        &burst.wave,
        origin + PREAMBLE * burst.params.chips(),
        burst.payload.len(),
        &mut again,
    );
    assert_eq!(again, burst.payload);
}
