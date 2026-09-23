use num_complex::Complex;
use sdrmm_dsp::Ddc;
use sdrmm_wire::{
    ChannelParams, ChannelSettings, DecoderEvent, IdentParams, IdentReport, IdentSignal,
    MAX_IDENT_BANDWIDTH_HZ, MIN_IDENT_BANDWIDTH_HZ, Modulation, Squelch,
};

use crate::{ChannelCtx, ChannelOutputs, ChannelRx, IdentChannel};

pub const RATE_HZ: f64 = 240_000.0;
const SECONDS: f64 = 3.0;
const INTERVAL_MS: u32 = 500;
const BLOCK: usize = 65_536;
const MIN_SECONDS: f64 = 1.3;
const RECORDED_SPAN: f64 = 0.8;

pub enum Expect {
    Named {
        type_id: &'static str,
        families: &'static [Modulation],
    },
    Family(&'static [Modulation]),
    Beyond(&'static str),
}

pub struct Fixture {
    pub stem: &'static str,
    pub rate: f64,
    pub offset_hz: f64,
    pub dial_hz: f64,
    pub expect: Expect,
}

impl Fixture {
    #[must_use]
    pub fn file(&self) -> String {
        format!("{}.sigmf-data", self.stem)
    }

    #[must_use]
    pub fn expected_family(&self) -> Option<Modulation> {
        match &self.expect {
            Expect::Named { families, .. } | Expect::Family(families) => families.first().copied(),
            Expect::Beyond(_) => None,
        }
    }
}

const fn named(
    stem: &'static str,
    rate: f64,
    offset_hz: f64,
    dial_hz: f64,
    type_id: &'static str,
    families: &'static [Modulation],
) -> Fixture {
    Fixture {
        stem,
        rate,
        offset_hz,
        dial_hz,
        expect: Expect::Named { type_id, families },
    }
}

const fn family(
    stem: &'static str,
    rate: f64,
    offset_hz: f64,
    dial_hz: f64,
    families: &'static [Modulation],
) -> Fixture {
    Fixture {
        stem,
        rate,
        offset_hz,
        dial_hz,
        expect: Expect::Family(families),
    }
}

const fn beyond(
    stem: &'static str,
    rate: f64,
    offset_hz: f64,
    dial_hz: f64,
    why: &'static str,
) -> Fixture {
    Fixture {
        stem,
        rate,
        offset_hz,
        dial_hz,
        expect: Expect::Beyond(why),
    }
}

const FOUR_LEVEL: &[Modulation] = &[Modulation::Fsk4];
const TWO_LEVEL: &[Modulation] = &[Modulation::Fsk2];
const TWO_LEVEL_OR_IDLE: &[Modulation] = &[Modulation::Fsk2, Modulation::Carrier];
const TONES_OVER_FM: &[Modulation] = &[Modulation::Fm, Modulation::Fsk2, Modulation::Fsk4];
const KEYED: &[Modulation] = &[Modulation::Ook, Modulation::Carrier];
const TONES_ON_FM: &[Modulation] = &[Modulation::Fm, Modulation::Fsk2];
const ANY: &[Modulation] = &[];
const PICTURE_TONES: &[Modulation] = &[
    Modulation::Fm,
    Modulation::Fsk2,
    Modulation::Fsk4,
    Modulation::Fsk8,
    Modulation::Ssb,
    Modulation::Unknown,
];

pub const FIXTURES: &[Fixture] = &[
    named(
        "pocsag_1200_240k",
        240_000.0,
        50_000.0,
        466_075_000.0,
        "pocsag",
        TWO_LEVEL,
    ),
    family(
        "flex_1600_2_240k",
        240_000.0,
        30_000.0,
        929_662_500.0,
        TWO_LEVEL_OR_IDLE,
    ),
    named(
        "ermes_alpha_240k",
        240_000.0,
        -30_000.0,
        169_650_000.0,
        "ermes",
        FOUR_LEVEL,
    ),
    named(
        "ais_position_240k",
        240_000.0,
        25_000.0,
        161_975_000.0,
        "ais",
        TWO_LEVEL,
    ),
    named(
        "ais_position_pre_cpm_240k",
        240_000.0,
        25_000.0,
        161_975_000.0,
        "ais",
        TWO_LEVEL,
    ),
    named(
        "aprs_afsk1200_240k",
        240_000.0,
        -40_000.0,
        144_800_000.0,
        "aprs",
        TONES_ON_FM,
    ),
    named(
        "acars_downlink_240k",
        240_000.0,
        -40_000.0,
        131_550_000.0,
        "acars",
        &[Modulation::Am, Modulation::Ook],
    ),
    named(
        "rtty_45_170_48k",
        48_000.0,
        5_000.0,
        14_080_000.0,
        "rtty",
        TWO_LEVEL,
    ),
    named(
        "navtex_518_48k",
        48_000.0,
        3_000.0,
        518_000.0,
        "navtex",
        TWO_LEVEL,
    ),
    named(
        "morse_20wpm_48k",
        48_000.0,
        -5_000.0,
        14_050_000.0,
        "morse",
        KEYED,
    ),
    named(
        "cw_skimmer_dual_48k",
        48_000.0,
        -3_500.0,
        14_030_000.0,
        "morse",
        KEYED,
    ),
    named(
        "dmr_call_48k",
        48_000.0,
        0.0,
        446_006_250.0,
        "dmr",
        FOUR_LEVEL,
    ),
    named(
        "dmr_capacity_plus_48k",
        48_000.0,
        0.0,
        460_800_000.0,
        "dmr",
        FOUR_LEVEL,
    ),
    named(
        "dmr_tier3_control_48k",
        48_000.0,
        0.0,
        460_137_500.0,
        "dmr",
        FOUR_LEVEL,
    ),
    named(
        "nxdn_addressed_48k",
        48_000.0,
        0.0,
        451_200_000.0,
        "nxdn",
        FOUR_LEVEL,
    ),
    named(
        "ysf_callsigns_48k",
        48_000.0,
        0.0,
        438_500_000.0,
        "ysf",
        FOUR_LEVEL,
    ),
    beyond(
        "rds_station_960k",
        960_000.0,
        200_000.0,
        95_500_000.0,
        "a single tone at broadcast deviation reads as four discrete frequency levels",
    ),
    named(
        "subghz_ev1527_500k",
        500_000.0,
        100_000.0,
        433_920_000.0,
        "subghz",
        &[Modulation::Ook],
    ),
    named(
        "dcf77_2026_2k",
        2_000.0,
        0.0,
        77_500.0,
        "radio_clock",
        &[Modulation::Carrier, Modulation::Ook, Modulation::Am],
    ),
    named(
        "adsb_squitters_2m",
        2_000_000.0,
        0.0,
        1_090_000_000.0,
        "adsb",
        ANY,
    ),
    named(
        "adsb_offair_2m",
        2_000_000.0,
        0.0,
        1_090_000_000.0,
        "adsb",
        ANY,
    ),
    named(
        "dect_base_2m304",
        2_304_000.0,
        0.0,
        1_897_344_000.0,
        "dect",
        ANY,
    ),
    named(
        "sstv_robot36_48k",
        48_000.0,
        4_000.0,
        14_230_000.0,
        "sstv",
        PICTURE_TONES,
    ),
    named(
        "selcall_ccir1_48k",
        48_000.0,
        5_000.0,
        156_800_000.0,
        "selcall",
        TONES_OVER_FM,
    ),
    named(
        "selcall_zvei1_48k",
        48_000.0,
        -5_000.0,
        156_800_000.0,
        "selcall",
        TONES_OVER_FM,
    ),
    named("freedv_1600_8k", 8_000.0, 0.0, 14_236_000.0, "freedv", ANY),
    beyond(
        "gps_l1_ca_prn7_2m048",
        2_048_000.0,
        0.0,
        1_575_420_000.0,
        "spread spectrum below the noise floor leaves nothing to detect",
    ),
    beyond(
        "atv_ccir625_2m4",
        2_400_000.0,
        200_000.0,
        1_255_000_000.0,
        "five megahertz of vision is far wider than the identifier's span",
    ),
    beyond(
        "ft8_20m_busy_12k",
        12_000.0,
        0.0,
        14_074_000.0,
        "a slot crowded with 50 Hz signals lies below the detector's resolution",
    ),
];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub reports: usize,
    pub modulation: Modulation,
    pub type_id: Option<String>,
    pub name: Option<String>,
    pub confirmed: bool,
    pub signals: usize,
    pub details: Vec<String>,
}

#[must_use]
pub fn samples(bytes: &[u8]) -> Vec<Complex<f32>> {
    bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|s| {
            Complex::new(
                f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                f32::from_le_bytes([s[4], s[5], s[6], s[7]]),
            )
        })
        .collect()
}

