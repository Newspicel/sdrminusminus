use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass};
use sdrmm_modem::{
    cpm::{CpmDemod, CpmParams, KnownSymbols, Mapping, MlseDetector, TIMING_BW_BURST},
    pulse::{self, Norm},
    soft::SoftBit,
};

use super::{
    Measurement,
    framing::{
        self, Acquisition, FRONT_TAPS, NOISE_BW_HZ, PREAMBLE, RATE, SPS, STEADY_BITS, TAIL, UW24,
        cpm_wave, find_uw, framed_symbols, steady_link, steady_soft, uw_levels,
    },
};
use crate::ber::{
    impair::{BurstModel, ChannelSpec},
    sweep::Link,
};

#[must_use]
pub fn span(bt: f64) -> usize {
    if bt < 0.4 { 4 } else { 3 }
}

#[must_use]
pub fn params(bt: f64) -> CpmParams {
    CpmParams::from_h(
        Mapping::natural(2),
        0.5,
        pulse::gaussian_freq(SPS, bt, span(bt), Norm::Area),
        SPS,
    )
}

#[must_use]
pub fn rx(bt: f64) -> Vec<f32> {
    if bt < 0.4 {
        pulse::gaussian(SPS, 0.5, 3, Norm::Area)
    } else {
        pulse::gaussian_freq(SPS, bt, span(bt), Norm::Area)
    }
}

#[must_use]
pub fn mlse_rx(bt: f64) -> Vec<f32> {
    pulse::gaussian_freq(SPS, bt, span(bt), Norm::Area)
}

fn rx_name(bt: f64) -> &'static str {
    if bt < 0.4 {
        "gaussian-BT0.5 rx"
    } else {
        "pulse-matched rx"
    }
}

#[must_use]
pub fn link(bt: f64) -> Link {
    framed_link(bt, Acquisition::DataLike)
}

#[must_use]
pub fn alternating_link(bt: f64) -> Link {
    framed_link(bt, Acquisition::Alternating)
}

fn framed_link(bt: f64, acquisition: Acquisition) -> Link {
    let filler = match acquisition {
        Acquisition::DataLike => "data-like",
        Acquisition::Alternating => "alternating",
    };
    steady_link(
        &format!(
            "gmsk BT={bt} h=0.5 uncoded, CpmMod -> +/-6 kHz front lowpass -> CpmDemod \
             ({}, timing bw 0.015), 48 kHz 4800 baud, {filler} 96+24+24 symbol overhead \
             in Eb, release",
            rx_name(bt)
        ),
        acquisition,
        params(bt),
        rx(bt),
    )
}

#[must_use]
pub fn bt03_link() -> Link {
    link(0.3)
}

#[must_use]
pub fn bt05_link() -> Link {
    link(0.5)
}

fn find_uw_soft(bits: &[SoftBit], lo: usize, hi: usize, uw: &[u8]) -> Option<usize> {
    let last = hi.min(bits.len().checked_sub(uw.len())?);
    let misfit = |at: usize| -> f32 {
        uw.iter()
            .enumerate()
            .map(|(i, &s)| {
                let want = if s == 1 { 1.0 } else { -1.0 };
                let got = bits[at + i].0;
                (got - want) * (got - want)
            })
            .sum()
    };
    (lo..=last).min_by(|&a, &b| misfit(a).total_cmp(&misfit(b)))
}

#[must_use]
pub fn mlse_link(bt: f64) -> Link {
    let p = params(bt);
    let filter = mlse_rx(bt);
    let mod_params = p.clone();
    Link {
        label: format!(
            "gmsk BT={bt} h=0.5 uncoded, MLSE tier: CpmMod -> +/-6 kHz front lowpass -> \
             CpmDemod (pulse-matched rx, timing bw 0.015) -> MlseDetector over the pulse's own \
             symbol-spaced response, 48 kHz 4800 baud, data-like 96+24+24 symbol overhead in \
             Eb, release"
        ),
        bits_per_trial: STEADY_BITS,
        modulate: Box::new(move |bits| {
            cpm_wave(
                &mod_params,
                &framed_symbols(Acquisition::DataLike, PREAMBLE, &UW24, bits, TAIL),
            )
        }),
        demodulate: Box::new(move |wave| {
            let soft = steady_soft(&p, &filter, wave);
            let mut detector = MlseDetector::new(&p, &filter);
            let (mut decided, mut bits) = (Vec::new(), Vec::new());
            detector.process(&soft, &mut decided, &mut bits);
            detector.flush(&mut decided, &mut bits);
            let Some(at) = find_uw_soft(&bits, PREAMBLE, PREAMBLE + 48, &UW24) else {
                return Vec::new();
            };
            (0..STEADY_BITS)
                .map(|k| decided.get(at + UW24.len() + k) == Some(&1))
                .collect()
        }),
    }
}

