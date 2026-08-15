mod catalog;
mod classify;
mod detect;
mod features;
mod framing;

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass, flat_bandwidth_hz};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, IdentFeatures, IdentParams,
    IdentReport, MAX_IDENT_BANDWIDTH_HZ, MAX_IDENT_INTERVAL_MS, MAX_IDENT_THRESHOLD_DB,
    MIN_IDENT_BANDWIDTH_HZ, MIN_IDENT_INTERVAL_MS, MIN_IDENT_THRESHOLD_DB, Modulation,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const INPUT_RATE_HZ: f64 = 240_000.0;

const CHANNEL_TAPS: usize = 63;

const MAX_WINDOW: usize = 262_144;

const MIN_WINDOW: usize = 4 * detect::DETECT_FFT;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "ident".to_owned(),
    name: "Signal identifier".to_owned(),
    bandwidth_hz: MAX_IDENT_BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    decoder_kind: Some("ident".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct IdentChannel {
    params: IdentParams,
    window: Vec<Complex<f32>>,
    pending: usize,
    detector: detect::Detector,
    meter: features::Meter,
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
    if !(MIN_IDENT_INTERVAL_MS..=MAX_IDENT_INTERVAL_MS).contains(&p.interval_ms) {
        return Err(ChannelError::InvalidSettings(format!(
            "ident interval must be in [{MIN_IDENT_INTERVAL_MS}, {MAX_IDENT_INTERVAL_MS}] ms, got {}",
            p.interval_ms
        )));
    }
    if !(p.threshold_db.is_finite()
        && (MIN_IDENT_THRESHOLD_DB..=MAX_IDENT_THRESHOLD_DB).contains(&p.threshold_db))
    {
        return Err(ChannelError::InvalidSettings(format!(
            "ident threshold must be in [{MIN_IDENT_THRESHOLD_DB}, {MAX_IDENT_THRESHOLD_DB}] dB, got {}",
            p.threshold_db
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

    fn analyse(&mut self) -> IdentReport {
        let measured = self.detector.measure(
            &self.window,
            INPUT_RATE_HZ,
            self.params.bandwidth_hz / 2.0,
            self.params.threshold_db,
        );
        let Some(band) = measured.band else {
            return IdentReport {
                snr_db: measured.peak_db - measured.floor_db,
                confidence: 1.0,
                ..IdentReport::default()
            };
        };
        let Some(zoom) = features::zoom(&self.window, INPUT_RATE_HZ, &band) else {
            return IdentReport {
                modulation: Modulation::Unknown,
                bandwidth_hz: band.bandwidth_hz,
                center_offset_hz: band.center_hz,
                snr_db: band.snr_db,
                ..IdentReport::default()
            };
        };

        let waveform = self.meter.measure(&zoom, &band);
        let verdict = classify::classify(&band, &waveform);
        let mut candidates = catalog::candidates(verdict.modulation, &band, &waveform);
        framing::confirm(&mut candidates, &self.window, INPUT_RATE_HZ, &band);

        IdentReport {
            modulation: verdict.modulation,
            confidence: verdict.confidence,
            sideband: verdict.sideband,
            bandwidth_hz: band.bandwidth_hz,
            center_offset_hz: band.center_hz,
            snr_db: band.snr_db,
            symbol_rate_hz: waveform.symbol_rate_hz,
            deviation_hz: shifts(verdict.modulation).then_some(waveform.deviation_hz),
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
        }
    }
}

const fn shifts(modulation: Modulation) -> bool {
    matches!(
        modulation,
        Modulation::Fm | Modulation::Fsk2 | Modulation::Fsk4
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
            pending: 0,
            detector: detect::Detector::new(),
            meter: features::Meter::new(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let params = *params(&settings)?;
        check_params(&params)?;
        if interval_samples(&params) != interval_samples(&self.params) {
            self.restart();
        }
        self.params = params;
        Ok(())
    }

    fn retuned(&mut self) {
        self.restart();
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
                out.events.push(DecoderEvent::Ident(report));
                self.restart();
            }
        }
    }
}

#[cfg(test)]
mod tests;
