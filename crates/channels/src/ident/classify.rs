use sdrmm_wire::{Modulation, Sideband};

use super::{detect::Band, features::Waveform};

const KEYED_DUTY: f32 = 0.9;
const KEYED_DEPTH_DB: f32 = 15.0;
const AMPLITUDE_MODULATED: f32 = 0.18;
const CARRIER_LINE_DB: f32 = 12.0;
const SIDEBAND_SKEW: f32 = 0.45;
const CARRIER_BANDWIDTH_HZ: f64 = 2_500.0;
const CARRIER_SPREAD_HZ: f64 = 100.0;
const PHASE_LINE_DB: f32 = 10.0;
const FLAT_SPECTRUM: f32 = 0.55;
const FM_SPREAD_HZ: f64 = 200.0;
const MIN_SHIFT_FRACTION: f64 = 0.02;
const LEVEL_SEPARATION: f64 = 0.7;
const LEVEL_VALLEY: f32 = 0.5;
const NO_CLOCK_FIRMNESS: f32 = 0.5;
const SNR_FLOOR_DB: f32 = 3.0;
const SNR_TRUSTED_DB: f32 = 18.0;

pub(crate) struct Verdict {
    pub(crate) modulation: Modulation,
    pub(crate) confidence: f32,
    pub(crate) sideband: Option<Sideband>,
}

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

    let modulated = (waveform.envelope_variation.powi(2) - waveform.noise_variation.powi(2))
        .max(0.0)
        .sqrt();

    let separated = waveform.deviation_hz >= waveform.frequency_spread_hz * LEVEL_SEPARATION
        && waveform.level_valley <= LEVEL_VALLEY
        && modulated <= AMPLITUDE_MODULATED;
    let shifted = waveform.frequency_spread_hz > band.bandwidth_hz * MIN_SHIFT_FRACTION
        && (waveform.symbol_rate_hz.is_some() || separated);
    if shifted {
        let firm = firmness(band.snr_db, SNR_FLOOR_DB, 15.0)
            * if waveform.symbol_rate_hz.is_some() {
                1.0
            } else {
                NO_CLOCK_FIRMNESS
            };
        match waveform.frequency_levels {
            4 => return settle(Modulation::Fsk4, firm),
            2 => return settle(Modulation::Fsk2, firm),
            _ => {}
        }
    }

    if waveform.duty < KEYED_DUTY && waveform.on_off_db > KEYED_DEPTH_DB {
        return settle(
            Modulation::Ook,
            firmness(waveform.on_off_db, KEYED_DEPTH_DB, 20.0),
        );
    }

    if modulated > AMPLITUDE_MODULATED {
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
        return settle(Modulation::Am, 0.0);
    }

    if band.bandwidth_hz < CARRIER_BANDWIDTH_HZ && waveform.frequency_spread_hz < CARRIER_SPREAD_HZ
    {
        return settle(Modulation::Carrier, 1.0);
    }

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
    fn a_shift_is_not_amplitude_modulated_by_the_noise_on_it() {
        let w = Waveform {
            envelope_variation: 0.22,
            noise_variation: 0.24,
            frequency_levels: 2,
            frequency_spread_hz: 4_800.0,
            deviation_hz: 4_500.0,
            level_valley: 0.1,
            ..steady()
        };
        let mut b = band(12_500.0);
        b.snr_db = 16.0;
        b.carrier_db = 25.0;
        assert_eq!(classify(&b, &w).modulation, Modulation::Fsk2);
    }

    #[test]
    fn amplitude_over_the_noise_is_still_amplitude_modulation() {
        let w = Waveform {
            envelope_variation: 0.38,
            noise_variation: 0.16,
            ..steady()
        };
        let mut b = band(9_000.0);
        b.snr_db = 20.0;
        b.carrier_db = 25.0;
        assert_eq!(classify(&b, &w).modulation, Modulation::Am);
    }

    #[test]
    fn bumps_inside_a_continuum_are_not_a_shift() {
        let w = Waveform {
            frequency_levels: 2,
            frequency_spread_hz: 2_000.0,
            deviation_hz: 600.0,
            level_valley: 0.9,
            ..steady()
        };
        assert_eq!(classify(&band(12_500.0), &w).modulation, Modulation::Fm);
    }

    #[test]
    fn a_clockless_shift_is_reported_less_confidently_than_a_clocked_one() {
        let w = Waveform {
            frequency_levels: 2,
            frequency_spread_hz: 4_800.0,
            deviation_hz: 4_500.0,
            level_valley: 0.1,
            ..steady()
        };
        let clocked = Waveform {
            symbol_rate_hz: Some(1_200.0),
            ..w
        };
        let b = band(12_500.0);
        assert_eq!(classify(&b, &w).modulation, Modulation::Fsk2);
        assert!(classify(&b, &w).confidence < classify(&b, &clocked).confidence);
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
