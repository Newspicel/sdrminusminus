//! Stage three of identification: decide which family the measurements describe.
//!
//! A decision tree rather than a trained classifier, because every branch here has to be
//! explainable to the operator reading it — the report carries the features the branch was taken
//! on, and a verdict nobody can check against them is worth less than no verdict.

use sdrmm_wire::{Modulation, Sideband};

use super::{detect::Band, features::Waveform};

/// Duty below which the carrier is being keyed rather than merely fading.
const KEYED_DUTY: f32 = 0.9;
/// On-to-off depth a keyed carrier must show. Fifteen dB is far more than fading or a fringe
/// signal's own amplitude noise, and far less than a real remote control's off state.
const KEYED_DEPTH_DB: f32 = 15.0;
/// Envelope spread, over the keyed-on samples, above which amplitude is carrying information.
const AMPLITUDE_MODULATED: f32 = 0.18;
/// Strength of the central line above which there is a carrier under the modulation.
const CARRIER_LINE_DB: f32 = 12.0;
/// Spectral skew beyond which the band is lopsided enough to be one sideband of something.
const SIDEBAND_SKEW: f32 = 0.45;
/// A band this narrow with nothing moving in it and no clock under it is a carrier. The width is
/// generous because a pure tone's *measured* occupied bandwidth is its analysis window's skirt
/// rather than anything the transmitter did.
const CARRIER_BANDWIDTH_HZ: f64 = 2_500.0;
const CARRIER_SPREAD_HZ: f64 = 100.0;
/// A nonlinearity line has to stand this far over its own spectrum to name a phase modulation.
const PHASE_LINE_DB: f32 = 10.0;
/// Wiener entropy above which the band has no structure left to find.
const FLAT_SPECTRUM: f32 = 0.55;
/// Frequency spread above which a constant-envelope signal is being frequency-modulated.
const FM_SPREAD_HZ: f64 = 200.0;
/// Frequency spread, as a fraction of the occupied bandwidth, below which the levels found in the
/// histogram are jitter rather than a shift.
const MIN_SHIFT_FRACTION: f64 = 0.02;
/// SNR at which the waveform measurements are worth full confidence, and the floor below which
/// they are worth almost none. Every ratio here is measured against noise that is also in the
/// band, so how far the signal stands out of it bounds how much any verdict can be believed.
const SNR_FLOOR_DB: f32 = 3.0;
const SNR_TRUSTED_DB: f32 = 18.0;

pub(crate) struct Verdict {
    pub(crate) modulation: Modulation,
    pub(crate) confidence: f32,
    pub(crate) sideband: Option<Sideband>,
}

/// How firmly `value` clears `threshold`, over a scale of `span`.
fn firmness(value: f32, threshold: f32, span: f32) -> f32 {
    ((value - threshold) / span).clamp(0.0, 1.0)
}

pub(crate) fn classify(band: &Band, waveform: &Waveform) -> Verdict {
    let quality = ((band.snr_db - SNR_FLOOR_DB) / (SNR_TRUSTED_DB - SNR_FLOOR_DB)).clamp(0.15, 1.0);
    let settle = |modulation, firmness: f32| Verdict {
        modulation,
        confidence: ((0.5 + 0.5 * firmness) * quality).clamp(0.0, 1.0),
        sideband: None,
    };

    // Frequency structure is read before anything about the envelope, because the envelope
    // cannot distinguish the two things that produce a keyed carrier: a remote control switching
    // its transmitter, and a TDMA radio that occupies one timeslot in two. Both key off between
    // bursts; only one of them shifts frequency while it is up.
    //
    // The symbol clock has to have been found as well. Levels alone are a shape in a histogram,
    // and a voice waveform wandering over its own passband produces those by accident; a keyed
    // carrier that has levels *and* a clock is being keyed.
    let shifted = waveform.symbol_rate_hz.is_some()
        && waveform.frequency_spread_hz > band.bandwidth_hz * MIN_SHIFT_FRACTION;
    if shifted {
        match waveform.frequency_levels {
            4 => return settle(Modulation::Fsk4, firmness(band.snr_db, SNR_FLOOR_DB, 15.0)),
            2 => return settle(Modulation::Fsk2, firmness(band.snr_db, SNR_FLOOR_DB, 15.0)),
            _ => {}
        }
    }

    if waveform.duty < KEYED_DUTY && waveform.on_off_db > KEYED_DEPTH_DB {
        return settle(
            Modulation::Ook,
            firmness(waveform.on_off_db, KEYED_DEPTH_DB, 20.0),
        );
    }

    if waveform.envelope_variation > AMPLITUDE_MODULATED {
        if band.carrier_db > CARRIER_LINE_DB {
            return settle(
                Modulation::Am,
                firmness(band.carrier_db, CARRIER_LINE_DB, 12.0),
            );
        }
        if band.skew.abs() > SIDEBAND_SKEW {
            return Verdict {
                sideband: sideband_of(band),
                ..settle(
                    Modulation::Ssb,
                    firmness(band.skew.abs(), SIDEBAND_SKEW, 0.6),
                )
            };
        }
        // Amplitude carries the message and no carrier survives to say where the middle is:
        // suppressed-carrier double sideband, which is AM as far as anything downstream of here
        // is concerned. Reported at the lower confidence the missing carrier deserves.
        return settle(Modulation::Am, 0.0);
    }

    // A narrow band with nothing moving in it.
    if band.bandwidth_hz < CARRIER_BANDWIDTH_HZ && waveform.frequency_spread_hz < CARRIER_SPREAD_HZ
    {
        return settle(Modulation::Carrier, 1.0);
    }

    // Frequency modulation before the phase tests, and not after: an angle-modulated carrier
    // rings in every power of itself — a tone-modulated one especially, its spectrum being
    // discrete lines to begin with — so testing for phase modulation first calls half the FM on
    // the air BPSK. What separates them is that the frequency is *moving*, over a range no phase
    // modulation produces once each sample is weighted by how long it dwells.
    if waveform.frequency_spread_hz > FM_SPREAD_HZ {
        return settle(Modulation::Fm, firmness(band.snr_db, SNR_FLOOR_DB, 20.0));
    }

    if waveform.quartic_line_db > PHASE_LINE_DB
        && waveform.quartic_line_db > waveform.square_line_db
    {
        return settle(
            Modulation::Psk4,
            firmness(waveform.quartic_line_db, PHASE_LINE_DB, 15.0),
        );
    }
    if waveform.square_line_db > PHASE_LINE_DB {
        return settle(
            Modulation::Psk2,
            firmness(waveform.square_line_db, PHASE_LINE_DB, 15.0),
        );
    }

    if band.flatness > FLAT_SPECTRUM {
        return settle(
            Modulation::NoiseLike,
            firmness(band.flatness, FLAT_SPECTRUM, 0.3),
        );
    }
    settle(Modulation::Unknown, 0.0)
}

