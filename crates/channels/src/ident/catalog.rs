use sdrmm_wire::{Modulation, ProtocolMatch};

use super::{detect::Band, features::Waveform};
use crate::dv::MODE_SIGNATURES;

const MIN_SCORE: f32 = 0.35;
const MAX_CANDIDATES: usize = 5;
const SKIRT: f64 = 1.5;
const MIN_SKIRT_FRACTION: f64 = 0.05;
const UNMEASURED: f32 = 0.75;

const WEIGHT_BANDWIDTH: f32 = 1.0;
const WEIGHT_SYMBOL_RATE: f32 = 2.0;
const WEIGHT_DEVIATION: f32 = 1.5;

#[derive(Clone, Copy)]
struct Range {
    low: f64,
    high: f64,
}

const fn range(low: f64, high: f64) -> Range {
    Range { low, high }
}

const fn about(center: f64, fraction: f64) -> Range {
    Range {
        low: center * (1.0 - fraction),
        high: center * (1.0 + fraction),
    }
}

impl Range {
    fn score(self, value: f64) -> f32 {
        if value >= self.low && value <= self.high {
            return 1.0;
        }
        let width = (self.high - self.low)
            .max(self.high * MIN_SKIRT_FRACTION)
            .max(1.0);
        let distance = if value < self.low {
            self.low - value
        } else {
            value - self.high
        };
        (1.0 - distance / (width * SKIRT)).clamp(0.0, 1.0) as f32
    }
}

#[derive(Clone, Copy)]
struct Signature {
    name: &'static str,
    type_id: Option<&'static str>,
    modulations: &'static [Modulation],
    bandwidth_hz: Range,
    symbol_rate_hz: Option<Range>,
    deviation_hz: Option<Range>,
    why: &'static str,
}