#[must_use]
pub fn bt03_mlse_link() -> Link {
    mlse_link(0.3)
}

#[must_use]
pub fn bt05_mlse_link() -> Link {
    mlse_link(0.5)
}

const BURST_LEAD_SAMPLES: usize = 12_000;

pub const BURST_FRAMES: usize = 6;

#[derive(Clone, Copy)]
pub struct BurstRecipe {
    pub payload_symbols: usize,
    pub off_symbols: usize,
    pub payload_frames: usize,
    pub level_step_db: f64,
}

impl BurstRecipe {
    #[must_use]
    pub fn reference(payload_frames: usize) -> Self {
        Self {
            payload_symbols: 128,
            off_symbols: 104,
            payload_frames,
            level_step_db: 0.0,
        }
    }

    fn content(&self) -> usize {
        UW24.len() + self.payload_symbols
    }

    fn frame_symbols(&self) -> usize {
        self.content() + self.off_symbols
    }

    fn on_samples(&self) -> usize {
        self.content() * SPS as usize + params(0.5).freq_pulse().len()
    }

    fn lead_frames(&self) -> usize {
        BURST_LEAD_SAMPLES.div_ceil(self.frame_symbols() * SPS as usize)
    }

    fn bits(&self) -> usize {
        self.payload_symbols * self.payload_frames
    }

    fn symbols(&self, payload: &[bool]) -> Vec<u8> {
        let frame = self.frame_symbols();
        let mut symbols = framing::data_like_symbols(
            frame * self.payload_frames + self.content(),
            framing::DATA_LIKE_SEED,
        );
        for p in 0..self.payload_frames {
            let base = frame * (p + 1);
            symbols[base..base + UW24.len()].copy_from_slice(&UW24);
            for k in 0..self.payload_symbols {
                symbols[base + UW24.len() + k] = u8::from(payload[p * self.payload_symbols + k]);
            }
        }
        symbols
    }

    #[must_use]
    pub fn channel(&self) -> ChannelSpec {
        let frame_samples = self.frame_symbols() * SPS as usize;
        ChannelSpec::default().burst(BurstModel::new(
            self.on_samples(),
            frame_samples - self.on_samples(),
            SPS as usize,
            self.level_step_db,
            26.0,
        ))
    }

    #[must_use]
    pub fn link(&self, label: &str) -> Link {
        let recipe = *self;
        let demod_recipe = *self;
        Link {
            label: label.to_string(),
            bits_per_trial: self.bits(),
            modulate: Box::new(move |bits| {
                let mut wave = vec![
                    Complex::default();
                    recipe.lead_frames() * recipe.frame_symbols() * SPS as usize
                ];
                wave.extend(cpm_wave(&params(0.5), &recipe.symbols(bits)));
                wave
            }),
            demodulate: Box::new(move |wave| demod_recipe.demodulate(wave)),
        }
    }