#[must_use]
pub fn identify(fixture: &Fixture, iq: &[Complex<f32>]) -> Outcome {
    let take = ((SECONDS * fixture.rate) as usize).min(iq.len());
    let mut centred = centre(fixture, &iq[..take]);
    let least = (MIN_SECONDS * RATE_HZ) as usize;
    if centred.len() < least {
        centred.resize(least, Complex::default());
    }
    let reports = run(fixture, &centred);
    summarise(&reports)
}

fn centre(fixture: &Fixture, iq: &[Complex<f32>]) -> Vec<Complex<f32>> {
    if fixture.rate < RATE_HZ {
        let mut wide = super::resample(iq, fixture.rate, RATE_HZ);
        super::shift(&mut wide, -fixture.offset_hz, RATE_HZ);
        return wide;
    }
    let mut ddc = Ddc::new(fixture.rate, RATE_HZ, fixture.offset_hz)
        .unwrap_or_else(|e| panic!("{}: {e}", fixture.stem));
    let mut out = Vec::with_capacity((iq.len() as f64 * RATE_HZ / fixture.rate) as usize + 1);
    ddc.process(iq, &mut out);
    out
}

fn run(fixture: &Fixture, iq: &[Complex<f32>]) -> Vec<IdentReport> {
    let settings = ChannelSettings {
        frequency_hz: fixture.dial_hz,
        squelch: Squelch::Off,
        params: ChannelParams::Ident(IdentParams {
            interval_ms: INTERVAL_MS,
            bandwidth_hz: (fixture.rate * RECORDED_SPAN)
                .clamp(MIN_IDENT_BANDWIDTH_HZ, MAX_IDENT_BANDWIDTH_HZ),
            ..IdentParams::default()
        }),
        blanker: Default::default(),
    };
    let mut channel = IdentChannel::new(
        ChannelCtx {
            input_rate: RATE_HZ,
        },
        settings,
    )
    .unwrap_or_else(|e| panic!("ident channel: {e}"));
    let mut out = ChannelOutputs::default();
    let mut reports = Vec::new();
    for block in iq.chunks(BLOCK) {
        out.reset();
        channel.process(block, &mut out);
        reports.extend(out.events.drain(..).filter_map(|event| match event {
            DecoderEvent::Ident(report) => Some(report),
            _ => None,
        }));
    }
    reports
}

