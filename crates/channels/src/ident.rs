mod agreement;
mod catalog;
mod classify;
mod confirm;
pub(crate) mod detect;
mod features;
mod framing;

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass, flat_bandwidth_hz};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, DecoderFamily, IdentFeatures,
    IdentParams, IdentReport, IdentSignal, MAX_IDENT_BANDWIDTH_HZ, MIN_IDENT_BANDWIDTH_HZ,
    Modulation,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const INPUT_RATE_HZ: f64 = 240_000.0;

const CHANNEL_TAPS: usize = 63;

const MAX_WINDOW: usize = 262_144;

const MIN_WINDOW: usize = 4 * detect::DETECT_FFT;

const STEADY_POWER_VARIATION: f64 = 0.3;

const OCCUPANCY_BLOCK: usize = 256;

const KEYED_SPAN_DB: f64 = 10.0;

const HF_TOP_HZ: f64 = 30_000_000.0;

const HF_GAP_HZ: f64 = 500.0;

const GAP_HZ: f64 = 12_000.0;

const PROBE_BANDWIDTH_HZ: f64 = 30_000.0;

const PROBED_CONFIDENCE: f32 = 0.9;

const PROBE_ENVELOPE: f32 = 0.3;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "ident".to_owned(),
    name: "Signal identifier".to_owned(),
    summary: "Guesses what an unknown signal is".to_owned(),
    family: DecoderFamily::Utility,
    bandwidth_hz: MAX_IDENT_BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    decoder_kind: Some("ident".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct IdentChannel {
    params: IdentParams,
    frequency_hz: f64,
    window: Vec<Complex<f32>>,
    pending: usize,
    detector: detect::Detector,
    meter: features::Meter,
    tracker: agreement::Tracker,
    confirmer: confirm::Confirmer,
    artifact_hz: Option<f64>,
    block_power: Vec<f64>,
    last_heard: Option<bool>,
}

fn params(settings: &ChannelSettings) -> Result<&IdentParams, ChannelError> {
    match &settings.params {
        ChannelParams::Ident(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "ident channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_params(p: &IdentParams) -> Result<(), ChannelError> {
    let widest = flat_bandwidth_hz(INPUT_RATE_HZ).min(MAX_IDENT_BANDWIDTH_HZ);
    if !(p.bandwidth_hz.is_finite() && (MIN_IDENT_BANDWIDTH_HZ..=widest).contains(&p.bandwidth_hz))
    {
        return Err(ChannelError::InvalidSettings(format!(
            "ident bandwidth must be in [{MIN_IDENT_BANDWIDTH_HZ}, {widest}] Hz, got {}",
            p.bandwidth_hz
        )));
    }
    Ok(())
}

pub(crate) fn occupied_band(p: &IdentParams) -> (f64, f64) {
    let half = p.bandwidth_hz / 2.0;
    (-half, half)
}

pub(crate) fn channel_filter(p: &IdentParams) -> Result<ChannelFilter, ChannelError> {
    check_params(p)?;
    let (_, half) = occupied_band(p);
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, half / INPUT_RATE_HZ),
        1,
    )))
}

fn interval_samples(p: &IdentParams) -> usize {
    let wanted = (f64::from(p.interval_ms) / 1_000.0 * INPUT_RATE_HZ) as usize;
    wanted.max(MIN_WINDOW)
}

impl IdentChannel {
    fn restart(&mut self) {
        self.window.clear();
        self.pending = 0;
    }

    fn forget(&mut self) {
        self.restart();
        self.tracker.forget();
        self.confirmer.forget();
        self.last_heard = None;
    }

    fn worth_reporting(&self, report: &IdentReport) -> bool {
        !report.signals.is_empty() || self.last_heard != Some(false)
    }

    fn analyse(&mut self) -> IdentReport {
        let dominated = self.fills_the_span();
        let survey = self.detector.measure(
            &self.window,
            INPUT_RATE_HZ,
            &detect::Search {
                half_span_hz: self.params.bandwidth_hz / 2.0,
                threshold_db: self.params.threshold_db,
                gap_hz: self.gap_hz(),
                dominated,
                artifact_hz: self.artifact_hz,
            },
        );
        let signals = survey
            .bands
            .iter()
            .map(|band| self.describe(band))
            .collect();
        self.tracker.sweep();
        self.confirmer.sweep();
        IdentReport {
            snr_db: survey.peak_db - survey.floor_db,
            signals,
        }
    }

