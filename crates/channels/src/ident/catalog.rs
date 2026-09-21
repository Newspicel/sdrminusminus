use sdrmm_wire::{Modulation, ProtocolMatch};

use super::{detect::Band, features::Waveform};
use crate::dv::MODE_SIGNATURES;

const MIN_SCORE: f32 = 0.35;
const MAX_CANDIDATES: usize = 5;
const SKIRT: f64 = 1.5;
const MIN_SKIRT_FRACTION: f64 = 0.05;
const UNMEASURED: f32 = 0.75;
const UNKNOWN_DIAL: f32 = 0.8;
const GENERIC_WITH_DIAL: f32 = 0.95;
const OUT_OF_ALLOCATION: f32 = 0.4;

const WEIGHT_BANDWIDTH: f32 = 1.0;
const WEIGHT_SYMBOL_RATE: f32 = 2.0;
const WEIGHT_DEVIATION: f32 = 1.5;
const WEIGHT_BURST: f32 = 1.5;
const WEIGHT_SYMBOL_LENGTH: f32 = 2.0;

const ALLOCATION_SKIRT_FRACTION: f64 = 0.05;
const ALLOCATION_MIN_SKIRT_HZ: f64 = 1_000.0;

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

const fn khz(low: f64, high: f64) -> Range {
    range(low * 1e3, high * 1e3)
}

const fn mhz(low: f64, high: f64) -> Range {
    range(low * 1e6, high * 1e6)
}

const fn spot_khz(center: f64, half: f64) -> Range {
    range((center - half) * 1e3, (center + half) * 1e3)
}

impl Range {
    fn score(self, value: f64) -> f32 {
        let width = (self.high - self.low)
            .max(self.high * MIN_SKIRT_FRACTION)
            .max(1.0);
        let edge = if value < self.low {
            self.low
        } else {
            self.high
        };
        self.score_within(value, (width * SKIRT).min(edge.max(1.0)))
    }

    fn score_within(self, value: f64, skirt: f64) -> f32 {
        if value >= self.low && value <= self.high {
            return 1.0;
        }
        let distance = if value < self.low {
            self.low - value
        } else {
            value - self.high
        };
        (1.0 - distance / skirt.max(1.0)).clamp(0.0, 1.0) as f32
    }

    fn allocation_score(self, value: f64) -> f32 {
        let skirt =
            ((self.high - self.low) * ALLOCATION_SKIRT_FRACTION).max(ALLOCATION_MIN_SKIRT_HZ);
        self.score_within(value, skirt)
    }
}

