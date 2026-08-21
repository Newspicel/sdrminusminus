use num_complex::Complex;
use sdrmm_dsp::{
    Decimator, FmDemod, Nco, RealDecimator, SymbolSync, ToneCorrelator, design_lowpass,
    one_pole_coeff,
};

use super::params::CpmParams;

pub const TIMING_BW_BURST: f64 = 0.015;

pub const TIMING_BW_CONTINUOUS: f64 = 0.003;

const CENTRE_SYMBOLS_FLOOR: f32 = 150.0;

const CENTRE_POWER_SYMBOLS: f32 = 30.0;

const PEAK_SYMBOLS: f32 = 60.0;

const PEAK_HOLD_SYMBOLS: f32 = 4.0 * PEAK_SYMBOLS;

const PEAK_ATTACK: f32 = 0.125;

const ENVELOPE_TAU_SYMBOLS: f64 = 0.5;

const FLOOR_TAU_SYMBOLS: f64 = 96.0;

const FLOOR_SETTLE: f64 = 4.0;

const CARRIER_RISE: f32 = 4.0;

/// How much the received power may vary, as a fraction of its own mean squared, and still be
/// read as a carrier rather than as noise.
///
/// Power alone cannot tell a carrier that has never stopped from the noise it is measured
/// against, because the floor it would be compared to was measured on the carrier itself. How
/// steady the power is can: noise out of a receiver is Rayleigh, so its power varies by about
/// its own mean, while a constant-envelope transmission barely varies at all. Measured through
/// this chain a live control channel sits near 0.001 and noise near 0.99, so the bar is set well
/// clear of both. Real-valued input never reaches it - a tone's power still swings by half its
/// mean - which leaves those front ends judged on power alone, as before.
const STEADY_SPREAD: f32 = 0.25;

const IMAGE_TAPS: usize = 127;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RealDetector {
    Discriminator { centre_hz: f64 },
    ToneFilterbank { plus_hz: f64, minus_hz: f64 },
}

enum FrontEnd {
    Quadrature(FmDemod),
    Analytic {
        mixer: Nco,
        image: Decimator,
        demod: FmDemod,
        mixed: Vec<Complex<f32>>,
        baseband: Vec<Complex<f32>>,
    },
    Filterbank {
        plus: ToneCorrelator,
        minus: ToneCorrelator,
    },
}

pub struct CpmDemod {
    front: FrontEnd,
    matched: RealDecimator,
    sync: SymbolSync,
    level_max: f32,
    centre: f32,
    centre_scale: f64,
    peak: f32,
    idle_peak: f32,
    centre_alpha: f32,
    peak_decay: f32,
    peak_hold_decay: f32,
    outer_region: f32,
    envelope: f32,
    floor: f32,
    steady_level: f32,
    mean: f32,
    mean_square: f32,
    envelope_alpha: f32,
    floor_alpha: f32,
    settling: usize,
    settle_samples: usize,
    keyed: usize,
    support: usize,
    demod_buf: Vec<f32>,
    filtered: Vec<f32>,
    carrier_run: Vec<bool>,
    settled_run: Vec<bool>,
    centred: Vec<Complex<f32>>,
    retimed: Vec<Complex<f32>>,
    retimed_carrier: Vec<bool>,
    retimed_settled: Vec<bool>,
}

impl CpmDemod {
    #[must_use]
    pub fn new(params: &CpmParams, receive_filter: &[f32], timing_bw: f64) -> Self {
        let front = FrontEnd::Quadrature(FmDemod::new(params.sps(), params.h() / 2.0));
        Self::build(params, receive_filter, timing_bw, front, 0)
    }

