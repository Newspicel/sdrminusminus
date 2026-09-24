use std::{f64::consts::PI, fmt};

use num_complex::Complex;

use super::params::CpmParams;
use crate::{
    constellation::tables,
    linear::{CarrierLoop, LinearBurstDemod, LinearParams, PhaseDetector, TimingMetric},
    pulse::phase_pulse,
};

pub const QUARTER_TURN: f64 = PI / 2.0;

const H_TOLERANCE: f64 = 1e-3;

const POWER_SYMBOLS: f64 = 64.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LaurentError {
    NotBinary(usize),
    NotHalfIndex(f64),
    FractionalSps(f64),
}

impl fmt::Display for LaurentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotBinary(m) => write!(f, "coherent detection needs 2 levels, got {m}"),
            Self::NotHalfIndex(h) => write!(f, "coherent detection needs h = 0.5, got {h}"),
            Self::FractionalSps(sps) => {
                write!(
                    f,
                    "coherent detection needs whole samples per symbol, got {sps}"
                )
            }
        }
    }
}

impl std::error::Error for LaurentError {}

pub fn laurent_main_pulse(params: &CpmParams) -> Result<Vec<f32>, LaurentError> {
    let sps = whole_sps(params)?;
    let q = phase_pulse(params.freq_pulse());
    let memory = q.len().div_ceil(sps).max(1);
    let span = memory * sps;
    let h = params.h();
    let q_at = |n: isize| -> f64 {
        if n < 0 {
            0.0
        } else {
            q.get(n as usize).map_or(0.5, |&v| f64::from(v))
        }
    };
    let s0 = |n: isize| -> f64 {
        let ramp = if n < span as isize {
            2.0 * PI * h * q_at(n)
        } else {
            PI * h - 2.0 * PI * h * q_at(n - span as isize)
        };
        ramp.sin() / (PI * h).sin()
    };
    let len = (memory + 1) * sps;
    let raw: Vec<f64> = (0..len as isize)
        .map(|n| {
            (0..memory)
                .map(|i| s0(n + (i * sps) as isize))
                .product::<f64>()
        })
        .collect();
    let energy: f64 = raw.iter().map(|v| v * v).sum();
    let scale = energy.sqrt().recip();
    Ok(raw.iter().map(|v| (v * scale) as f32).collect())
}

pub(super) fn whole_sps(params: &CpmParams) -> Result<usize, LaurentError> {
    if params.mapping().m() != 2 {
        return Err(LaurentError::NotBinary(params.mapping().m()));
    }
    if (params.h() - 0.5).abs() > H_TOLERANCE {
        return Err(LaurentError::NotHalfIndex(params.h()));
    }
    let sps = params.sps();
    let whole = sps.round();
    if (sps - whole).abs() > 1e-9 {
        return Err(LaurentError::FractionalSps(sps));
    }
    Ok(whole as usize)
}

fn linear_params(params: &CpmParams) -> Result<LinearParams, LaurentError> {
    let sps = whole_sps(params)?;
    let pulse = laurent_main_pulse(params)?;
    let table = tables::pam(2).map_err(|_| LaurentError::NotBinary(2))?;
    LinearParams::new(table, pulse, sps)
        .and_then(|p| p.with_rotation(QUARTER_TURN))
        .map_err(|_| LaurentError::FractionalSps(params.sps()))
}

pub struct CoherentCpmDemod {
    receiver: LinearBurstDemod,
    polarity: f32,
    symbols: Vec<Complex<f32>>,
}

impl CoherentCpmDemod {
    pub fn new(params: &CpmParams, loop_bw: f64) -> Result<Self, LaurentError> {
        let linear = linear_params(params)?;
        let receiver = LinearBurstDemod::new(
            &linear,
            linear.pulse(),
            POWER_SYMBOLS,
            Some(carrier(loop_bw)),
        )
        .with_timing_metric(TimingMetric::RotatedPower {
            rotation_rad: QUARTER_TURN,
            order: 2,
        });
        Ok(Self {
            receiver,
            polarity: polarity(params),
            symbols: Vec::new(),
        })
    }

    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<f32>) {
        self.symbols.clear();
        let _ = self.receiver.process(iq, &mut self.symbols);
        out.extend(
            self.symbols
                .windows(2)
                .map(|pair| self.polarity * pair[1].re * pair[0].re),
        );
    }

    #[must_use]
    pub fn carrier_freq_cycles_per_symbol(&self) -> f64 {
        self.receiver.carrier_freq_cycles_per_symbol()
    }
}

pub(super) fn polarity(params: &CpmParams) -> f32 {
    let levels = params.mapping().levels();
    if levels[1] > levels[0] { 1.0 } else { -1.0 }
}