const HF: &[Range] = &[khz(150.0, 30_000.0)];
const FT8_SPOTS: &[Range] = &[
    spot_khz(1_840.0, 2.0),
    spot_khz(3_573.0, 2.0),
    spot_khz(7_074.0, 2.0),
    spot_khz(10_136.0, 2.0),
    spot_khz(14_074.0, 2.0),
    spot_khz(18_100.0, 2.0),
    spot_khz(21_074.0, 2.0),
    spot_khz(24_915.0, 2.0),
    spot_khz(28_074.0, 2.0),
    spot_khz(50_313.0, 2.0),
];
const FT4_SPOTS: &[Range] = &[
    spot_khz(3_575.0, 2.0),
    spot_khz(7_047.5, 2.0),
    spot_khz(10_140.0, 2.0),
    spot_khz(14_080.0, 2.0),
    spot_khz(18_104.0, 2.0),
    spot_khz(21_140.0, 2.0),
    spot_khz(24_919.0, 2.0),
    spot_khz(28_180.0, 2.0),
];
const WSPR_SPOTS: &[Range] = &[
    spot_khz(1_836.6, 1.0),
    spot_khz(3_568.6, 1.0),
    spot_khz(7_038.6, 1.0),
    spot_khz(10_138.7, 1.0),
    spot_khz(14_095.6, 1.0),
    spot_khz(18_104.6, 1.0),
    spot_khz(21_094.6, 1.0),
    spot_khz(24_924.6, 1.0),
    spot_khz(28_124.6, 1.0),
];
const PSK_SPOTS: &[Range] = &[
    spot_khz(3_580.0, 3.0),
    spot_khz(7_040.0, 3.0),
    spot_khz(7_070.0, 3.0),
    spot_khz(10_142.0, 3.0),
    spot_khz(14_070.0, 3.0),
    spot_khz(18_100.0, 3.0),
    spot_khz(21_070.0, 3.0),
    spot_khz(24_920.0, 3.0),
    spot_khz(28_120.0, 3.0),
];
const SSTV_SPOTS: &[Range] = &[
    spot_khz(3_730.0, 3.0),
    spot_khz(7_171.0, 3.0),
    spot_khz(14_230.0, 3.0),
    spot_khz(21_340.0, 3.0),
    spot_khz(28_680.0, 3.0),
    mhz(145.49, 145.51),
];
const FREEDV_SPOTS: &[Range] = &[
    spot_khz(7_177.0, 3.0),
    spot_khz(14_236.0, 3.0),
    spot_khz(21_313.0, 3.0),
    spot_khz(28_330.0, 3.0),
];
const LAND_MOBILE: &[Range] = &[mhz(136.0, 174.0), mhz(400.0, 470.0), mhz(769.0, 870.0)];
const AMATEUR_VU: &[Range] = &[mhz(144.0, 148.0), mhz(430.0, 450.0)];
const PMR446: &[Range] = &[mhz(446.0, 446.2)];
const MARINE_VHF: &[Range] = &[mhz(156.0, 162.05)];
const SELCALL_BANDS: &[Range] = &[mhz(136.0, 174.0), mhz(400.0, 470.0)];
const AIRBAND: &[Range] = &[mhz(118.0, 137.0)];
const FM_BROADCAST: &[Range] = &[mhz(76.0, 108.0)];
const DAB_BANDS: &[Range] = &[mhz(174.0, 240.0), mhz(1_452.0, 1_492.0)];
const DVBT_BANDS: &[Range] = &[mhz(174.0, 230.0), mhz(470.0, 862.0)];
const DRM_VHF: &[Range] = &[mhz(47.0, 108.0), mhz(174.0, 240.0)];
const PAGING: &[Range] = &[mhz(148.0, 175.0), mhz(426.0, 470.0), mhz(929.0, 932.0)];
const ERMES_BAND: &[Range] = &[mhz(169.4, 169.8)];
const AIS_CHANNELS: &[Range] = &[spot_khz(161_975.0, 12.5), spot_khz(162_025.0, 12.5)];
const ACARS_VHF: &[Range] = &[mhz(129.0, 137.0)];
const VDL2_CHANNELS: &[Range] = &[mhz(136.7, 137.0)];
const APRS_CHANNELS: &[Range] = &[mhz(144.3, 145.2)];
const ISM: &[Range] = &[
    mhz(314.5, 315.5),
    mhz(433.05, 434.79),
    mhz(863.0, 870.0),
    mhz(902.0, 928.0),
];
const ADSB: &[Range] = &[mhz(1_089.0, 1_091.0)];
const DECT_BANDS: &[Range] = &[mhz(1_880.0, 1_900.0), mhz(1_920.0, 1_930.0)];
const IRIDIUM_BAND: &[Range] = &[mhz(1_616.0, 1_626.5)];
const INMARSAT_BAND: &[Range] = &[mhz(1_525.0, 1_559.0)];
const DSC_VHF: &[Range] = &[spot_khz(156_525.0, 12.5)];
const DSC_HF: &[Range] = &[
    spot_khz(2_187.5, 1.0),
    spot_khz(4_207.5, 1.0),
    spot_khz(6_312.0, 1.0),
    spot_khz(8_414.5, 1.0),
    spot_khz(12_577.0, 1.0),
    spot_khz(16_804.5, 1.0),
];
const NAVTEX_CHANNELS: &[Range] = &[
    spot_khz(490.0, 1.0),
    spot_khz(518.0, 1.0),
    spot_khz(4_209.5, 1.0),
];
const HFDL_BANDS: &[Range] = &[khz(2_500.0, 22_000.0)];
const VOR_BAND: &[Range] = &[mhz(108.0, 118.0)];
const ILS_BANDS: &[Range] = &[mhz(108.1, 111.95), mhz(329.15, 335.0)];
const TIME_SIGNALS: &[Range] = &[
    spot_khz(40.0, 0.5),
    spot_khz(60.0, 0.5),
    spot_khz(68.5, 0.5),
    spot_khz(77.5, 0.5),
    spot_khz(162.0, 0.5),
];

const ANY: &[Modulation] = &[
    Modulation::Carrier,
    Modulation::Ook,
    Modulation::Am,
    Modulation::Ssb,
    Modulation::Fm,
    Modulation::Fsk2,
    Modulation::Fsk4,
    Modulation::Fsk8,
    Modulation::Psk2,
    Modulation::Psk4,
    Modulation::Ofdm,
    Modulation::NoiseLike,
    Modulation::Unknown,
];