fn nearest(report: &IdentReport) -> Option<&IdentSignal> {
    report.signals.iter().min_by(|a, b| {
        a.center_offset_hz
            .abs()
            .total_cmp(&b.center_offset_hz.abs())
    })
}

fn weighted<T: Clone + PartialEq>(items: impl Iterator<Item = (T, f32)>) -> Option<T> {
    let mut tallies: Vec<(T, f32)> = Vec::new();
    for (item, weight) in items {
        match tallies.iter_mut().find(|(seen, _)| *seen == item) {
            Some((_, total)) => *total += weight,
            None => tallies.push((item, weight)),
        }
    }
    let mut best: Option<(T, f32)> = None;
    for (item, total) in tallies {
        if best.as_ref().is_none_or(|(_, top)| total > *top) {
            best = Some((item, total));
        }
    }
    best.map(|(item, _)| item)
}

fn weight(signal: &IdentSignal) -> f32 {
    signal
        .best()
        .map_or(0.5, |m| if m.confirmed { 4.0 } else { m.score })
}

fn tally<'a>(found: impl Iterator<Item = &'a &'a IdentSignal>) -> Option<(Option<String>, String)> {
    weighted(found.filter_map(|s| {
        s.best()
            .map(|m| ((m.type_id.clone(), m.name.clone()), weight(s)))
    }))
}

fn summarise(reports: &[IdentReport]) -> Outcome {
    let found: Vec<&IdentSignal> = reports.iter().filter_map(nearest).collect();
    let named = |s: &&&IdentSignal| s.best().is_some_and(|m| m.type_id.is_some());
    let best = tally(found.iter().filter(named)).or_else(|| tally(found.iter()));
    let chosen: Vec<&&IdentSignal> = found
        .iter()
        .filter(|s| {
            best.as_ref()
                .is_none_or(|(type_id, _)| s.best().map(|m| &m.type_id) == Some(type_id))
        })
        .collect();
    let modulation = weighted(chosen.iter().map(|s| (s.modulation, weight(s))))
        .or_else(|| weighted(found.iter().map(|s| (s.modulation, weight(s)))))
        .unwrap_or(Modulation::None);
    Outcome {
        reports: reports.len(),
        modulation,
        type_id: best.as_ref().and_then(|(type_id, _)| type_id.clone()),
        name: best.map(|(_, name)| name),
        confirmed: found.iter().any(|s| s.best().is_some_and(|m| m.confirmed)),
        signals: reports.iter().map(|r| r.signals.len()).max().unwrap_or(0),
        details: reports.iter().map(detail).collect(),
    }
}