/// Which side of the tuned frequency the band sits on, when it sits wholly on one.
fn sideband_of(band: &Band) -> Option<Sideband> {
    if band.center_hz.abs() < band.bandwidth_hz * 0.3 {
        return None;
    }
    Some(if band.center_hz > 0.0 {
        Sideband::Usb
    } else {
        Sideband::Lsb
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(bandwidth_hz: f64) -> Band {
        Band {
            center_hz: 0.0,
            bandwidth_hz,
            snr_db: 25.0,
            carrier_db: 3.0,
            flatness: 0.3,
            skew: 0.0,
            peak_hz: 0.0,
        }
    }

    fn steady() -> Waveform {
        Waveform {
            duty: 1.0,
            on_off_db: 1.0,
            ..Waveform::default()
        }
    }

    #[test]
    fn four_frequency_levels_are_four_level_keying() {
        let w = Waveform {
            frequency_levels: 4,
            deviation_hz: 1_944.0,
            frequency_spread_hz: 1_400.0,
            symbol_rate_hz: Some(4_800.0),
            ..steady()
        };
        let verdict = classify(&band(12_500.0), &w);
        assert_eq!(verdict.modulation, Modulation::Fsk4);
        assert!(verdict.confidence > 0.7, "{}", verdict.confidence);
    }

    #[test]
    fn a_deeply_keyed_carrier_is_on_off_keying() {
        let w = Waveform {
            duty: 0.4,
            on_off_db: 40.0,
            frequency_levels: 1,
            ..Waveform::default()
        };
        assert_eq!(classify(&band(20_000.0), &w).modulation, Modulation::Ook);
    }

    #[test]
    fn amplitude_over_a_carrier_is_am() {
        let w = Waveform {
            envelope_variation: 0.4,
            ..steady()
        };
        let mut b = band(9_000.0);
        b.carrier_db = 25.0;
        assert_eq!(classify(&b, &w).modulation, Modulation::Am);
    }

    #[test]
    fn a_lopsided_band_with_no_carrier_is_sideband() {
        let w = Waveform {
            envelope_variation: 0.5,
            ..steady()
        };
        let mut b = band(2_700.0);
        b.skew = 0.9;
        b.center_hz = 1_400.0;
        let verdict = classify(&b, &w);
        assert_eq!(verdict.modulation, Modulation::Ssb);
        assert_eq!(verdict.sideband, Some(Sideband::Usb));
    }

    #[test]
    fn a_bare_narrow_line_is_a_carrier_not_bpsk() {
        let w = Waveform {
            frequency_levels: 1,
            frequency_spread_hz: 4.0,
            square_line_db: 40.0,
            quartic_line_db: 38.0,
            ..steady()
        };
        assert_eq!(classify(&band(400.0), &w).modulation, Modulation::Carrier);
    }

    #[test]
    fn a_wide_frequency_continuum_is_analog_fm() {
        let w = Waveform {
            frequency_levels: 1,
            frequency_spread_hz: 2_500.0,
            ..steady()
        };
        assert_eq!(classify(&band(12_500.0), &w).modulation, Modulation::Fm);
    }

    /// A weak signal gets the same verdict and less of the operator's trust.
    #[test]
    fn confidence_follows_the_signal_to_noise_ratio() {
        let w = Waveform {
            frequency_levels: 2,
            frequency_spread_hz: 2_000.0,
            symbol_rate_hz: Some(9_600.0),
            ..steady()
        };
        let mut weak = band(12_500.0);
        weak.snr_db = 4.0;
        let strong = band(12_500.0);
        assert_eq!(classify(&weak, &w).modulation, Modulation::Fsk2);
        assert!(classify(&weak, &w).confidence < classify(&strong, &w).confidence);
    }
}