#[derive(Clone, Copy)]
struct Signature {
    name: &'static str,
    type_id: Option<&'static str>,
    modulations: &'static [Modulation],
    bandwidth_hz: Range,
    symbol_rate_hz: Option<Range>,
    deviation_hz: Option<Range>,
    burst_ms: Option<Range>,
    burst_hint_ms: Option<Range>,
    symbol_us: Option<Range>,
    frequencies: &'static [Range],
    prior: f32,
    why: &'static str,
}

const fn signature(
    name: &'static str,
    type_id: Option<&'static str>,
    modulations: &'static [Modulation],
    bandwidth_hz: Range,
    why: &'static str,
) -> Signature {
    Signature {
        name,
        type_id,
        modulations,
        bandwidth_hz,
        symbol_rate_hz: None,
        deviation_hz: None,
        burst_ms: None,
        burst_hint_ms: None,
        symbol_us: None,
        frequencies: &[],
        prior: 1.0,
        why,
    }
}

const SIGNATURES: &[Signature] = &[
    Signature {
        deviation_hz: Some(range(15_000.0, 80_000.0)),
        frequencies: FM_BROADCAST,
        ..signature(
            "FM broadcast",
            Some("wfm"),
            &[Modulation::Fm],
            range(100_000.0, 220_000.0),
            "wideband FM at broadcast deviation",
        )
    },
    Signature {
        deviation_hz: Some(range(1_000.0, 5_000.0)),
        ..signature(
            "FM voice (narrowband)",
            Some("nfm"),
            &[Modulation::Fm],
            range(6_000.0, 25_000.0),
            "channel-width FM with no symbol structure",
        )
    },
    Signature {
        deviation_hz: Some(range(1_000.0, 3_500.0)),
        frequencies: PMR446,
        ..signature(
            "PMR446 voice (FM)",
            Some("nfm"),
            &[Modulation::Fm],
            range(6_000.0, 14_000.0),
            "narrowband FM on a licence-free PMR446 channel",
        )
    },
    Signature {
        deviation_hz: Some(range(1_000.0, 5_500.0)),
        frequencies: SELCALL_BANDS,
        prior: 0.9,
        ..signature(
            "Selcall (tone sequence over FM)",
            Some("selcall"),
            &[Modulation::Fm],
            range(6_000.0, 25_000.0),
            "a run of audio tones inside an FM channel",
        )
    },
    Signature {
        deviation_hz: Some(range(2_000.0, 5_500.0)),
        frequencies: MARINE_VHF,
        ..signature(
            "Marine VHF voice (FM)",
            Some("nfm"),
            &[Modulation::Fm],
            range(8_000.0, 25_000.0),
            "narrowband FM in the maritime VHF band",
        )
    },
    Signature {
        ..signature(
            "AM voice",
            Some("am"),
            &[Modulation::Am],
            range(2_000.0, 12_000.0),
            "amplitude modulation over a carrier",
        )
    },
    Signature {
        frequencies: AIRBAND,
        ..signature(
            "Airband voice (AM)",
            Some("am"),
            &[Modulation::Am],
            range(4_000.0, 10_000.0),
            "amplitude modulation over a carrier in the aeronautical band",
        )
    },
    Signature {
        ..signature(
            "SSB voice",
            Some("ssb"),
            &[Modulation::Ssb],
            range(1_500.0, 4_000.0),
            "one sideband, no carrier",
        )
    },
    signature(
        "Unmodulated carrier",
        None,
        &[Modulation::Carrier],
        range(0.0, 2_500.0),
        "a bare line with nothing on it",
    ),
    Signature {
        symbol_rate_hz: Some(range(4.0, 80.0)),
        ..signature(
            "Morse (CW)",
            Some("morse"),
            &[Modulation::Ook, Modulation::Carrier],
            range(0.0, 2_500.0),
            "a keyed carrier at hand speed",
        )
    },
    Signature {
        symbol_rate_hz: Some(about(512.0, 0.05)),
        deviation_hz: Some(range(3_000.0, 6_000.0)),
        frequencies: PAGING,
        ..signature(
            "POCSAG (512 bd)",
            Some("pocsag"),
            &[Modulation::Fsk2],
            range(6_000.0, 20_000.0),
            "two-level keying at a pager rate and ±4.5 kHz",
        )
    },
    Signature {
        symbol_rate_hz: Some(about(1_200.0, 0.05)),
        deviation_hz: Some(range(3_000.0, 6_000.0)),
        frequencies: PAGING,
        ..signature(
            "POCSAG (1200 bd)",
            Some("pocsag"),
            &[Modulation::Fsk2],
            range(6_000.0, 20_000.0),
            "two-level keying at a pager rate and ±4.5 kHz",
        )
    },
    Signature {
        symbol_rate_hz: Some(about(2_400.0, 0.05)),
        deviation_hz: Some(range(3_000.0, 6_000.0)),
        frequencies: PAGING,
        ..signature(
            "POCSAG (2400 bd)",
            Some("pocsag"),
            &[Modulation::Fsk2],
            range(6_000.0, 20_000.0),
            "two-level keying at a pager rate and ±4.5 kHz",
        )
    },
    Signature {
        symbol_rate_hz: Some(range(1_500.0, 6_700.0)),
        deviation_hz: Some(range(3_500.0, 6_000.0)),
        frequencies: PAGING,
        ..signature(
            "FLEX",
            Some("flex"),
            &[Modulation::Fsk2, Modulation::Fsk4],
            range(6_000.0, 20_000.0),
            "1600 to 6400 baud keying at ±4.8 kHz, pager band",
        )
    },
    Signature {
        symbol_rate_hz: Some(about(3_125.0, 0.05)),
        deviation_hz: Some(range(3_500.0, 6_500.0)),
        frequencies: ERMES_BAND,
        ..signature(
            "ERMES",
            Some("ermes"),
            &[Modulation::Fsk4],
            range(12_500.0, 30_000.0),
            "four-level keying at 3125 baud in the ERMES band",
        )
    },
    Signature {
        symbol_rate_hz: Some(about(9_600.0, 0.05)),
        deviation_hz: Some(range(1_800.0, 3_200.0)),
        frequencies: AIS_CHANNELS,
        ..signature(
            "AIS",
            Some("ais"),
            &[Modulation::Fsk2],
            range(12_000.0, 30_000.0),
            "9600 baud GMSK in a 25 kHz maritime channel",
        )
    },
    Signature {
        symbol_rate_hz: Some(about(2_400.0, 0.06)),
        frequencies: ACARS_VHF,
        ..signature(
            "ACARS",
            Some("acars"),
            &[Modulation::Am, Modulation::Ook],
            range(2_400.0, 9_000.0),
            "2400 baud minimum-shift keying carried on an AM carrier",
        )
    },
    Signature {
        symbol_rate_hz: Some(about(10_500.0, 0.05)),
        frequencies: VDL2_CHANNELS,
        ..signature(
            "VDL Mode 2",
            Some("vdl2"),
            &[Modulation::Psk4, Modulation::NoiseLike, Modulation::Unknown],
            range(15_000.0, 30_000.0),
            "10.5 kbaud D8PSK in an aeronautical data channel",
        )
    },
    Signature {
        symbol_rate_hz: Some(about(1_800.0, 0.06)),
        frequencies: HFDL_BANDS,
        ..signature(
            "HFDL",
            Some("hfdl"),
            &[Modulation::Psk2, Modulation::Psk4, Modulation::Unknown],
            range(1_500.0, 3_500.0),
            "1800 baud phase keying in an aeronautical HF channel",
        )
    },
    Signature {
        symbol_rate_hz: Some(range(40.0, 110.0)),
        deviation_hz: Some(range(40.0, 500.0)),
        ..signature(
            "RTTY",
            Some("rtty"),
            &[Modulation::Fsk2],
            range(100.0, 2_500.0),
            "a slow two-tone shift, teleprinter speed",
        )
    },
    Signature {
        symbol_rate_hz: Some(about(100.0, 0.08)),
        deviation_hz: Some(range(50.0, 200.0)),
        frequencies: NAVTEX_CHANNELS,
        ..signature(
            "NAVTEX (SITOR-B)",
            Some("navtex"),
            &[Modulation::Fsk2],
            range(100.0, 2_500.0),
            "100 baud at a 170 Hz shift",
        )
    },
    Signature {
        symbol_rate_hz: Some(about(100.0, 0.08)),
        deviation_hz: Some(range(50.0, 200.0)),
        frequencies: DSC_HF,
        ..signature(
            "DSC (HF)",
            Some("dsc"),
            &[Modulation::Fsk2],
            range(100.0, 1_000.0),
            "100 baud at a 170 Hz shift on a distress and calling frequency",
        )
    },
    Signature {
        symbol_rate_hz: Some(about(1_200.0, 0.06)),
        deviation_hz: Some(range(1_500.0, 5_000.0)),
        frequencies: DSC_VHF,
        ..signature(
            "DSC (VHF channel 70)",
            Some("dsc"),
            &[Modulation::Fm, Modulation::Fsk2],
            range(6_000.0, 20_000.0),
            "1200 baud audio tones inside an FM channel on marine channel 70",
        )
    },
    Signature {
        symbol_rate_hz: Some(range(1_000.0, 2_400.0)),
        deviation_hz: Some(range(1_500.0, 5_000.0)),
        frequencies: APRS_CHANNELS,
        ..signature(
            "APRS / AX.25 (AFSK over FM)",
            Some("aprs"),
            &[Modulation::Fm, Modulation::Fsk2],
            range(6_000.0, 20_000.0),
            "audio tones inside an FM channel, at packet speed",
        )
    },
    Signature {
        symbol_rate_hz: Some(range(300.0, 30_000.0)),
        frequencies: ISM,
        ..signature(
            "Sub-GHz remote (OOK)",
            Some("subghz"),
            &[Modulation::Ook],
            range(2_000.0, 160_000.0),
            "a keyed carrier at remote-control speed",
        )
    },
    Signature {
        symbol_rate_hz: Some(range(600.0, 100_000.0)),
        deviation_hz: Some(range(8_000.0, 80_000.0)),
        frequencies: ISM,
        ..signature(
            "Sub-GHz telemetry (2-FSK)",
            Some("subghz"),
            &[Modulation::Fsk2],
            range(15_000.0, 160_000.0),
            "a wide two-level shift, sensor-radio speed",
        )
    },
    Signature {
        symbol_rate_hz: Some(about(6.25, 0.15)),
        frequencies: FT8_SPOTS,
        ..signature(
            "FT8",
            Some("ft8"),
            &[Modulation::Fsk8],
            range(30.0, 90.0),
            "eight tones at 6.25 baud in a 50 Hz slot",
        )
    },
    Signature {
        symbol_rate_hz: Some(about(20.83, 0.15)),
        frequencies: FT4_SPOTS,
        ..signature(
            "FT4",
            Some("ft4"),
            &[Modulation::Fsk4],
            range(60.0, 130.0),
            "four tones at 20.8 baud in a 90 Hz slot",
        )
    },
    Signature {
        frequencies: WSPR_SPOTS,
        ..signature(
            "WSPR",
            Some("wspr"),
            &[Modulation::Fsk4, Modulation::Carrier],
            range(0.0, 20.0),
            "a 6 Hz wide four-tone beacon on a WSPR dial frequency",
        )
    },
    Signature {
        symbol_rate_hz: Some(about(31.25, 0.1)),
        frequencies: PSK_SPOTS,
        ..signature(
            "PSK31",
            Some("psk"),
            &[Modulation::Psk2],
            range(20.0, 120.0),
            "31.25 baud phase reversals in a 60 Hz slot",
        )
    },
    Signature {
        symbol_rate_hz: Some(about(62.5, 0.1)),
        frequencies: PSK_SPOTS,
        ..signature(
            "PSK63",
            Some("psk"),
            &[Modulation::Psk2],
            range(50.0, 200.0),
            "62.5 baud phase reversals in a 120 Hz slot",
        )
    },
    Signature {
        symbol_rate_hz: Some(about(125.0, 0.1)),
        frequencies: PSK_SPOTS,
        ..signature(
            "PSK125",
            Some("psk"),
            &[Modulation::Psk2],
            range(100.0, 400.0),
            "125 baud phase reversals in a 250 Hz slot",
        )
    },
    Signature {
        frequencies: SSTV_SPOTS,
        ..signature(
            "SSTV",
            Some("sstv"),
            &[
                Modulation::Fm,
                Modulation::Ssb,
                Modulation::Fsk2,
                Modulation::Fsk4,
                Modulation::Fsk8,
                Modulation::Unknown,
            ],
            range(800.0, 3_000.0),
            "a 1200 to 2300 Hz video tone on a picture frequency",
        )
    },
    Signature {
        frequencies: FREEDV_SPOTS,
        ..signature(
            "FreeDV",
            Some("freedv"),
            &[
                Modulation::Psk4,
                Modulation::Psk2,
                Modulation::Ofdm,
                Modulation::Unknown,
                Modulation::NoiseLike,
            ],
            range(900.0, 2_400.0),
            "a digital voice waveform inside an SSB channel",
        )
    },
    Signature {
        symbol_us: Some(about(1_000.0, 0.03)),
        frequencies: DAB_BANDS,
        ..signature(
            "DAB / DAB+",
            Some("dab"),
            &[Modulation::Ofdm],
            range(100_000.0, 2_000_000.0),
            "OFDM with a 1 ms useful symbol, the DAB transmission mode I",
        )
    },
    Signature {
        symbol_us: Some(range(9_000.0, 24_500.0)),
        frequencies: HF,
        ..signature(
            "DRM30",
            Some("drm"),
            &[Modulation::Ofdm],
            range(4_000.0, 21_000.0),
            "OFDM with a 9 to 24 ms useful symbol in a broadcast HF channel",
        )
    },
    Signature {
        symbol_us: Some(about(2_250.0, 0.03)),
        frequencies: DRM_VHF,
        ..signature(
            "DRM+",
            Some("drm"),
            &[Modulation::Ofdm],
            range(80_000.0, 120_000.0),
            "OFDM with a 2.25 ms useful symbol in a 100 kHz channel",
        )
    },
    Signature {
        symbol_us: Some(about(224.0, 0.03)),
        frequencies: DVBT_BANDS,
        ..signature(
            "DVB-T (2k)",
            None,
            &[Modulation::Ofdm],
            range(100_000.0, 8_000_000.0),
            "OFDM with a 224 µs useful symbol, terrestrial television",
        )
    },
    Signature {
        symbol_us: Some(about(896.0, 0.03)),
        frequencies: DVBT_BANDS,
        ..signature(
            "DVB-T (8k)",
            None,
            &[Modulation::Ofdm],
            range(100_000.0, 8_000_000.0),
            "OFDM with an 896 µs useful symbol, terrestrial television",
        )
    },
    Signature {
        burst_ms: Some(range(0.05, 0.15)),
        frequencies: ADSB,
        ..signature(
            "ADS-B / Mode S",
            Some("adsb"),
            ANY,
            range(100_000.0, 4_000_000.0),
            "120 µs pulse bursts on the 1090 MHz transponder downlink",
        )
    },
    Signature {
        burst_ms: Some(range(0.3, 0.5)),
        frequencies: DECT_BANDS,
        ..signature(
            "DECT",
            Some("dect"),
            ANY,
            range(100_000.0, 2_000_000.0),
            "417 µs slots every 10 ms in the DECT band",
        )
    },
    Signature {
        burst_ms: Some(range(6.0, 10.0)),
        frequencies: IRIDIUM_BAND,
        ..signature(
            "Iridium",
            Some("iridium"),
            ANY,
            range(25_000.0, 50_000.0),
            "8.3 ms bursts in a 41.67 kHz satellite channel",
        )
    },
    Signature {
        symbol_rate_hz: Some(about(1_200.0, 0.06)),
        frequencies: INMARSAT_BAND,
        ..signature(
            "Inmarsat STD-C",
            Some("inmarsat_stdc"),
            &[Modulation::Psk2, Modulation::Unknown],
            range(2_000.0, 6_000.0),
            "1200 baud BPSK from a geostationary satellite",
        )
    },
    Signature {
        symbol_rate_hz: Some(range(500.0, 11_000.0)),
        frequencies: INMARSAT_BAND,
        ..signature(
            "Inmarsat Aero",
            Some("inmarsat_aero"),
            &[Modulation::Psk2, Modulation::Psk4, Modulation::Unknown],
            range(2_000.0, 20_000.0),
            "phase keying at an aeronautical satellite rate",
        )
    },
    Signature {
        frequencies: VOR_BAND,
        ..signature(
            "VOR",
            Some("vor"),
            &[Modulation::Am],
            range(15_000.0, 25_000.0),
            "a 30 Hz AM bearing tone and a 9960 Hz subcarrier on a VOR frequency",
        )
    },
    Signature {
        frequencies: ILS_BANDS,
        ..signature(
            "ILS",
            Some("ils"),
            &[Modulation::Am, Modulation::Carrier],
            range(0.0, 4_000.0),
            "90 and 150 Hz amplitude tones on a localizer or glide-slope frequency",
        )
    },
    Signature {
        frequencies: TIME_SIGNALS,
        ..signature(
            "Time signal (DCF77 / MSF / WWVB / JJY)",
            Some("radio_clock"),
            &[Modulation::Carrier, Modulation::Ook, Modulation::Am],
            range(0.0, 400.0),
            "a longwave carrier dipped once a second on a standard-time frequency",
        )
    },
];

