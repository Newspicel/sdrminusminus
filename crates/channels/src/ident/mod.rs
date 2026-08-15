//! Signal identifier: point it at something unknown and it says what the modulation is and,
//! where it can, which protocol.
//!
//! This is a channel like any other — IQ in, decoder events out — but it demodulates nothing.
//! Each report is one observation window run through five stages:
//!
//! 1. [`detect`] averages the spectrum of the slice and pulls the occupied band out of it.
//! 2. [`features`] mixes that band to DC, decimates to a rate matched to it, and measures the
//!    waveform: envelope, instantaneous frequency, symbol clock, phase nonlinearities.
//! 3. [`classify`] walks a decision tree over those measurements to a modulation family.
//! 4. [`catalog`] scores every protocol signature the family admits.
//! 5. [`framing`] settles the digital-voice shortlist — whose members share a waveform — by
//!    demodulating against each candidate and looking for its frame sync.
//!
//! Reports carry the measurements they were decided from, so a verdict can be checked rather than
//! believed. Nothing here is a decode: the identifier tells an operator which channel type to
//! reach for, and that channel does the decoding.
//!
//! The analysis is bounded but not free — it is a handful of transforms, one decimation pass and,
//! at most, a couple of demodulations — so it runs once per `interval_ms` rather than per block,
//! and allocates while it does. That is a deliberate exception to the crate's no-allocation rule,
//! of the same shape and for the same reason as the host's own owned-event hand-off: it happens
//! once a second on a path whose cost is bounded by construction.

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

/// The rate the identifier meets the DDC at.
///
/// Wide enough to hold a broadcast FM signal whole, which is the widest thing anything else in
/// this build decodes — and exactly five times the digital-voice front-end rate, so the framing
/// search reaches 48 kHz by integer decimation alone and never through an interpolator.
const INPUT_RATE_HZ: f64 = 240_000.0;

/// Channel-selection filter length. The passband is most of the rate, so the transition band is
/// enormous and a short filter is ample.
const CHANNEL_TAPS: usize = 63;

/// Longest observation one report may stand on, in samples — a little over a second at the
/// channel rate. The cap is what keeps a long report interval from turning into an unbounded
/// buffer and an unbounded analysis; past it the *cadence* still lengthens, and each report
/// describes the second before it rather than the whole gap.
const MAX_WINDOW: usize = 262_144;

/// Shortest observation worth analysing. Below this the detection transform cannot be averaged
/// at all and the band edges would move between reports on a signal that never changed.
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
    /// What the next report will be measured from: the most recent [`MAX_WINDOW`] samples.
    window: Vec<Complex<f32>>,
    /// Samples since the last report, which is what the cadence is counted in — the window holds
    /// fewer than these once the interval outruns the analysis cap.
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
    // The band edges are read off an averaged spectrum, so the whole slice has to arrive at the
    // same level — the DDC's flat passband, not the wider band it merely keeps free of aliases.
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

/// Occupied RF band relative to the channel offset, in Hz.
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

/// Samples between reports.
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
                // How close the loudest thing in the slice came to the threshold. Reported so a
                // "nothing here" is a measurement rather than a shrug.
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

/// Whether a deviation is a thing this modulation has. Reporting one for a keyed *carrier* would
/// be quoting the discriminator's noise as a property of the transmission.
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
        // Half an observation from the old frequency and half from the new would be measured as
        // one signal, and reported as a protocol neither of them is.
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