const SIGNATURES: &[Signature] = &[
    Signature {
        name: "FM broadcast",
        type_id: Some("wfm"),
        modulations: &[Modulation::Fm],
        bandwidth_hz: range(100_000.0, 220_000.0),
        symbol_rate_hz: None,
        deviation_hz: Some(range(15_000.0, 80_000.0)),
        why: "wideband FM at broadcast deviation",
    },
    Signature {
        name: "FM voice (narrowband)",
        type_id: Some("nfm"),
        modulations: &[Modulation::Fm],
        bandwidth_hz: range(6_000.0, 25_000.0),
        symbol_rate_hz: None,
        deviation_hz: Some(range(1_000.0, 5_000.0)),
        why: "channel-width FM with no symbol structure",
    },
    Signature {
        name: "AM voice",
        type_id: Some("am"),
        modulations: &[Modulation::Am],
        bandwidth_hz: range(3_000.0, 12_000.0),
        symbol_rate_hz: None,
        deviation_hz: None,
        why: "amplitude modulation over a carrier",
    },
    Signature {
        name: "SSB voice",
        type_id: Some("ssb"),
        modulations: &[Modulation::Ssb],
        bandwidth_hz: range(1_500.0, 4_000.0),
        symbol_rate_hz: None,
        deviation_hz: None,
        why: "one sideband, no carrier",
    },
    Signature {
        name: "Unmodulated carrier",
        type_id: None,
        modulations: &[Modulation::Carrier],
        bandwidth_hz: range(0.0, 2_500.0),
        symbol_rate_hz: None,
        deviation_hz: None,
        why: "a bare line with nothing on it",
    },
    Signature {
        name: "Morse (CW)",
        type_id: Some("morse"),
        modulations: &[Modulation::Ook, Modulation::Carrier],
        bandwidth_hz: range(0.0, 2_500.0),
        symbol_rate_hz: Some(range(4.0, 80.0)),
        deviation_hz: None,
        why: "a keyed carrier at hand speed",
    },
    Signature {
        name: "POCSAG (512 bd)",
        type_id: Some("pocsag"),
        modulations: &[Modulation::Fsk2],
        bandwidth_hz: range(6_000.0, 20_000.0),
        symbol_rate_hz: Some(about(512.0, 0.05)),
        deviation_hz: Some(range(3_000.0, 6_000.0)),
        why: "two-level keying at a pager rate and ±4.5 kHz",
    },
    Signature {
        name: "POCSAG (1200 bd)",
        type_id: Some("pocsag"),
        modulations: &[Modulation::Fsk2],
        bandwidth_hz: range(6_000.0, 20_000.0),
        symbol_rate_hz: Some(about(1_200.0, 0.05)),
        deviation_hz: Some(range(3_000.0, 6_000.0)),
        why: "two-level keying at a pager rate and ±4.5 kHz",
    },
    Signature {
        name: "POCSAG (2400 bd)",
        type_id: Some("pocsag"),
        modulations: &[Modulation::Fsk2],
        bandwidth_hz: range(6_000.0, 20_000.0),
        symbol_rate_hz: Some(about(2_400.0, 0.05)),
        deviation_hz: Some(range(3_000.0, 6_000.0)),
        why: "two-level keying at a pager rate and ±4.5 kHz",
    },
    Signature {
        name: "AIS",
        type_id: Some("ais"),
        modulations: &[Modulation::Fsk2],
        bandwidth_hz: range(12_000.0, 30_000.0),
        symbol_rate_hz: Some(about(9_600.0, 0.05)),
        deviation_hz: Some(range(1_800.0, 3_200.0)),
        why: "9600 baud GMSK in a 25 kHz maritime channel",
    },
    Signature {
        name: "ACARS",
        type_id: Some("acars"),
        modulations: &[Modulation::Am, Modulation::Ook],
        bandwidth_hz: range(2_400.0, 9_000.0),
        symbol_rate_hz: Some(about(2_400.0, 0.06)),
        deviation_hz: None,
        why: "2400 baud minimum-shift keying carried on an AM carrier",
    },
    Signature {
        name: "RTTY",
        type_id: Some("rtty"),
        modulations: &[Modulation::Fsk2],
        bandwidth_hz: range(100.0, 2_500.0),
        symbol_rate_hz: Some(range(40.0, 110.0)),
        deviation_hz: Some(range(40.0, 500.0)),
        why: "a slow two-tone shift, teleprinter speed",
    },
    Signature {
        name: "NAVTEX (SITOR-B)",
        type_id: Some("navtex"),
        modulations: &[Modulation::Fsk2],
        bandwidth_hz: range(100.0, 2_500.0),
        symbol_rate_hz: Some(about(100.0, 0.08)),
        deviation_hz: Some(range(50.0, 200.0)),
        why: "100 baud at a 170 Hz shift",
    },
    Signature {
        name: "APRS / AX.25 (AFSK over FM)",
        type_id: Some("aprs"),
        modulations: &[Modulation::Fm, Modulation::Fsk2],
        bandwidth_hz: range(6_000.0, 20_000.0),
        symbol_rate_hz: Some(range(1_000.0, 2_400.0)),
        deviation_hz: Some(range(1_500.0, 5_000.0)),
        why: "audio tones inside an FM channel, at packet speed",
    },
    Signature {
        name: "Sub-GHz remote (OOK)",
        type_id: Some("subghz"),
        modulations: &[Modulation::Ook],
        bandwidth_hz: range(2_000.0, 160_000.0),
        symbol_rate_hz: Some(range(300.0, 30_000.0)),
        deviation_hz: None,
        why: "a keyed carrier at remote-control speed",
    },
    Signature {
        name: "Sub-GHz telemetry (2-FSK)",
        type_id: Some("subghz"),
        modulations: &[Modulation::Fsk2],
        bandwidth_hz: range(15_000.0, 160_000.0),
        symbol_rate_hz: Some(range(600.0, 100_000.0)),
        deviation_hz: Some(range(8_000.0, 80_000.0)),
        why: "a wide two-level shift, sensor-radio speed",
    },
];