fn dv_signatures() -> impl Iterator<Item = Signature> {
    MODE_SIGNATURES.iter().map(|mode| {
        let (frequencies, burst_ms): (&'static [Range], Option<Range>) = match mode.type_id {
            "dmr" => (LAND_MOBILE, Some(range(25.0, 35.0))),
            "p25" | "nxdn" | "dpmr" => (LAND_MOBILE, None),
            _ => (AMATEUR_VU, None),
        };
        Signature {
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
            burst_ms: None,
            burst_hint_ms: burst_ms,
            symbol_us: None,
            frequencies,
            prior: 1.0,
            why: "matches the mode's channel width, symbol rate and deviation",
        }
    })
}

pub(crate) fn in_allocation(kind: &str, frequency_hz: f64) -> bool {
    SIGNATURES.iter().any(|signature| {
        signature.type_id == Some(kind)
            && signature
                .frequencies
                .iter()
                .any(|range| (range.low..=range.high).contains(&frequency_hz))
    })
}

pub(crate) fn candidates(
    modulation: Modulation,
    band: &Band,
    waveform: &Waveform,
    frequency_hz: Option<f64>,
) -> Vec<ProtocolMatch> {
    let mut found: Vec<(ProtocolMatch, f64)> = SIGNATURES
        .iter()
        .copied()
        .chain(dv_signatures())
        .filter(|signature| signature.modulations.contains(&modulation))
        .filter_map(|signature| {
            let matched = score(&signature, band, waveform, frequency_hz)?;
            let tolerance = signature.symbol_rate_hz.map_or(f64::INFINITY, |rate| {
                (rate.high - rate.low) / rate.high.max(1.0)
            });
            Some((matched, tolerance))
        })
        .collect();
    found.sort_by(|(a, a_tolerance), (b, b_tolerance)| {
        b.score
            .total_cmp(&a.score)
            .then(a_tolerance.total_cmp(b_tolerance))
    });
    found.truncate(MAX_CANDIDATES);
    found.into_iter().map(|(matched, _)| matched).collect()
}