fn carrier(loop_bw: f64) -> CarrierLoop {
    CarrierLoop::new(PhaseDetector::MthPower { m: 2 }, loop_bw)
}

#[cfg(test)]
mod tests {
    use sdrmm_modem_test_support::ber::rng::Rng;

    use super::*;
    use crate::{
        cpm::{CpmMod, Mapping},
        pulse::{self, Norm},
    };

    const SPS: f64 = 8.0;

    fn gmsk(bt: f64) -> CpmParams {
        CpmParams::from_h(
            Mapping::natural(2),
            0.5,
            pulse::gaussian_freq(SPS, bt, 4, Norm::Area),
            SPS,
        )
    }

    fn msk() -> CpmParams {
        CpmParams::from_h(Mapping::natural(2), 0.5, pulse::rect(SPS, Norm::Area), SPS)
    }

    fn symbols(n: usize, seed: u64) -> Vec<u8> {
        let mut rng = Rng::new(seed);
        (0..n).map(|_| (rng.next_u64() & 1) as u8).collect()
    }

    fn wave(params: &CpmParams, sent: &[u8]) -> Vec<Complex<f32>> {
        let mut m = CpmMod::new(params.clone());
        let mut out = Vec::new();
        m.modulate(sent, &mut out);
        m.flush(&mut out);
        out
    }

    fn best_alignment(soft: &[f32], sent: &[u8]) -> (usize, isize) {
        (-8isize..8)
            .map(|lag| {
                let errors = (64..sent.len() - 64)
                    .filter(|&i| {
                        soft.get((i as isize + lag) as usize)
                            .is_none_or(|&s| (s > 0.0) != (sent[i] == 1))
                    })
                    .count();
                (errors, lag)
            })
            .min()
            .unwrap_or((usize::MAX, 0))
    }

    #[test]
    fn the_msk_main_pulse_is_a_half_sine_over_two_symbols() {
        let c0 = laurent_main_pulse(&msk()).unwrap();
        assert_eq!(c0.len(), 2 * SPS as usize);
        let peak = c0.iter().copied().fold(0.0f32, f32::max);
        let middle = c0[SPS as usize - 1].max(c0[SPS as usize]);
        assert!((middle - peak).abs() < 1e-6, "peak {peak} sits off centre");
        let energy: f32 = c0.iter().map(|v| v * v).sum();
        assert!((energy - 1.0).abs() < 1e-5);
    }

    #[test]
    fn only_binary_half_index_cpm_is_accepted() {
        let four = CpmParams::from_h(Mapping::natural(4), 0.5, pulse::rect(SPS, Norm::Area), SPS);
        assert_eq!(laurent_main_pulse(&four), Err(LaurentError::NotBinary(4)));
        let wide = CpmParams::from_h(Mapping::natural(2), 1.0, pulse::rect(SPS, Norm::Area), SPS);
        assert_eq!(
            laurent_main_pulse(&wide),
            Err(LaurentError::NotHalfIndex(1.0))
        );
        let fractional =
            CpmParams::from_h(Mapping::natural(2), 0.5, pulse::rect(8.5, Norm::Area), 8.5);
        assert_eq!(
            laurent_main_pulse(&fractional),
            Err(LaurentError::FractionalSps(8.5))
        );
    }

    #[test]
    fn a_noiseless_burst_decodes_clean() {
        for (name, params) in [
            ("msk", msk()),
            ("gmsk 0.5", gmsk(0.5)),
            ("gmsk 0.3", gmsk(0.3)),
        ] {
            let sent = symbols(2_000, 0x1a0);
            let mut demod = CoherentCpmDemod::new(&params, 0.005).unwrap();
            let mut soft = Vec::new();
            demod.process(&wave(&params, &sent), &mut soft);
            let (errors, _) = best_alignment(&soft, &sent);
            assert_eq!(errors, 0, "{name}");
        }
    }

    #[test]
    fn a_carrier_offset_is_acquired_on_a_burst() {
        let params = gmsk(0.3);
        let sent = symbols(3_000, 0x2b1);
        let mut w = wave(&params, &sent);
        for (k, s) in w.iter_mut().enumerate() {
            let theta = std::f64::consts::TAU * 0.004 * k as f64;
            *s *= Complex::new(theta.cos() as f32, theta.sin() as f32);
        }
        let mut demod = CoherentCpmDemod::new(&params, 0.005).unwrap();
        let mut soft = Vec::new();
        demod.process(&w, &mut soft);
        let (errors, _) = best_alignment(&soft, &sent);
        assert_eq!(errors, 0);
        let read = demod.carrier_freq_cycles_per_symbol() / SPS;
        assert!((read - 0.004).abs() < 1e-4, "read {read} cycles/sample");
    }
}