    #[must_use]
    pub fn real(
        params: &CpmParams,
        receive_filter: &[f32],
        timing_bw: f64,
        sample_rate: f64,
        detector: RealDetector,
    ) -> Self {
        assert!(
            sample_rate.is_finite() && sample_rate > 0.0,
            "sample rate must be positive"
        );
        let baud = sample_rate / params.sps();
        let (front, extra) = match detector {
            RealDetector::Discriminator { centre_hz } => {
                assert!(
                    centre_hz > 0.0 && centre_hz < sample_rate / 2.0,
                    "subcarrier centre must lie inside the Nyquist band"
                );
                let taps = design_lowpass(IMAGE_TAPS, centre_hz / sample_rate);
                (
                    FrontEnd::Analytic {
                        mixer: Nco::new(-centre_hz as f32, sample_rate as f32),
                        image: Decimator::new(&taps, 1),
                        demod: FmDemod::new(sample_rate, params.h() * baud / 2.0),
                        mixed: Vec::new(),
                        baseband: Vec::new(),
                    },
                    IMAGE_TAPS,
                )
            }
            RealDetector::ToneFilterbank { plus_hz, minus_hz } => {
                assert_eq!(
                    params.mapping().m(),
                    2,
                    "a two-tone filterbank detects two levels"
                );
                assert!(plus_hz != minus_hz, "filterbank tones must be distinct");
                let window = (sample_rate / (plus_hz - minus_hz).abs()).round() as usize;
                (
                    FrontEnd::Filterbank {
                        plus: ToneCorrelator::new(sample_rate, plus_hz, window),
                        minus: ToneCorrelator::new(sample_rate, minus_hz, window),
                    },
                    window,
                )
            }
        };
        Self::build(params, receive_filter, timing_bw, front, extra)
    }

    fn build(
        params: &CpmParams,
        receive_filter: &[f32],
        timing_bw: f64,
        front: FrontEnd,
        front_support: usize,
    ) -> Self {
        assert!(!receive_filter.is_empty(), "receive filter must have taps");
        let area: f64 = receive_filter.iter().map(|&t| f64::from(t)).sum();
        assert!(
            (area - 1.0).abs() < 1e-3,
            "receive filter must be unit-area (pulse::Norm::Area), got Σ = {area}"
        );
        let sps = params.sps();
        let level_max = params.mapping().max_level();
        let settle = (FLOOR_SETTLE * FLOOR_TAU_SYMBOLS * sps) as usize;
        let mean_sq = params
            .mapping()
            .levels()
            .iter()
            .map(|&l| l * l)
            .sum::<f32>()
            / params.mapping().m() as f32;
        let half_spacing = params.mapping().min_spacing() / 2.0;
        let centre_symbols = (CENTRE_POWER_SYMBOLS * mean_sq / (half_spacing * half_spacing))
            .max(CENTRE_SYMBOLS_FLOOR);
        Self {
            front,
            matched: RealDecimator::new(receive_filter, 1),
            sync: SymbolSync::new(sps, timing_bw),
            level_max,
            centre: 0.0,
            centre_scale: params.h() / (2.0 * sps),
            peak: level_max,
            idle_peak: level_max,
            centre_alpha: 1.0 / (centre_symbols * sps as f32),
            peak_decay: 1.0 - 1.0 / PEAK_SYMBOLS,
            peak_hold_decay: 1.0 - 1.0 / PEAK_HOLD_SYMBOLS,
            outer_region: (level_max - params.mapping().min_spacing() / 2.0).max(0.0) / level_max,
            envelope: 0.0,
            floor: 0.0,
            steady_level: 0.0,
            mean: 0.0,
            mean_square: 0.0,
            envelope_alpha: one_pole_coeff(sps, ENVELOPE_TAU_SYMBOLS),
            floor_alpha: one_pole_coeff(sps, FLOOR_TAU_SYMBOLS),
            settling: settle,
            settle_samples: settle,
            keyed: 0,
            support: receive_filter.len() + front_support,
            demod_buf: Vec::new(),
            filtered: Vec::new(),
            carrier_run: Vec::new(),
            settled_run: Vec::new(),
            centred: Vec::new(),
            retimed: Vec::new(),
            retimed_carrier: Vec::new(),
            retimed_settled: Vec::new(),
        }
    }

    #[must_use]
    pub fn settled(&self) -> &[bool] {
        &self.retimed_settled
    }