fn detail(report: &IdentReport) -> String {
    let Some(signal) = nearest(report) else {
        return format!("nothing, loudest bin {:.1} dB", report.snr_db);
    };
    let f = &signal.features;
    format!(
        "{} {:.0}% at {:+.0} Hz bw {:.0} snr {:.1} baud {:?} dev {:?} burst {:?}/{:?} ofdm {:?} env {:.2} duty {:.2} depth {:.1} carrier {:.1} flat {:.2} levels {} spread {:.0} sq {:.1} q4 {:.1} -> {:?}",
        signal.modulation.label(),
        signal.confidence * 100.0,
        signal.center_offset_hz,
        signal.bandwidth_hz,
        signal.snr_db,
        signal.symbol_rate_hz.map(f64::round),
        signal.deviation_hz.map(f64::round),
        signal.burst_ms,
        signal.burst_period_ms,
        signal.ofdm_symbol_us,
        f.envelope_variation,
        f.duty,
        f.keying_depth_db,
        f.carrier_db,
        f.spectral_flatness,
        f.frequency_levels,
        f.frequency_spread_hz,
        f.square_line_db,
        f.quartic_line_db,
        signal
            .candidates
            .iter()
            .map(|m| format!(
                "{} {:.2}{}",
                m.name,
                m.score,
                if m.confirmed { "✓" } else { "" }
            ))
            .collect::<Vec<_>>()
    )
}

pub fn judge(fixture: &Fixture, outcome: &Outcome) -> Result<(), String> {
    let describe = || {
        format!(
            "{}: got {} named {:?} ({:?}) over {} reports",
            fixture.stem,
            outcome.modulation.label(),
            outcome.name,
            outcome.type_id,
            outcome.reports
        )
    };
    match &fixture.expect {
        Expect::Named { type_id, families } => {
            if !families.is_empty() && !families.contains(&outcome.modulation) {
                return Err(format!("{}, expected one of {families:?}", describe()));
            }
            if outcome.type_id.as_deref() != Some(type_id) {
                return Err(format!("{}, expected {type_id}", describe()));
            }
            Ok(())
        }
        Expect::Family(families) => {
            if families.contains(&outcome.modulation) {
                Ok(())
            } else {
                Err(format!("{}, expected one of {families:?}", describe()))
            }
        }
        Expect::Beyond(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::ProtocolMatch;

    use super::*;

    fn matched(name: &str, type_id: Option<&str>, score: f32) -> ProtocolMatch {
        ProtocolMatch {
            name: name.to_owned(),
            type_id: type_id.map(str::to_owned),
            score,
            confirmed: false,
            why: String::new(),
        }
    }

    fn window(modulation: Modulation, best: ProtocolMatch) -> IdentReport {
        IdentReport {
            snr_db: 40.0,
            signals: vec![IdentSignal {
                modulation,
                candidates: vec![best],
                ..IdentSignal::default()
            }],
        }
    }

    #[test]
    fn a_named_protocol_outvotes_a_generic_label_it_is_outscored_by() {
        let reports = [
            window(Modulation::Fsk2, matched("FLEX", Some("flex"), 0.70)),
            window(Modulation::Fsk2, matched("FLEX", Some("flex"), 1.00)),
            window(
                Modulation::Carrier,
                matched("Unmodulated carrier", None, 0.95),
            ),
            window(
                Modulation::Carrier,
                matched("Unmodulated carrier", None, 0.95),
            ),
            window(
                Modulation::Fm,
                matched("FM voice (narrowband)", Some("nfm"), 0.95),
            ),
        ];
        let outcome = summarise(&reports);
        assert_eq!(outcome.type_id.as_deref(), Some("flex"));
        assert_eq!(outcome.modulation, Modulation::Fsk2);
    }

    #[test]
    fn a_recording_that_names_nothing_keeps_its_generic_label() {
        let reports = [
            window(
                Modulation::Carrier,
                matched("Unmodulated carrier", None, 0.95),
            ),
            window(
                Modulation::Carrier,
                matched("Unmodulated carrier", None, 0.95),
            ),
        ];
        let outcome = summarise(&reports);
        assert_eq!(outcome.type_id, None);
        assert_eq!(outcome.name.as_deref(), Some("Unmodulated carrier"));
        assert_eq!(outcome.modulation, Modulation::Carrier);
    }
}