    fn demodulate(&self, wave: &[Complex<f32>]) -> Vec<bool> {
        let p = params(0.5);
        let front = design_lowpass(FRONT_TAPS, NOISE_BW_HZ / RATE);
        let mut filter = Decimator::new(&front, 1);
        let mut demod = CpmDemod::new(&p, &rx(0.5), TIMING_BW_BURST);
        let mut filtered = Vec::new();
        filter.process(wave, &mut filtered);
        let mut soft = Vec::new();
        demod.process(&filtered, &mut soft);
        let levels = uw_levels(&p, &UW24);

        let frame = self.frame_symbols();
        let lead = self.lead_frames() * frame;
        let mut hook = KnownSymbols::new(&p, (4 * frame) as u32);
        let mut bits = Vec::with_capacity(self.bits());
        let mut delay: usize = 0;
        for k in 0..self.payload_frames {
            let expect = lead + frame * (k + 1);
            let (lo, hi) = if k == 0 {
                (expect, expect + 48)
            } else {
                ((expect + delay).saturating_sub(4), expect + delay + 4)
            };
            let at = find_uw(&soft, lo, hi, &levels);
            if let Some(at) = at {
                delay = at.saturating_sub(expect);
                if at + UW24.len() <= soft.len() {
                    hook.anchor(&UW24, &soft[at..at + UW24.len()]);
                }
            }
            for i in 0..self.payload_symbols {
                hook.tick();
                let bit = at
                    .and_then(|at| soft.get(at + UW24.len() + i))
                    .is_some_and(|&s| p.mapping().slice(hook.correct(s)) == 1);
                bits.push(bit);
            }
        }
        bits
    }
}

pub const BT03_GRID: &[f64] = &[
    14.0, 15.0, 16.0, 17.0, 18.0, 19.0, 20.0, 21.0, 22.0, 23.0, 24.0, 25.0, 26.0,
];
pub const BT05_GRID: &[f64] = &[8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0];

pub const BT03_MLSE_GRID: &[f64] = &[
    8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0,
];
pub const BT05_MLSE_GRID: &[f64] = &[9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0];

pub const BT03_SEED: u64 = 0x63a3;
pub const BT05_SEED: u64 = 0x63a5;
pub const BT03_MLSE_SEED: u64 = 0x63a3_11e5;
pub const BT05_MLSE_SEED: u64 = 0x63a5_11e5;

pub const BT03_AWGN: &str = "cpm/gmsk_bt03_datalike_awgn";
pub const BT05_AWGN: &str = "cpm/gmsk_bt05_datalike_awgn";
pub const BT03_MLSE_AWGN: &str = "cpm/gmsk_bt03_mlse_awgn";
pub const BT05_MLSE_AWGN: &str = "cpm/gmsk_bt05_mlse_awgn";

pub const BT03_AWGN_ALTERNATING: &str = "cpm/gmsk_bt03_awgn";
pub const BT05_AWGN_ALTERNATING: &str = "cpm/gmsk_bt05_awgn";

pub const LIMITS: &str = "cpm/gmsk_datalike_limits";
pub const PERF: &str = "cpm/gmsk_perf";
pub const MLSE_PERF: &str = "cpm/gmsk_mlse_perf";
pub const MLSE_LIMITS: &str = "cpm/gmsk_mlse_limits";
pub const LIMITS_ALTERNATING: &str = "cpm/gmsk_limits";

pub const MEASUREMENTS: &[Measurement] = &[
    Measurement::committed(
        BT03_AWGN,
        bt03_link,
        BT03_GRID,
        BT03_SEED,
        framing::FULL_CAP,
    ),
    Measurement::committed(
        BT05_AWGN,
        bt05_link,
        BT05_GRID,
        BT05_SEED,
        framing::FULL_CAP,
    ),
    Measurement::committed(
        BT03_MLSE_AWGN,
        bt03_mlse_link,
        BT03_MLSE_GRID,
        BT03_MLSE_SEED,
        framing::FULL_CAP,
    ),
    Measurement::committed(
        BT05_MLSE_AWGN,
        bt05_mlse_link,
        BT05_MLSE_GRID,
        BT05_MLSE_SEED,
        framing::FULL_CAP,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_tiers_transmit_the_same_waveform() {
        let bits: Vec<bool> = (0..STEADY_BITS).map(|i| i % 3 == 0).collect();
        for bt in [0.3, 0.5] {
            assert_eq!(
                (link(bt).modulate)(&bits),
                (mlse_link(bt).modulate)(&bits),
                "BT={bt}: the tiers no longer share a transmitter"
            );
        }
    }
}