    fn describe(&mut self, band: &detect::Band) -> IdentSignal {
        let frequency_hz = self.frequency_hz + band.center_hz;
        let dial = (self.frequency_hz > 0.0).then_some(frequency_hz);
        let located = IdentSignal {
            modulation: Modulation::Unknown,
            frequency_hz,
            center_offset_hz: band.center_hz,
            bandwidth_hz: band.bandwidth_hz,
            snr_db: band.snr_db,
            ..IdentSignal::default()
        };
        let Some(zoom) = features::zoom(&self.window, INPUT_RATE_HZ, band) else {
            return located;
        };
        let waveform = self.meter.measure(&zoom, band);
        let mut verdict = self
            .tracker
            .settle(band, classify::classify(band, &waveform));
        let mut candidates = catalog::candidates(verdict.modulation, band, &waveform, dial);
        framing::confirm(&mut candidates, &self.window, INPUT_RATE_HZ, band);
        if looks_analog(verdict.modulation)
            && band.bandwidth_hz <= PROBE_BANDWIDTH_HZ
            && waveform.envelope_variation <= PROBE_ENVELOPE
            && let Some(probe) = framing::probe(&self.window, INPUT_RATE_HZ, band)
        {
            verdict.modulation = probe.modulation;
            verdict.confidence = verdict.confidence.max(PROBED_CONFIDENCE);
            verdict.sideband = None;
            candidates.splice(0..0, probe.matches);
        }
        self.confirmer.confirm(
            &mut candidates,
            &self.window,
            INPUT_RATE_HZ,
            band,
            frequency_hz,
        );

        IdentSignal {
            modulation: verdict.modulation,
            confidence: verdict.confidence,
            sideband: verdict.sideband,
            symbol_rate_hz: waveform.symbol_rate_hz,
            deviation_hz: shifts(verdict.modulation).then_some(waveform.deviation_hz),
            burst_ms: waveform.burst_ms,
            burst_period_ms: waveform.burst_period_ms,
            ofdm_symbol_us: waveform.ofdm_symbol_us,
            ofdm_guard_us: waveform.ofdm_guard_us,
            candidates,
            features: IdentFeatures {
                envelope_variation: waveform.envelope_variation,
                duty: waveform.duty,
                keying_depth_db: waveform.on_off_db,
                spectral_asymmetry: band.skew,
                carrier_db: band.carrier_db,
                spectral_flatness: band.flatness,
                frequency_levels: waveform.frequency_levels,
                frequency_spread_hz: waveform.frequency_spread_hz,
                square_line_db: waveform.square_line_db,
                quartic_line_db: waveform.quartic_line_db,
            },
            ..located
        }
    }

    fn gap_hz(&self) -> f64 {
        if self.frequency_hz > 0.0 && self.frequency_hz < HF_TOP_HZ {
            HF_GAP_HZ
        } else {
            GAP_HZ
        }
    }

    fn fills_the_span(&mut self) -> bool {
        steady(&self.window)
            || self.keyed_blocks()
            || self
                .meter
                .cyclic_prefix(&self.window, INPUT_RATE_HZ, self.params.bandwidth_hz)
                .is_some()
    }

    fn keyed_blocks(&mut self) -> bool {
        self.block_power.clear();
        self.block_power.extend(
            self.window
                .as_chunks::<OCCUPANCY_BLOCK>()
                .0
                .iter()
                .map(|block| block.iter().map(|s| f64::from(s.norm_sqr())).sum::<f64>()),
        );
        if self.block_power.len() < 8 {
            return false;
        }
        let quiet = quantile(&mut self.block_power, 0.1);
        let loud = quantile(&mut self.block_power, 0.9);
        quiet > 0.0 && 10.0 * (loud / quiet).log10() >= KEYED_SPAN_DB
    }
}

fn steady(iq: &[Complex<f32>]) -> bool {
    let mut sum = 0.0f64;
    let mut sum_sq = 0.0f64;
    for sample in iq {
        let power = f64::from(sample.norm_sqr());
        sum += power;
        sum_sq += power * power;
    }
    let n = iq.len() as f64;
    if n < 2.0 || sum <= 0.0 {
        return false;
    }
    let mean = sum / n;
    let variation = (sum_sq / n - mean * mean).max(0.0).sqrt() / mean;
    variation < STEADY_POWER_VARIATION
}

fn quantile(values: &mut [f64], fraction: f64) -> f64 {
    let index = ((values.len() - 1) as f64 * fraction) as usize;
    let (_, value, _) = values.select_nth_unstable_by(index, f64::total_cmp);
    *value
}

const fn looks_analog(modulation: Modulation) -> bool {
    matches!(
        modulation,
        Modulation::Fm | Modulation::Am | Modulation::Unknown | Modulation::NoiseLike
    )
}

const fn shifts(modulation: Modulation) -> bool {
    matches!(
        modulation,
        Modulation::Fm | Modulation::Fsk2 | Modulation::Fsk4 | Modulation::Fsk8
    )
}

impl ChannelRx for IdentChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let params = *params(&settings)?;
        check_params(&params)?;
        Ok(Self {
            window: Vec::with_capacity(interval_samples(&params).min(MAX_WINDOW)),
            params,
            frequency_hz: settings.frequency_hz,
            pending: 0,
            detector: detect::Detector::new(),
            meter: features::Meter::new(),
            tracker: agreement::Tracker::new(),
            confirmer: confirm::Confirmer::new(),
            artifact_hz: None,
            block_power: Vec::with_capacity(MAX_WINDOW / OCCUPANCY_BLOCK),
            last_heard: None,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let params = *params(&settings)?;
        check_params(&params)?;
        if interval_samples(&params) != interval_samples(&self.params)
            || settings.frequency_hz != self.frequency_hz
        {
            self.forget();
        }
        self.params = params;
        self.frequency_hz = settings.frequency_hz;
        Ok(())
    }

    fn retuned(&mut self) {
        self.forget();
    }

    fn lo_artifact_at(&mut self, offset_hz: Option<f64>) {
        if offset_hz != self.artifact_hz {
            self.artifact_hz = offset_hz;
            self.forget();
        }
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        let interval = interval_samples(&self.params);
        let mut rest = iq;
        while !rest.is_empty() {
            let take = (interval - self.pending).min(rest.len());
            self.window.extend_from_slice(&rest[..take]);
            self.pending += take;
            rest = &rest[take..];
            if self.window.len() > MAX_WINDOW {
                self.window.drain(..self.window.len() - MAX_WINDOW);
            }
            if self.pending >= interval {
                let report = self.analyse();
                if self.worth_reporting(&report) {
                    self.last_heard = Some(!report.signals.is_empty());
                    out.events.push(DecoderEvent::Ident(report));
                }
                self.restart();
            }
        }
    }
}