fn score(
    signature: &Signature,
    band: &Band,
    waveform: &Waveform,
    frequency_hz: Option<f64>,
) -> Option<ProtocolMatch> {
    let mut total = signature.bandwidth_hz.score(band.bandwidth_hz) * WEIGHT_BANDWIDTH;
    let mut weight = WEIGHT_BANDWIDTH;

    if let Some(expected) = signature.symbol_rate_hz {
        total += WEIGHT_SYMBOL_RATE
            * waveform
                .symbol_rate_hz
                .map_or(UNMEASURED, |measured| expected.score(measured));
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
    if let Some(expected) = signature.symbol_us {
        total += WEIGHT_SYMBOL_LENGTH
            * waveform
                .ofdm_symbol_us
                .map_or(UNMEASURED, |measured| expected.score(measured));
        weight += WEIGHT_SYMBOL_LENGTH;
    }
    if let Some(expected) = signature.burst_ms {
        let measured = waveform.burst_ms?;
        total += WEIGHT_BURST * expected.score(measured);
        weight += WEIGHT_BURST;
    }
    if let (Some(expected), Some(measured)) = (signature.burst_hint_ms, waveform.burst_ms) {
        total += WEIGHT_BURST * expected.score(measured);
        weight += WEIGHT_BURST;
    }
    let allocation = frequency_hz.and_then(|hz| allocation_score(signature, hz));
    let placed = match (frequency_hz, signature.frequencies.is_empty()) {
        (Some(_), false) => {
            OUT_OF_ALLOCATION + (1.0 - OUT_OF_ALLOCATION) * allocation.unwrap_or(0.0)
        }
        (Some(_), true) => GENERIC_WITH_DIAL,
        (None, false) => UNKNOWN_DIAL,
        (None, true) => 1.0,
    };

    let score = total / weight * placed * signature.prior;
    (score >= MIN_SCORE).then(|| ProtocolMatch {
        name: signature.name.to_owned(),
        type_id: signature.type_id.map(str::to_owned),
        score,
        confirmed: false,
        why: if allocation == Some(1.0) {
            format!("{}, inside its allocation", signature.why)
        } else {
            signature.why.to_owned()
        },
    })
}

fn allocation_score(signature: &Signature, frequency_hz: f64) -> Option<f32> {
    if signature.frequencies.is_empty() {
        return None;
    }
    Some(
        signature
            .frequencies
            .iter()
            .map(|allocation| allocation.allocation_score(frequency_hz))
            .fold(0.0, f32::max),
    )
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
        let found = candidates(
            Modulation::Fsk2,
            &band(12_500.0),
            &keyed(1_200.0, 4_500.0),
            None,
        );
        assert_eq!(names(&found).first(), Some(&"POCSAG (1200 bd)"));
    }

    #[test]
    fn ermes_uses_its_symbol_rate_and_occupied_bandwidth() {
        let found = candidates(
            Modulation::Fsk4,
            &band(13_125.0),
            &keyed(3_126.0, 4_762.0),
            Some(169_650_000.0),
        );
        assert_eq!(names(&found).first(), Some(&"ERMES"));
        assert_eq!(found[0].score, 1.0);
    }

    #[test]
    fn a_maritime_carrier_finds_ais() {
        let found = candidates(
            Modulation::Fsk2,
            &band(25_000.0),
            &keyed(9_600.0, 2_400.0),
            None,
        );
        assert_eq!(names(&found).first(), Some(&"AIS"));
    }

    #[test]
    fn the_c4fm_family_comes_back_as_a_shortlist() {
        let found = candidates(
            Modulation::Fsk4,
            &band(12_500.0),
            &keyed(4_800.0, 1_944.0),
            None,
        );
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
        let found = candidates(Modulation::Fm, &band(180_000.0), &waveform, None);
        assert_eq!(names(&found).first(), Some(&"FM broadcast"));
    }

    #[test]
    fn nothing_is_offered_for_a_family_no_protocol_here_uses() {
        assert!(
            candidates(
                Modulation::Carrier,
                &band(200_000.0),
                &keyed(9_600.0, 0.0),
                None
            )
            .is_empty()
        );
    }

    #[test]
    fn the_dial_frequency_settles_a_tie_between_look_alikes() {
        let waveform = Waveform {
            deviation_hz: 2_500.0,
            ..Waveform::default()
        };
        let at_sea = candidates(
            Modulation::Fm,
            &band(12_500.0),
            &waveform,
            Some(156_800_000.0),
        );
        assert_eq!(names(&at_sea).first(), Some(&"Marine VHF voice (FM)"));
        assert!(
            at_sea[0].why.ends_with("inside its allocation"),
            "{}",
            at_sea[0].why
        );
        let on_pmr = candidates(
            Modulation::Fm,
            &band(12_500.0),
            &waveform,
            Some(446_006_250.0),
        );
        assert_eq!(names(&on_pmr).first(), Some(&"PMR446 voice (FM)"));
        let anywhere = candidates(Modulation::Fm, &band(12_500.0), &waveform, None);
        assert!(anywhere.iter().all(|m| !m.why.contains("allocation")));
    }

    #[test]
    fn a_bare_ism_frequency_does_not_make_pocsag_of_a_pager_shift_elsewhere() {
        let found = candidates(
            Modulation::Fsk2,
            &band(12_500.0),
            &keyed(1_200.0, 4_500.0),
            Some(466_075_000.0),
        );
        assert_eq!(names(&found).first(), Some(&"POCSAG (1200 bd)"));
        assert!(found[0].score > 0.99, "{}", found[0].score);
        let far = candidates(
            Modulation::Fsk2,
            &band(12_500.0),
            &keyed(1_200.0, 4_500.0),
            Some(14_000_000.0),
        );
        assert_eq!(names(&far).first(), Some(&"POCSAG (1200 bd)"));
        assert!(far[0].score < found[0].score);
    }

    #[test]
    fn a_millisecond_symbol_in_band_three_is_dab() {
        let waveform = Waveform {
            ofdm_symbol_us: Some(998.0),
            ofdm_guard_us: Some(246.0),
            ofdm_strength: 0.18,
            ..Waveform::default()
        };
        let found = candidates(
            Modulation::Ofdm,
            &band(192_000.0),
            &waveform,
            Some(227_360_000.0),
        );
        assert_eq!(names(&found).first(), Some(&"DAB / DAB+"));
    }

    #[test]
    fn slot_bursts_lift_dmr_over_the_other_c4fm_modes() {
        let bursty = Waveform {
            burst_ms: Some(29.5),
            burst_period_ms: Some(60.0),
            ..keyed(4_800.0, 1_944.0)
        };
        let found = candidates(Modulation::Fsk4, &band(12_500.0), &bursty, None);
        assert_eq!(names(&found).first(), Some(&"DMR"));
        let steady = candidates(
            Modulation::Fsk4,
            &band(12_500.0),
            &keyed(4_800.0, 1_944.0),
            None,
        );
        let dmr = |found: &[ProtocolMatch]| found.iter().find(|m| m.name == "DMR").map(|m| m.score);
        assert_eq!(
            dmr(&steady),
            dmr(&candidates(
                Modulation::Fsk4,
                &band(12_500.0),
                &keyed(4_800.0, 1_944.0),
                None
            ))
        );
    }

    #[test]
    fn short_bursts_at_the_transponder_frequency_are_mode_s() {
        let waveform = Waveform {
            burst_ms: Some(0.12),
            envelope_variation: 0.5,
            ..Waveform::default()
        };
        let found = candidates(
            Modulation::NoiseLike,
            &band(192_000.0),
            &waveform,
            Some(1_090_000_000.0),
        );
        assert_eq!(names(&found).first(), Some(&"ADS-B / Mode S"));
    }
}