    #[must_use]
    pub fn frequency_error_cycles_per_sample(&self) -> f64 {
        f64::from(self.centre) * self.centre_scale
    }

    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<f32>) {
        self.carrier_run.clear();
        self.settled_run.clear();
        for sample in iq {
            self.gate_sample(sample.norm_sqr());
        }
        let FrontEnd::Quadrature(demod) = &mut self.front else {
            panic!("constructed for real input; call process_real");
        };
        demod.process(iq, &mut self.demod_buf);
        self.finish(out);
    }

    pub fn process_real(&mut self, audio: &[f32], out: &mut Vec<f32>) {
        self.carrier_run.clear();
        self.settled_run.clear();
        for &sample in audio {
            self.gate_sample(sample * sample);
        }
        match &mut self.front {
            FrontEnd::Quadrature(_) => panic!("constructed for IQ input; call process"),
            FrontEnd::Analytic {
                mixer,
                image,
                demod,
                mixed,
                baseband,
            } => {
                mixed.clear();
                mixed.extend(
                    audio
                        .iter()
                        .map(|&s| Complex::new(s, 0.0) * mixer.next_sample()),
                );
                image.process(mixed, baseband);
                demod.process(baseband, &mut self.demod_buf);
            }
            FrontEnd::Filterbank { plus, minus } => {
                self.demod_buf.clear();
                self.demod_buf
                    .extend(audio.iter().map(|&s| plus.push(s) - minus.push(s)));
            }
        }
        self.finish(out);
    }

    fn gate_sample(&mut self, power: f32) {
        self.envelope += self.envelope_alpha * (power - self.envelope);
        self.mean += self.floor_alpha * (power - self.mean);
        self.mean_square += self.floor_alpha * (power * power - self.mean_square);
        self.settling = self.settling.saturating_sub(1);
        let spread = self.mean_square - self.mean * self.mean;
        let steady = spread < STEADY_SPREAD * self.mean * self.mean;
        let keyed = self.settling == 0 && (steady || self.envelope > self.floor * CARRIER_RISE);
        if steady {
            self.steady_level = self.mean;
        }
        if self.steady_level > 0.0 && self.mean * CARRIER_RISE < self.steady_level {
            // The steady carrier the floor never got to see underneath has stopped, and what is
            // left is the noise it was hiding.
            self.floor = self.mean;
            self.steady_level = 0.0;
        } else if !keyed && !steady {
            self.floor += self.floor_alpha * (self.envelope - self.floor);
        }
        self.keyed = if keyed {
            (self.keyed + 1).min(self.support)
        } else {
            0
        };
        self.carrier_run.push(keyed);
        self.settled_run.push(self.keyed == self.support);
    }

    fn finish(&mut self, out: &mut Vec<f32>) {
        self.matched.process(&self.demod_buf, &mut self.filtered);
        self.centred.clear();
        for (&sample, &settled) in self.filtered.iter().zip(&self.settled_run) {
            if settled {
                self.centre += self.centre_alpha * (sample - self.centre);
            }
            self.centred.push(Complex::new(sample - self.centre, 0.0));
        }

        self.retimed.clear();
        self.retimed_carrier.clear();
        self.retimed_settled.clear();
        let mut start = 0;
        while start < self.centred.len() {
            let (carrier, settled) = (self.carrier_run[start], self.settled_run[start]);
            let mut end = start + 1;
            while end < self.centred.len()
                && self.carrier_run[end] == carrier
                && self.settled_run[end] == settled
            {
                end += 1;
            }
            let run = &self.centred[start..end];
            if settled {
                self.sync.process(run, &mut self.retimed);
            } else {
                self.sync.process_held(run, &mut self.retimed);
            }
            self.retimed_carrier.resize(self.retimed.len(), carrier);
            self.retimed_settled.resize(self.retimed.len(), settled);
            start = end;
        }

        let (mut peak, mut idle) = (self.peak, self.idle_peak);
        let carriers = self.retimed_carrier.iter().zip(&self.retimed_settled);
        for (symbol, (&carrier, &settled)) in self.retimed.iter().zip(carriers) {
            let value = symbol.re;
            let magnitude = value.abs();
            if carrier {
                if settled {
                    if magnitude > peak {
                        peak += PEAK_ATTACK * (magnitude - peak);
                    } else if magnitude > peak * self.outer_region {
                        peak += (magnitude - peak) / PEAK_SYMBOLS;
                    } else {
                        peak *= self.peak_hold_decay;
                    }
                }
            } else {
                if magnitude > idle {
                    idle += PEAK_ATTACK * (magnitude - idle);
                } else {
                    idle *= self.peak_decay;
                }
            }
            let level = if carrier { &peak } else { &idle };
            let unit = *level / self.level_max;
            out.push(if unit > 1e-6 { value / unit } else { 0.0 });
        }
        (self.peak, self.idle_peak) = (peak, idle);
    }

    pub fn reset(&mut self) {
        self.sync.reset();
        if let FrontEnd::Filterbank { plus, minus } = &mut self.front {
            plus.reset();
            minus.reset();
        }
        self.centre = 0.0;
        self.peak = self.level_max;
        self.idle_peak = self.level_max;
        self.envelope = 0.0;
        self.floor = 0.0;
        self.steady_level = 0.0;
        self.mean = 0.0;
        self.mean_square = 0.0;
        self.settling = self.settle_samples;
        self.keyed = 0;
        self.demod_buf.clear();
        self.filtered.clear();
        self.carrier_run.clear();
        self.settled_run.clear();
        self.centred.clear();
        self.retimed.clear();
        self.retimed_carrier.clear();
        self.retimed_settled.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::{
        super::{levels::KnownSymbols, modulator::CpmMod, params::Mapping},
        *,
    };
    use crate::{
        ber::rng::Rng,
        pulse::{self, Norm},
    };

    const RATE: f64 = 48_000.0;
    const BAUD: f64 = 4_800.0;
    const SPS: f64 = 10.0;

    fn dibit_mapping() -> Mapping {
        Mapping::new(vec![1.0, 3.0, -1.0, -3.0])
    }

    fn four_level(deviation_hz: f64) -> CpmParams {
        CpmParams::from_deviation(
            dibit_mapping(),
            deviation_hz,
            BAUD,
            pulse::root_raised_cosine(SPS, 0.2, 8, Norm::Area),
            SPS,
        )
    }

    fn rx_rrc() -> Vec<f32> {
        pulse::root_raised_cosine(SPS, 0.2, 8, Norm::Area)
    }

    fn symbols(len: usize, seed: u32, m: u8) -> Vec<u8> {
        let mut state = seed | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state as u8) & (m - 1)
            })
            .collect()
    }

    fn transmit(params: &CpmParams, syms: &[u8]) -> Vec<Complex<f32>> {
        let mut m = CpmMod::new(params.clone());
        let mut out = Vec::new();
        m.modulate(syms, &mut out);
        m.flush(&mut out);
        out
    }

    const NOISE: f32 = 0.01;

    fn noise(seed: u64, len: usize) -> Vec<Complex<f32>> {
        let mut rng = Rng::new(seed);
        (0..len)
            .map(|_| {
                let re = (rng.uniform() as f32 * 2.0 - 1.0) * NOISE;
                let im = (rng.uniform() as f32 * 2.0 - 1.0) * NOISE;
                Complex::new(re, im)
            })
            .collect()
    }

    fn real_noise(seed: u64, len: usize) -> Vec<f32> {
        let mut rng = Rng::new(seed);
        (0..len)
            .map(|_| (rng.uniform() as f32 * 2.0 - 1.0) * NOISE)
            .collect()
    }

    fn listening(demod: &mut CpmDemod, seed: u64) {
        let len = demod.settle_samples + 4 * SPS as usize * 100;
        let quiet = noise(seed, len);
        let mut discard = Vec::new();
        demod.process(&quiet, &mut discard);
    }

    /// A trunked control channel is transmitting before the receiver is switched on and never
    /// stops, so there is no quiet stretch to measure a noise floor against and no gap to recover
    /// in. Judging that on power alone reads the carrier as its own floor and the gate never
    /// opens again.
    #[test]
    fn a_carrier_that_never_stopped_keys_the_gate_from_a_cold_start() {
        let params = four_level(1_944.0);
        let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_BURST);
        let iq = transmit(&params, &symbols(4_000, 0x5eed, 4));
        let mut out = Vec::new();

        demod.process(&iq, &mut out);

        let opened = demod
            .settled()
            .iter()
            .position(|&settled| settled)
            .expect("the gate never opened on a carrier that was already running");
        assert!(
            opened < 600,
            "the gate took {opened} symbols to open on a live carrier"
        );
    }

    #[test]
    fn noise_alone_never_keys_the_gate() {
        let params = four_level(1_944.0);
        let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_BURST);
        let mut out = Vec::new();

        demod.process(&noise(0x1157, 96_000), &mut out);

        assert!(
            demod.settled().iter().all(|&settled| !settled),
            "noise on its own was taken for a carrier"
        );
    }

    #[test]
    fn a_carrier_that_stops_shuts_the_gate_and_reopens_it_when_it_returns() {
        let params = four_level(1_944.0);
        let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_BURST);
        let iq = transmit(&params, &symbols(4_000, 0x5eed, 4));
        let mut out = Vec::new();

        demod.process(&iq, &mut out);
        assert!(demod.settled().iter().any(|&settled| settled));

        demod.process(&noise(0x2468, 96_000), &mut out);
        demod.process(&noise(0x1359, 96_000), &mut out);
        assert!(
            demod.settled().iter().all(|&settled| !settled),
            "the gate stayed open after the carrier went away"
        );

        demod.process(&iq, &mut out);
        assert!(
            demod.settled().iter().any(|&settled| settled),
            "the gate never reopened when the carrier came back"
        );
    }

    fn symbol_errors(got: &[u8], sent: &[u8], skip: usize) -> (usize, usize) {
        let (delay, errors) = (0..48)
            .map(|delay| {
                let errors = got
                    .iter()
                    .enumerate()
                    .skip(skip)
                    .filter(|&(i, s)| sent.get(i.wrapping_sub(delay)).is_none_or(|w| w != s))
                    .count();
                (delay, errors)
            })
            .min_by_key(|&(_, errors)| errors)
            .unwrap();
        assert!((1..40).contains(&delay), "implausible alignment {delay}");
        (errors, got.len() - skip)
    }

    #[test]
    fn the_centre_tracker_reads_back_the_carrier_offset_it_was_given() {
        let sent = symbols(20_000, 23, 4);
        let params = four_level(1_944.0);
        let clean = transmit(&params, &sent);
        let measure = |offset_hz: f64| {
            let shifted: Vec<Complex<f32>> = clean
                .iter()
                .enumerate()
                .map(|(i, &x)| {
                    let phase = TAU * offset_hz * i as f64 / RATE;
                    x * Complex::new(phase.cos() as f32, phase.sin() as f32)
                })
                .collect();
            let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_BURST);
            listening(&mut demod, 0x2211);
            let mut soft = Vec::new();
            demod.process(&shifted, &mut soft);
            demod.frequency_error_cycles_per_sample() * RATE
        };

        let base = measure(0.0);
        for offset_hz in [-600.0f64, -200.0, 200.0, 600.0] {
            let moved = measure(offset_hz) - base;
            assert!(
                (moved - offset_hz).abs() < 20.0,
                "a {offset_hz} Hz shift moved the tracker by {moved} Hz"
            );
        }
    }

    #[test]
    fn recovers_four_level_symbols_at_an_unexpected_deviation() {
        for deviation in [1_944.0, 1_400.0, 2_600.0] {
            let sent = symbols(400, 17, 4);
            let iq = transmit(&four_level(deviation), &sent);
            let nominal = four_level(1_944.0);
            let mut demod = CpmDemod::new(&nominal, &rx_rrc(), TIMING_BW_BURST);
            listening(&mut demod, 0x1157);
            let mut soft = Vec::new();
            demod.process(&iq, &mut soft);
            let got: Vec<u8> = soft.iter().map(|&s| nominal.mapping().slice(s)).collect();
            let (errors, total) = symbol_errors(&got, &sent, 100);
            assert!(total > 280, "only {total} symbols at {deviation} Hz");
            assert_eq!(errors, 0, "symbol errors at {deviation} Hz deviation");
        }
    }

    #[test]
    fn tracks_a_carrier_offset() {
        let sent = symbols(900, 5, 4);
        let params = four_level(1_944.0);
        let mut iq = transmit(&params, &sent);
        for (k, s) in iq.iter_mut().enumerate() {
            *s *= Complex::from_polar(1.0, (TAU * 400.0 * k as f64 / RATE) as f32);
        }
        let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_BURST);
        listening(&mut demod, 0x1157);
        let mut soft = Vec::new();
        demod.process(&iq, &mut soft);
        let got: Vec<u8> = soft.iter().map(|&s| params.mapping().slice(s)).collect();
        let (errors, _) = symbol_errors(&got, &sent, soft.len() - 200);
        assert_eq!(errors, 0);
    }

    #[test]
    fn block_splits_do_not_change_the_symbols() {
        let sent = symbols(300, 41, 4);
        let params = four_level(1_944.0);
        let iq = transmit(&params, &sent);
        let mut whole = Vec::new();
        let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_BURST);
        listening(&mut demod, 0x1157);
        demod.process(&iq, &mut whole);

        let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_BURST);
        listening(&mut demod, 0x1157);
        let mut ragged = Vec::new();
        let mut pos = 0;
        for len in [997usize, 1, 4_096, 65, 7].iter().cycle() {
            if pos >= iq.len() {
                break;
            }
            let end = (pos + len).min(iq.len());
            demod.process(&iq[pos..end], &mut ragged);
            pos = end;
        }
        assert_eq!(whole, ragged);
    }

    #[test]
    fn a_keyed_transmitter_does_not_lose_its_clock_in_the_dead_time() {
        const ON: usize = 132;
        const FRAME: usize = 288;
        let params = four_level(1_944.0);
        let sent = symbols(2_880, 23, 4);
        let keyed: Vec<Option<u8>> = sent
            .iter()
            .enumerate()
            .map(|(i, &s)| (i % FRAME < ON).then_some(s))
            .collect();
        let mut iq = CpmMod::new(params.clone()).keyed(&keyed);
        let floor = noise(0xbeef, iq.len());
        for (s, n) in iq.iter_mut().zip(floor) {
            *s += n;
        }
        let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_BURST);
        listening(&mut demod, 0x1157);
        let mut soft = Vec::new();
        demod.process(&iq, &mut soft);

        let ideal = iq.len() / SPS as usize;
        assert!(
            (soft.len() as i64 - ideal as i64).abs() <= 2,
            "recovered {} symbols, ideal {ideal}",
            soft.len()
        );

        let got: Vec<u8> = soft.iter().map(|&s| params.mapping().slice(s)).collect();
        let (delay, _) = (0..48)
            .map(|delay| {
                let errors = (300usize..400)
                    .filter(|&i| sent.get(i.wrapping_sub(delay)).is_none_or(|w| *w != got[i]))
                    .count();
                (delay, errors)
            })
            .min_by_key(|&(_, errors)| errors)
            .unwrap();
        let last = sent.len() - FRAME + delay;
        let bad: Vec<usize> = (last..last + ON)
            .filter(|&i| sent.get(i - delay).is_none_or(|w| *w != got[i]))
            .map(|i| i - last)
            .collect();
        assert!(
            bad.is_empty(),
            "symbol errors at {bad:?} in the last of {} bursts",
            sent.len() / FRAME
        );
    }

    #[test]
    fn a_continuous_stream_holds_lock_over_twenty_thousand_symbols() {
        let sent = symbols(20_000, 0x5eed, 4);
        let params = four_level(1_944.0);
        let iq = transmit(&params, &sent);
        let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_CONTINUOUS);
        listening(&mut demod, 0x1157);
        let mut soft = Vec::new();
        demod.process(&iq, &mut soft);
        let got: Vec<u8> = soft.iter().map(|&s| params.mapping().slice(s)).collect();
        let (errors, total) = symbol_errors(&got, &sent, 500);
        assert!(total > 19_000, "only {total} symbols recovered");
        assert!(
            errors <= total / 1_000,
            "{errors} symbol errors in {total}: the continuous floor is back"
        );
    }

    #[test]
    fn gmsk_loopback_is_clean() {
        let params = CpmParams::from_h(
            Mapping::natural(2),
            0.5,
            pulse::gaussian_freq(SPS, 0.5, 3, Norm::Area),
            SPS,
        );
        let sent = symbols(600, 71, 2);
        let iq = transmit(&params, &sent);
        let mut demod = CpmDemod::new(
            &params,
            &pulse::gaussian(SPS, 0.5, 3, Norm::Area),
            TIMING_BW_BURST,
        );
        listening(&mut demod, 0x1157);
        let mut soft = Vec::new();
        demod.process(&iq, &mut soft);
        let got: Vec<u8> = soft.iter().map(|&s| params.mapping().slice(s)).collect();
        let (errors, total) = symbol_errors(&got, &sent, 100);
        assert!(total > 400);
        assert_eq!(errors, 0, "GMSK symbol errors");
    }

    #[test]
    fn two_level_cpfsk_loopback_is_clean() {
        let params = CpmParams::from_h(Mapping::natural(2), 0.5, pulse::rect(SPS, Norm::Area), SPS);
        let sent = symbols(600, 29, 2);
        let iq = transmit(&params, &sent);
        let mut demod = CpmDemod::new(&params, &pulse::rect(SPS, Norm::Area), TIMING_BW_BURST);
        listening(&mut demod, 0x1157);
        let mut soft = Vec::new();
        demod.process(&iq, &mut soft);
        let got: Vec<u8> = soft.iter().map(|&s| params.mapping().slice(s)).collect();
        let (errors, total) = symbol_errors(&got, &sent, 100);
        assert!(total > 400);
        assert_eq!(errors, 0, "2FSK symbol errors");
    }

    #[test]
    fn eight_level_loopback_is_clean_on_the_known_symbol_hook() {
        const PATTERN: [u8; 16] = [7, 0, 5, 2, 6, 1, 4, 3, 0, 7, 3, 4, 1, 6, 2, 5];
        const PERIOD: usize = 128;
        const FRAMES: usize = 12;
        let params = CpmParams::from_h(Mapping::natural(8), 0.3, pulse::rect(SPS, Norm::Area), SPS);
        let mut sent = Vec::with_capacity(FRAMES * PERIOD);
        for frame in 0..FRAMES {
            sent.extend_from_slice(&PATTERN);
            sent.extend(symbols(PERIOD - PATTERN.len(), 0x0dd5 ^ frame as u32, 8));
        }
        let iq = transmit(&params, &sent);
        let mut demod = CpmDemod::new(&params, &pulse::rect(SPS, Norm::Area), TIMING_BW_BURST);
        listening(&mut demod, 0x1157);
        let mut soft = Vec::new();
        demod.process(&iq, &mut soft);

        let got: Vec<u8> = soft.iter().map(|&s| params.mapping().slice(s)).collect();
        let (delay, _) = (0..48usize)
            .map(|d| {
                let errs = (200..1_000usize)
                    .filter(|&i| {
                        sent.get(i.wrapping_sub(d))
                            .is_none_or(|w| got.get(i).is_none_or(|g| g != w))
                    })
                    .count();
                (d, errs)
            })
            .min_by_key(|&(_, e)| e)
            .unwrap();
        let mut hook = KnownSymbols::new(&params, (4 * PERIOD) as u32);
        let mut errors = Vec::new();
        for frame in 1..FRAMES {
            let base = frame * PERIOD + delay;
            hook.anchor(&PATTERN, &soft[base..base + PATTERN.len()]);
            for k in PATTERN.len()..PERIOD {
                hook.tick();
                let Some(&s) = soft.get(base + k) else {
                    continue;
                };
                if params.mapping().slice(hook.correct(s)) != sent[frame * PERIOD + k] {
                    errors.push(frame * PERIOD + k);
                }
            }
        }
        assert!(
            errors.is_empty(),
            "8-level symbol errors at {errors:?} through the hook"
        );
    }

    #[test]
    fn a_silent_channel_produces_finite_symbols() {
        let params = four_level(1_944.0);
        let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_BURST);
        let mut soft = Vec::new();
        demod.process(&vec![Complex::new(0.0, 0.0); 4_800], &mut soft);
        assert!(!soft.is_empty());
        assert!(soft.iter().all(|s| s.is_finite()), "non-finite symbol");
    }

    fn afsk_params() -> CpmParams {
        CpmParams::from_deviation(
            Mapping::new(vec![1.0, -1.0]),
            500.0,
            1_200.0,
            pulse::rect(40.0, Norm::Area),
            40.0,
        )
    }

    fn afsk_rx() -> Vec<f32> {
        pulse::rect(20.0, Norm::Area)
    }

    fn afsk_audio(sent: &[u8]) -> Vec<f32> {
        let baseband = transmit(&afsk_params(), sent);
        let mut carrier = Nco::new(1_700.0, RATE as f32);
        baseband
            .iter()
            .map(|&s| (s * carrier.next_sample()).re)
            .collect()
    }

    fn afsk_roundtrip(detector: RealDetector, receive_filter: &[f32], seed: u32) {
        let params = afsk_params();
        let sent = symbols(500, seed, 2);
        let audio = afsk_audio(&sent);
        let mut demod = CpmDemod::real(&params, receive_filter, TIMING_BW_BURST, RATE, detector);
        let quiet = real_noise(0x1157, demod.settle_samples + 19_200);
        let mut discard = Vec::new();
        demod.process_real(&quiet, &mut discard);
        let mut soft = Vec::new();
        demod.process_real(&audio, &mut soft);
        let got: Vec<u8> = soft.iter().map(|&s| params.mapping().slice(s)).collect();
        let (errors, total) = symbol_errors(&got, &sent, 80);
        assert!(total > 350, "only {total} symbols");
        assert_eq!(errors, 0, "AFSK bit errors with {detector:?}");
    }

    #[test]
    fn afsk_decodes_through_the_tone_filterbank() {
        afsk_roundtrip(
            RealDetector::ToneFilterbank {
                plus_hz: 2_200.0,
                minus_hz: 1_200.0,
            },
            &afsk_rx(),
            33,
        );
    }

    #[test]
    fn afsk_decodes_through_the_analytic_discriminator() {
        afsk_roundtrip(
            RealDetector::Discriminator { centre_hz: 1_700.0 },
            &pulse::rect(40.0, Norm::Area),
            34,
        );
    }

    #[test]
    fn real_block_splits_do_not_change_the_symbols() {
        let params = afsk_params();
        let sent = symbols(200, 9, 2);
        let audio = afsk_audio(&sent);
        let detector = RealDetector::ToneFilterbank {
            plus_hz: 2_200.0,
            minus_hz: 1_200.0,
        };
        let filter = afsk_rx();
        let mut whole = Vec::new();
        let mut demod = CpmDemod::real(&params, &filter, TIMING_BW_BURST, RATE, detector);
        demod.process_real(&audio, &mut whole);

        let mut demod = CpmDemod::real(&params, &filter, TIMING_BW_BURST, RATE, detector);
        let mut ragged = Vec::new();
        let mut pos = 0;
        for len in [997usize, 1, 4_096, 65, 7].iter().cycle() {
            if pos >= audio.len() {
                break;
            }
            let end = (pos + len).min(audio.len());
            demod.process_real(&audio[pos..end], &mut ragged);
            pos = end;
        }
        assert_eq!(whole, ragged);
    }

    #[test]
    fn a_silent_audio_channel_produces_finite_symbols() {
        let params = afsk_params();
        let mut demod = CpmDemod::real(
            &params,
            &pulse::rect(40.0, Norm::Area),
            TIMING_BW_BURST,
            RATE,
            RealDetector::Discriminator { centre_hz: 1_700.0 },
        );
        let mut soft = Vec::new();
        demod.process_real(&vec![0.0; 48_000], &mut soft);
        assert!(!soft.is_empty());
        assert!(soft.iter().all(|s| s.is_finite()), "non-finite symbol");
    }

    #[test]
    #[should_panic(expected = "call process_real")]
    fn feeding_iq_to_a_real_construction_is_a_caller_bug() {
        let mut demod = CpmDemod::real(
            &afsk_params(),
            &pulse::rect(40.0, Norm::Area),
            TIMING_BW_BURST,
            RATE,
            RealDetector::Discriminator { centre_hz: 1_700.0 },
        );
        demod.process(&[Complex::new(0.0, 0.0); 16], &mut Vec::new());
    }

    #[test]
    #[should_panic(expected = "call process")]
    fn feeding_audio_to_an_iq_construction_is_a_caller_bug() {
        let params = four_level(1_944.0);
        let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_BURST);
        demod.process_real(&[0.0; 16], &mut Vec::new());
    }

    #[test]
    #[should_panic(expected = "two levels")]
    fn a_filterbank_needs_a_two_level_mapping() {
        let params = four_level(1_944.0);
        let _ = CpmDemod::real(
            &params,
            &rx_rrc(),
            TIMING_BW_BURST,
            RATE,
            RealDetector::ToneFilterbank {
                plus_hz: 2_200.0,
                minus_hz: 1_200.0,
            },
        );
    }

    #[test]
    fn complex_process_steady_state_allocates_nothing() {
        let params = four_level(1_944.0);
        let iq = transmit(&params, &symbols(1_200, 0x5eed, 4));
        let mut demod = CpmDemod::new(&params, &rx_rrc(), TIMING_BW_BURST);
        let mut soft = Vec::with_capacity(iq.len());
        demod.process(&iq, &mut soft);
        soft.clear();
        demod.process(&iq, &mut soft);
        soft.clear();
        crate::ber::perf::assert_no_alloc("CpmDemod::process", || demod.process(&iq, &mut soft));
        assert!(!soft.is_empty(), "the measured call recovered no symbols");
    }

    #[test]
    fn real_process_steady_state_allocates_nothing() {
        let params = afsk_params();
        let audio = afsk_audio(&symbols(300, 0x0dd5, 2));
        let mut demod = CpmDemod::real(
            &params,
            &afsk_rx(),
            TIMING_BW_BURST,
            RATE,
            RealDetector::ToneFilterbank {
                plus_hz: 2_200.0,
                minus_hz: 1_200.0,
            },
        );
        let mut soft = Vec::with_capacity(audio.len());
        demod.process_real(&audio, &mut soft);
        soft.clear();
        demod.process_real(&audio, &mut soft);
        soft.clear();
        crate::ber::perf::assert_no_alloc("CpmDemod::process_real", || {
            demod.process_real(&audio, &mut soft);
        });
        assert!(!soft.is_empty(), "the measured call recovered no symbols");
    }
}