pub(crate) fn in_allocation(kind: &str, frequency_hz: f64) -> bool {
    catalog::in_allocation(kind, frequency_hz)
}

pub(crate) fn identifiable(kind: &str) -> bool {
    catalog::identifiable(kind)
}

pub(crate) fn identify(
    iq: &[Complex<f32>],
    rate: f64,
    band: &detect::Band,
    center: f64,
) -> IdentSignal {
    let mut signal = IdentSignal {
        frequency_hz: center + band.center_hz,
        center_offset_hz: band.center_hz,
        bandwidth_hz: band.bandwidth_hz,
        snr_db: band.snr_db,
        ..IdentSignal::default()
    };
    let Some(zoom) = features::zoom(iq, rate, band) else {
        return signal;
    };
    let waveform = features::Meter::new().measure(&zoom, band);
    let verdict = classify::classify(band, &waveform);
    signal.modulation = verdict.modulation;
    signal.confidence = verdict.confidence;
    signal.sideband = verdict.sideband;
    signal.symbol_rate_hz = waveform.symbol_rate_hz;
    signal.deviation_hz = shifts(verdict.modulation).then_some(waveform.deviation_hz);
    signal.burst_ms = waveform.burst_ms;
    signal.burst_period_ms = waveform.burst_period_ms;
    signal.ofdm_symbol_us = waveform.ofdm_symbol_us;
    signal.ofdm_guard_us = waveform.ofdm_guard_us;
    signal.features = IdentFeatures {
        envelope_variation: waveform.envelope_variation,
        duty: waveform.duty,
        keying_depth_db: waveform.on_off_db,
        spectral_asymmetry: band.skew,
        carrier_db: band.carrier_db,
        spectral_flatness: band.flatness,
        frequency_levels: waveform.frequency_levels,
        frequency_spread_hz: waveform.frequency_spread_hz,
        square_line_db: waveform.square_line_db,
        quartic_line_db: waveform.quartic_line_db,
    };
    signal.candidates = catalog::candidates(
        verdict.modulation,
        band,
        &waveform,
        Some(signal.frequency_hz),
    );
    framing::confirm(&mut signal.candidates, iq, rate, band);
    if (looks_analog(signal.modulation) || signal.modulation == Modulation::Ook)
        && band.bandwidth_hz <= PROBE_BANDWIDTH_HZ
        && waveform.envelope_variation <= PROBE_ENVELOPE
        && let Some(probe) = framing::probe(iq, rate, band)
    {
        signal.modulation = probe.modulation;
        signal.confidence = PROBED_CONFIDENCE;
        signal.candidates.splice(0..0, probe.matches);
    }
    signal
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use num_complex::Complex;
    use sdrmm_wire::{
        ChannelSettings, DecoderEvent, IdentParams, IdentReport, IdentSignal, Modulation,
    };

    use super::{INPUT_RATE_HZ, IdentChannel, MAX_WINDOW};
    use crate::{
        ChannelCtx, ChannelOutputs, ChannelRx,
        testgen::{self, dv as tgdv},
        testutil::{complex_noise, realtime_budget},
    };

    const INTERVAL_MS: u32 = 500;

    fn params() -> IdentParams {
        IdentParams {
            interval_ms: INTERVAL_MS,
            ..IdentParams::default()
        }
    }

    fn settings(params: IdentParams) -> ChannelSettings {
        settings_at(params, 0.0)
    }

    fn settings_at(params: IdentParams, frequency_hz: f64) -> ChannelSettings {
        ChannelSettings {
            frequency_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: sdrmm_wire::ChannelParams::Ident(params),
            blanker: Default::default(),
        }
    }

    fn run(params: IdentParams, iq: &[Complex<f32>]) -> Vec<IdentReport> {
        run_at(settings(params), iq)
    }

    fn run_at(settings: ChannelSettings, iq: &[Complex<f32>]) -> Vec<IdentReport> {
        let ctx = ChannelCtx {
            input_rate: INPUT_RATE_HZ,
        };
        let mut channel =
            IdentChannel::new(ctx, settings).expect("ident channel builds at its own rate");
        let mut out = ChannelOutputs::default();
        let mut reports = Vec::new();
        let mut pos = 0;
        for len in [8_191usize, 1, 65_536, 129, 4_096].iter().cycle() {
            if pos >= iq.len() {
                break;
            }
            let end = (pos + len).min(iq.len());
            out.reset();
            channel.process(&iq[pos..end], &mut out);
            for event in out.events.drain(..) {
                match event {
                    DecoderEvent::Ident(report) => reports.push(report),
                    other => panic!("unexpected {} event", other.kind()),
                }
            }
            pos = end;
        }
        reports
    }

    fn in_noise(iq: &[Complex<f32>], seconds: f64, seed: u32, amp: f32) -> Vec<Complex<f32>> {
        let wanted = (seconds * INPUT_RATE_HZ) as usize;
        let mut out = Vec::with_capacity(wanted + iq.len());
        while out.len() < wanted {
            out.extend_from_slice(iq);
        }
        let noise = complex_noise(seed, amp, out.len());
        for (sample, noise) in out.iter_mut().zip(noise) {
            *sample += noise;
        }
        out
    }

    fn on_air(iq: &[Complex<f32>], seconds: f64, seed: u32) -> Vec<Complex<f32>> {
        in_noise(iq, seconds, seed, 0.004)
    }

    fn programme(len: usize, seed: u32) -> Vec<f32> {
        let mut state = seed | 1;
        let mut smoothed = 0.0f32;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let noise = state as f32 / u32::MAX as f32 - 0.5;
                smoothed += 0.08 * (noise - smoothed);
                smoothed * 8.0
            })
            .collect()
    }

    fn best(report: &IdentReport) -> Option<&str> {
        report.best().map(|m| m.name.as_str())
    }

    fn family(report: &IdentReport) -> Modulation {
        report
            .loudest()
            .map_or(Modulation::None, |signal| signal.modulation)
    }

    fn loudest(report: &IdentReport) -> &IdentSignal {
        report.loudest().expect("a signal was found")
    }

    fn consensus(reports: &[IdentReport]) -> Modulation {
        let mut counts: Vec<(Modulation, usize)> = Vec::new();
        for report in reports {
            let found = family(report);
            match counts.iter_mut().find(|(m, _)| *m == found) {
                Some((_, n)) => *n += 1,
                None => counts.push((found, 1)),
            }
        }
        counts
            .into_iter()
            .max_by_key(|&(_, n)| n)
            .map_or(Modulation::None, |(m, _)| m)
    }

    fn carrier(offset: f64, len: usize, seed: u32) -> Vec<Complex<f32>> {
        let mut iq: Vec<Complex<f32>> = (0..len)
            .map(|k| {
                Complex::from_polar(
                    0.5,
                    (std::f64::consts::TAU * offset * k as f64 / INPUT_RATE_HZ)
                        .rem_euclid(std::f64::consts::TAU) as f32,
                )
            })
            .collect();
        for (s, n) in iq.iter_mut().zip(complex_noise(seed, 0.002, len)) {
            *s += n;
        }
        iq
    }

    #[test]
    fn a_quiet_channel_says_no_signal_once_and_then_keeps_quiet() {
        let noise = complex_noise(0x2b71, 0.02, (INPUT_RATE_HZ * 2.2) as usize);
        let reports = run(params(), &noise);
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert!(reports[0].signals.is_empty());
        assert!(reports[0].best().is_none());
    }

    #[test]
    fn reports_arrive_once_per_interval_while_something_is_on_the_air() {
        let iq = carrier(42_000.0, (INPUT_RATE_HZ * 2.0) as usize, 0x9c14);
        let reports = run(params(), &iq);
        assert_eq!(reports.len(), 4);
        assert!(reports.iter().all(|report| !report.signals.is_empty()));
    }

    #[test]
    fn a_signal_that_stops_is_reported_gone_once() {
        let mut iq = carrier(42_000.0, (INPUT_RATE_HZ * 1.0) as usize, 0x9c15);
        iq.extend(complex_noise(0x9c16, 0.02, (INPUT_RATE_HZ * 1.6) as usize));
        let reports = run(params(), &iq);
        let heard = reports
            .iter()
            .take_while(|report| !report.signals.is_empty())
            .count();
        assert_eq!(heard, 2, "{reports:?}");
        assert_eq!(reports.len(), 3, "one report says the signal is gone");
        assert!(reports[2].signals.is_empty());
    }

    #[test]
    fn a_retune_reports_the_new_dial_even_when_it_is_quiet() {
        let ctx = ChannelCtx {
            input_rate: INPUT_RATE_HZ,
        };
        let mut channel = IdentChannel::new(ctx, settings(params())).expect("builds");
        let mut out = ChannelOutputs::default();
        let window = (INPUT_RATE_HZ * f64::from(INTERVAL_MS) / 1_000.0) as usize;
        channel.process(&complex_noise(0x5566, 0.02, 2 * window), &mut out);
        assert_eq!(out.events.len(), 1);
        channel.retuned();
        channel.process(&complex_noise(0x7788, 0.02, window), &mut out);
        assert_eq!(out.events.len(), 2, "the new dial gets its own first word");
    }

    #[test]
    fn an_unmodulated_carrier_is_named_and_located() {
        let offset = 42_000.0;
        let iq = carrier(offset, (INPUT_RATE_HZ * 1.2) as usize, 0x4411);
        let reports = run(params(), &iq);
        assert_eq!(consensus(&reports), Modulation::Carrier);
        let first = loudest(&reports[0]);
        assert!(
            (first.center_offset_hz - offset).abs() < 500.0,
            "offset {} Hz",
            first.center_offset_hz
        );
        assert_eq!(best(&reports[0]), Some("Unmodulated carrier"));
    }

    #[test]
    fn a_dmr_transmission_is_four_level_and_confirmed_by_its_framing() {
        let call = tgdv::dmr::Call::default();
        let iq = on_air(&tgdv::dmr::transmission(&call, INPUT_RATE_HZ), 2.0, 0x71a2);
        let reports = run(params(), &iq);
        assert_eq!(consensus(&reports), Modulation::Fsk4);
        let confirmed = reports
            .iter()
            .find(|r| r.best().is_some_and(|m| m.confirmed))
            .expect("a DMR transmission carries its own frame sync");
        let best = confirmed.best().expect("checked above");
        assert_eq!(best.name, "DMR");
        assert_eq!(best.type_id.as_deref(), Some("dmr"));
        let signal = loudest(confirmed);
        assert!(
            (signal.symbol_rate_hz.unwrap_or_default() - 4_800.0).abs() < 250.0,
            "baud {:?}",
            signal.symbol_rate_hz
        );
    }

    #[test]
    fn a_p25_transmission_is_told_apart_from_dmr() {
        let iq = on_air(&tgdv::p25::transmission(0x293, INPUT_RATE_HZ), 2.0, 0x33c1);
        let reports = run(params(), &iq);
        assert_eq!(consensus(&reports), Modulation::Fsk4);
        let confirmed = reports
            .iter()
            .find(|r| r.best().is_some_and(|m| m.confirmed))
            .expect("a P25 transmission carries its own frame sync");
        assert_eq!(best(confirmed), Some("P25 Phase 1"));
        assert!(
            loudest(confirmed)
                .candidates
                .iter()
                .any(|m| m.name == "DMR")
        );
    }

    #[test]
    fn a_pager_transmission_is_two_level_at_its_own_baud() {
        let pages = [testgen::pocsag::Page {
            address: 1_234_567,
            function: 3,
            text: "IDENT TEST".to_owned(),
            numeric: false,
        }];
        let iq = on_air(
            &testgen::pocsag::transmission(&pages, 1_200, 4_500.0, INPUT_RATE_HZ),
            2.0,
            0x5d90,
        );
        let reports = run(params(), &iq);
        eprintln!(
            "DEBUG-PAGER {:?}",
            reports
                .iter()
                .map(|r| r.loudest().map(|s| (
                    s.modulation,
                    s.symbol_rate_hz,
                    s.deviation_hz,
                    s.bandwidth_hz,
                    &s.candidates
                )))
                .collect::<Vec<_>>()
        );
        assert_eq!(consensus(&reports), Modulation::Fsk2);
        let named = reports
            .iter()
            .find(|r| best(r) == Some("POCSAG (1200 bd)"))
            .expect("a 1200 baud pager shift is POCSAG at 1200 baud");
        let signal = loudest(named);
        assert!(
            (signal.deviation_hz.unwrap_or_default() - 4_500.0).abs() < 1_200.0,
            "deviation {:?}",
            signal.deviation_hz
        );
        let confirmed = reports
            .iter()
            .find(|r| r.best().is_some_and(|m| m.confirmed))
            .expect("the POCSAG decoder reads the pages");
        assert_eq!(best(confirmed), Some("POCSAG (1200 bd)"));
        assert!(
            confirmed
                .best()
                .is_some_and(|m| m.why.contains("decoder read")),
            "{:?}",
            confirmed.best()
        );
    }

    #[test]
    fn a_weak_pager_is_a_shift_and_not_amplitude_modulation() {
        let pages = [testgen::pocsag::Page {
            address: 1_234_567,
            function: 3,
            text: "IDENT TEST".to_owned(),
            numeric: false,
        }];
        let iq = in_noise(
            &testgen::pocsag::transmission(&pages, 1_200, 4_500.0, INPUT_RATE_HZ),
            2.0,
            0x5d90,
            0.8,
        );
        let reports = run(params(), &iq);
        let strongest = reports
            .iter()
            .filter_map(|r| r.loudest())
            .map(|s| s.snr_db)
            .fold(0.0, f32::max);
        assert!(
            strongest < 20.0,
            "meant to be a weak signal, got {strongest} dB"
        );
        assert_eq!(consensus(&reports), Modulation::Fsk2);
        assert!(
            reports.iter().any(|r| r
                .best()
                .is_some_and(|m| m.type_id.as_deref() == Some("pocsag"))),
            "candidates: {:?}",
            reports.iter().map(best).collect::<Vec<_>>()
        );
    }

    #[test]
    fn weak_fm_voice_stays_analog() {
        let audio = programme((INPUT_RATE_HZ * 2.2) as usize, 0x4d21);
        let iq = in_noise(
            &testgen::fm_modulate(&audio, 3_000.0, INPUT_RATE_HZ),
            2.0,
            0x2ea7,
            1.2,
        );
        let reports = run(params(), &iq);
        let strongest = reports
            .iter()
            .filter_map(|r| r.loudest())
            .map(|s| s.snr_db)
            .fold(0.0, f32::max);
        assert!(
            strongest < 20.0,
            "meant to be a weak signal, got {strongest} dB"
        );
        assert_eq!(consensus(&reports), Modulation::Fm);
    }

    #[test]
    fn a_remote_control_is_keyed_rather_than_shifted() {
        let frame = testgen::subghz::Pwm {
            bits: (0..24).map(|i| 0x0A_1B23u32 >> (23 - i) & 1 == 1).collect(),
            short_us: 320,
            long_multiple: 3,
            sync_gap_multiple: 31,
            repeats: 6,
        };
        let iq = on_air(&testgen::subghz::pwm(&frame, INPUT_RATE_HZ), 2.0, 0x1e44);
        let reports = run(params(), &iq);
        assert_eq!(consensus(&reports), Modulation::Ook);
        assert!(
            reports
                .iter()
                .any(|r| best(r) == Some("Sub-GHz remote (OOK)")),
            "candidates: {:?}",
            reports.iter().map(best).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_broadcast_signal_is_wideband_fm() {
        let audio = programme(48_000, 0x4d21);
        let iq = on_air(
            &testgen::wfm::transmission(&audio, &audio, true, INPUT_RATE_HZ),
            1.6,
            0x6f02,
        );
        let reports = run(params(), &iq);
        assert_eq!(consensus(&reports), Modulation::Fm);
        assert!(
            reports.iter().any(|r| best(r) == Some("FM broadcast")),
            "candidates: {:?}",
            reports.iter().map(best).collect::<Vec<_>>()
        );
    }

    fn broadcast_audio(len: usize, seed: u32, smoothing: f32) -> Vec<f32> {
        let mut state = seed | 1;
        let mut smoothed = 0.0f32;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let noise = state as f32 / u32::MAX as f32 - 0.5;
                smoothed += smoothing * (noise - smoothed);
                (smoothed * 14.0).clamp(-1.0, 1.0)
            })
            .collect()
    }

    fn station(smoothing: f32) -> Vec<Complex<f32>> {
        let len = (INPUT_RATE_HZ * 2.2) as usize;
        let mut iq = testgen::wfm::transmission(
            &broadcast_audio(len, 0x4d21, smoothing),
            &broadcast_audio(len, 0x7712, smoothing),
            true,
            INPUT_RATE_HZ,
        );
        testgen::add_noise(&mut iq, 0x6f02, 0.004);
        iq
    }

    #[test]
    fn a_loud_stereo_station_is_broadcast_fm_in_every_window() {
        let reports = run(params(), &station(0.35));
        assert!(reports.len() >= 4, "{} reports", reports.len());
        for report in &reports {
            assert_eq!(family(report), Modulation::Fm, "{report:?}");
            assert_eq!(best(report), Some("FM broadcast"));
            let signal = loudest(report);
            assert!(signal.confidence > 0.6, "confidence {}", signal.confidence);
        }
    }

    #[test]
    fn a_processed_station_is_neither_keyed_nor_shifted() {
        let reports = run(params(), &station(1.0));
        assert!(reports.len() >= 4, "{} reports", reports.len());
        for report in &reports {
            assert_eq!(family(report), Modulation::Fm, "{report:?}");
            let signal = loudest(report);
            assert!(
                signal.bandwidth_hz > 100_000.0,
                "bandwidth {} Hz",
                signal.bandwidth_hz
            );
        }
    }

    #[test]
    fn a_station_carrying_one_tone_is_still_broadcast_fm() {
        let len = (INPUT_RATE_HZ * 2.2) as usize;
        let tone = testgen::tone_audio(1_000.0, 1.0, INPUT_RATE_HZ, len);
        let mut iq = testgen::wfm::transmission(&tone, &tone, true, INPUT_RATE_HZ);
        testgen::add_noise(&mut iq, 0x6f02, 0.004);
        let reports = run_at(settings_at(params(), 95_500_000.0), &iq);
        assert_eq!(consensus(&reports), Modulation::Fm, "{reports:?}");
        assert!(
            reports.iter().any(|r| best(r) == Some("FM broadcast")),
            "candidates: {:?}",
            reports.iter().map(best).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_keyed_carrier_is_morse_rather_than_a_bare_carrier() {
        let mut iq = testgen::morse::transmission("CQ CQ DE TEST", 20.0, 800.0, INPUT_RATE_HZ);
        testgen::add_noise(&mut iq, 0x3311, 0.004);
        let reports = run(params(), &iq);
        assert_eq!(consensus(&reports), Modulation::Ook);
        assert!(
            reports.iter().any(|r| best(r) == Some("Morse (CW)")),
            "candidates: {:?}",
            reports.iter().map(best).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_deeply_modulated_carrier_is_amplitude_modulation_not_keying() {
        let len = (INPUT_RATE_HZ * 2.2) as usize;
        let mut iq: Vec<Complex<f32>> = testgen::tone_audio(1_000.0, 1.0, INPUT_RATE_HZ, len)
            .iter()
            .map(|&a| Complex::new(0.5 * (1.0 + 0.8 * a), 0.0))
            .collect();
        testgen::add_noise(&mut iq, 0x5511, 0.004);
        let reports = run(params(), &iq);
        assert_eq!(consensus(&reports), Modulation::Am);
        assert!(
            reports.iter().all(|r| family(r) != Modulation::Ook),
            "an 80 percent modulated carrier dips without ever being keyed off"
        );
        assert_eq!(best(&reports[0]), Some("AM voice"));
    }

    #[test]
    fn a_retune_discards_the_half_window_it_was_holding() {
        let ctx = ChannelCtx {
            input_rate: INPUT_RATE_HZ,
        };
        let mut channel = IdentChannel::new(ctx, settings(params())).expect("builds");
        let mut out = ChannelOutputs::default();
        let half = (INPUT_RATE_HZ * f64::from(INTERVAL_MS) / 1_000.0) as usize / 2;
        channel.process(&complex_noise(0x1122, 0.02, half + 10), &mut out);
        assert!(out.events.is_empty());
        channel.retuned();
        channel.process(&complex_noise(0x3344, 0.02, half), &mut out);
        assert!(
            out.events.is_empty(),
            "the pre-retune samples must not have counted towards this window"
        );
    }

    #[test]
    fn one_report_costs_far_less_than_the_signal_it_describes() {
        let iq = on_air(
            &tgdv::dmr::transmission(&tgdv::dmr::Call::default(), INPUT_RATE_HZ),
            2.0,
            0x0f31,
        );
        let _ = run(params(), &iq);

        let mut reports = Vec::new();
        let mut elapsed = Vec::with_capacity(3);
        for _ in 0..3 {
            let started = Instant::now();
            reports = run(params(), &iq);
            elapsed.push(started.elapsed());
        }
        elapsed.sort_unstable();
        let elapsed = elapsed[1].as_secs_f64();
        let described = reports.len() as f64 * f64::from(INTERVAL_MS) / 1_000.0;
        assert!(described > 0.0, "the run produced no reports");
        assert!(
            elapsed < realtime_budget(described / 2.0),
            "identification took {elapsed:.3} s for {described:.1} s of signal"
        );
    }

    #[test]
    fn a_long_interval_lengthens_the_cadence_rather_than_the_analysis() {
        let long = IdentParams {
            interval_ms: 2_000,
            ..IdentParams::default()
        };
        let noise = complex_noise(0x77c2, 0.02, (INPUT_RATE_HZ * 4.5) as usize);
        let on_air = carrier(42_000.0, noise.len(), 0x77c3);
        assert_eq!(run(long, &on_air).len(), 2);

        let ctx = ChannelCtx {
            input_rate: INPUT_RATE_HZ,
        };
        let mut channel = IdentChannel::new(ctx, settings(long)).expect("ident channel");
        let mut out = ChannelOutputs::default();
        channel.process(&noise[..MAX_WINDOW + 1], &mut out);
        assert_eq!(channel.window.len(), MAX_WINDOW);
        assert!(out.events.is_empty());
    }

    #[test]
    fn two_transmissions_in_one_window_are_both_named() {
        let seconds = 2.0;
        let mut iq = on_air(
            &tgdv::dmr::transmission(&tgdv::dmr::Call::default(), INPUT_RATE_HZ),
            seconds,
            0x71a2,
        );
        testgen::shift(&mut iq, 60_000.0, INPUT_RATE_HZ);
        let pages = [testgen::pocsag::Page {
            address: 1_234_567,
            function: 3,
            text: "SURVEY".to_owned(),
            numeric: false,
        }];
        let mut pager = in_noise(
            &testgen::pocsag::transmission(&pages, 1_200, 4_500.0, INPUT_RATE_HZ),
            seconds,
            0x5d90,
            0.0,
        );
        testgen::shift(&mut pager, -50_000.0, INPUT_RATE_HZ);
        for (s, p) in iq.iter_mut().zip(&pager) {
            *s += p;
        }
        let reports = run(params(), &iq);
        let survey = reports
            .iter()
            .find(|r| r.signals.len() == 2)
            .unwrap_or_else(|| panic!("both signals are listed: {reports:?}"));
        let at = |offset: f64| {
            survey
                .signals
                .iter()
                .find(|s| (s.center_offset_hz - offset).abs() < 3_000.0)
                .unwrap_or_else(|| panic!("no signal near {offset} Hz in {:?}", survey.signals))
        };
        assert_eq!(at(60_000.0).modulation, Modulation::Fsk4);
        assert_eq!(at(-50_000.0).modulation, Modulation::Fsk2);
        assert_eq!(
            at(-50_000.0).best().map(|m| m.name.as_str()),
            Some("POCSAG (1200 bd)")
        );
    }

    #[test]
    fn a_signal_knows_where_it_sits_on_the_dial() {
        let offset = 42_000.0;
        let dial = 145_000_000.0;
        let len = (INPUT_RATE_HZ * 1.2) as usize;
        let mut iq: Vec<Complex<f32>> = (0..len)
            .map(|k| {
                Complex::from_polar(
                    0.5,
                    (std::f64::consts::TAU * offset * k as f64 / INPUT_RATE_HZ)
                        .rem_euclid(std::f64::consts::TAU) as f32,
                )
            })
            .collect();
        testgen::add_noise(&mut iq, 0x4411, 0.002);
        let reports = run_at(settings_at(params(), dial), &iq);
        let signal = loudest(&reports[0]);
        assert!(
            (signal.frequency_hz - (dial + offset)).abs() < 500.0,
            "{} Hz",
            signal.frequency_hz
        );
    }

    #[test]
    fn the_dial_frequency_is_part_of_the_evidence() {
        let pages = [testgen::pocsag::Page {
            address: 1_234_567,
            function: 3,
            text: "IDENT TEST".to_owned(),
            numeric: false,
        }];
        let iq = on_air(
            &testgen::pocsag::transmission(&pages, 1_200, 4_500.0, INPUT_RATE_HZ),
            1.2,
            0x5d90,
        );
        let reports = run_at(settings_at(params(), 466_075_000.0), &iq);
        let placed = reports
            .iter()
            .find(|r| best(r) == Some("POCSAG (1200 bd)"))
            .expect("named");
        assert!(
            placed
                .best()
                .is_some_and(|m| m.why.contains("allocation") || m.confirmed),
            "{:?}",
            placed.best()
        );
    }

    #[test]
    fn a_slotted_transmission_reports_its_bursts_and_the_bursts_favour_dmr() {
        let call = tgdv::dmr::Call::default();
        let iq = on_air(
            &tgdv::dmr::simplex_transmission(&call, INPUT_RATE_HZ),
            2.0,
            0x71a3,
        );
        let reports = run(params(), &iq);
        assert_eq!(consensus(&reports), Modulation::Fsk4);
        let bursty = reports
            .iter()
            .filter_map(|r| r.loudest())
            .find(|s| s.burst_ms.is_some())
            .unwrap_or_else(|| panic!("a single-slot call is bursty: {reports:?}"));
        let burst = bursty.burst_ms.unwrap_or_default();
        assert!((burst - 30.0).abs() < 5.0, "burst {burst} ms");
        let period = bursty.burst_period_ms.expect("slots repeat every frame");
        assert!((period - 60.0).abs() < 6.0, "period {period} ms");
        assert_eq!(bursty.best().map(|m| m.name.as_str()), Some("DMR"));
    }

    fn ofdm_like_dab(seconds: f64, seed: u32) -> Vec<Complex<f32>> {
        const RATE: f64 = 2_048_000.0;
        const USEFUL: usize = 2_048;
        const GUARD: usize = 504;
        const CARRIERS: usize = 1_536;
        let mut fft = sdrmm_dsp::fft::FftPair::new(USEFUL);
        let mut state = seed | 1;
        let symbols = (seconds * RATE / (USEFUL + GUARD) as f64) as usize;
        let mut out = Vec::with_capacity(symbols * (USEFUL + GUARD));
        let mut symbol = vec![Complex::default(); USEFUL];
        for _ in 0..symbols {
            symbol.fill(Complex::default());
            for k in 1..=CARRIERS / 2 {
                for index in [k, USEFUL - k] {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    let phase = (state % 4) as f32 * std::f32::consts::FRAC_PI_2
                        + std::f32::consts::FRAC_PI_4;
                    symbol[index] = Complex::from_polar(2.0, phase);
                }
            }
            fft.inverse_scaled(&mut symbol);
            out.extend_from_slice(&symbol[USEFUL - GUARD..]);
            out.extend_from_slice(&symbol);
        }
        testgen::resample(&out, RATE, INPUT_RATE_HZ)
    }

    #[test]
    fn a_slice_of_a_dab_ensemble_is_ofdm_with_a_millisecond_symbol() {
        let mut iq = ofdm_like_dab(2.2, 0x0da8);
        testgen::add_noise(&mut iq, 0x0da8, 0.002);
        let reports = run_at(settings_at(params(), 227_360_000.0), &iq);
        assert_eq!(consensus(&reports), Modulation::Ofdm, "{reports:?}");
        let signal = reports
            .iter()
            .filter_map(|r| r.loudest())
            .find(|s| s.modulation == Modulation::Ofdm)
            .expect("checked above");
        let symbol = signal.ofdm_symbol_us.expect("useful symbol");
        assert!((symbol - 1_000.0).abs() < 30.0, "symbol {symbol} µs");
        let guard = signal.ofdm_guard_us.expect("guard interval");
        assert!((guard - 246.0).abs() < 25.0, "guard {guard} µs");
        assert_eq!(signal.best().map(|m| m.name.as_str()), Some("DAB / DAB+"));
    }

    #[test]
    fn a_broadcast_station_is_confirmed_by_its_rds() {
        let station = testgen::rds::Station {
            pi: 0xD3C2,
            ps: "SDR-M4  ".to_owned(),
            radiotext: "identifier".to_owned(),
            pty: 10,
            tp: true,
            ta: false,
            music: true,
            alt_freqs_hz: Vec::new(),
        };
        let mut iq = testgen::rds::transmission(&station, 2.5, Some(1_000.0), INPUT_RATE_HZ);
        testgen::add_noise(&mut iq, 0x6f02, 0.002);
        let reports = run_at(settings_at(params(), 95_500_000.0), &iq);
        let confirmed = reports
            .iter()
            .find(|r| r.best().is_some_and(|m| m.confirmed))
            .unwrap_or_else(|| panic!("the wfm decoder reads the RDS groups: {reports:?}"));
        assert_eq!(best(confirmed), Some("FM broadcast"));
    }
}