fn dv_signatures() -> impl Iterator<Item = Signature> {
    MODE_SIGNATURES.iter().map(|mode| Signature {
        name: mode.name,
        type_id: Some(mode.type_id),
        modulations: if mode.params.mapping().m() == 4 {
            &[Modulation::Fsk4]
        } else {
            &[Modulation::Fsk2]
        },
        bandwidth_hz: about(mode.bandwidth_hz, 0.35),
        symbol_rate_hz: Some(about(mode.baud, 0.05)),
        deviation_hz: Some(about(mode.deviation_hz, 0.3)),
        why: "matches the mode's channel width, symbol rate and deviation",
    })
}

pub(crate) fn candidates(
    modulation: Modulation,
    band: &Band,
    waveform: &Waveform,
) -> Vec<ProtocolMatch> {
    let mut found: Vec<ProtocolMatch> = SIGNATURES
        .iter()
        .copied()
        .chain(dv_signatures())
        .filter(|signature| signature.modulations.contains(&modulation))
        .filter_map(|signature| score(&signature, band, waveform))
        .collect();
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    found.truncate(MAX_CANDIDATES);
    found
}

fn score(signature: &Signature, band: &Band, waveform: &Waveform) -> Option<ProtocolMatch> {
    let mut total = signature.bandwidth_hz.score(band.bandwidth_hz) * WEIGHT_BANDWIDTH;
    let mut weight = WEIGHT_BANDWIDTH;

    if let Some(expected) = signature.symbol_rate_hz {
        total += WEIGHT_SYMBOL_RATE
            * match waveform.symbol_rate_hz {
                Some(measured) => expected.score(measured),
                None => UNMEASURED,
            };
        weight += WEIGHT_SYMBOL_RATE;
    }
    if let Some(expected) = signature.deviation_hz {
        total += WEIGHT_DEVIATION
            * if waveform.deviation_hz > 0.0 {
                expected.score(waveform.deviation_hz)
            } else {
                UNMEASURED
            };
        weight += WEIGHT_DEVIATION;
    }

    let score = total / weight;
    (score >= MIN_SCORE).then(|| ProtocolMatch {
        name: signature.name.to_owned(),
        type_id: signature.type_id.map(str::to_owned),
        score,
        confirmed: false,
        why: signature.why.to_owned(),
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

    fn keyed(symbol_rate_hz: f64, deviation_hz: f64) -> Waveform {
        Waveform {
            duty: 1.0,
            symbol_rate_hz: Some(symbol_rate_hz),
            deviation_hz,
            ..Waveform::default()
        }
    }

    fn names(found: &[ProtocolMatch]) -> Vec<&str> {
        found.iter().map(|m| m.name.as_str()).collect()
    }

    #[test]
    fn a_pager_shift_finds_pocsag_at_its_own_baud() {
        let found = candidates(Modulation::Fsk2, &band(12_500.0), &keyed(1_200.0, 4_500.0));
        assert_eq!(names(&found).first(), Some(&"POCSAG (1200 bd)"));
    }

    #[test]
    fn a_maritime_carrier_finds_ais() {
        let found = candidates(Modulation::Fsk2, &band(25_000.0), &keyed(9_600.0, 2_400.0));
        assert_eq!(names(&found).first(), Some(&"AIS"));
    }

    #[test]
    fn the_c4fm_family_comes_back_as_a_shortlist() {
        let found = candidates(Modulation::Fsk4, &band(12_500.0), &keyed(4_800.0, 1_944.0));
        let names = names(&found);
        for mode in ["DMR", "P25 Phase 1", "System Fusion"] {
            assert!(names.contains(&mode), "{mode} missing from {names:?}");
        }
        assert!(found.iter().all(|m| !m.confirmed));
    }

    #[test]
    fn a_broadcast_signal_is_wideband_fm_and_not_a_repeater() {
        let waveform = Waveform {
            deviation_hz: 45_000.0,
            ..Waveform::default()
        };
        let found = candidates(Modulation::Fm, &band(180_000.0), &waveform);
        assert_eq!(names(&found).first(), Some(&"FM broadcast"));
    }

    #[test]
    fn nothing_is_offered_for_a_family_no_protocol_here_uses() {
        assert!(candidates(Modulation::Psk4, &band(20_000.0), &keyed(9_600.0, 0.0)).is_empty());
    }
}
