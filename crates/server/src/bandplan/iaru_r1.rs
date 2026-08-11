//! IARU Region 1 amateur band plan — an overlay, not an allocation.
//!
//! Nothing here is law. A regulator gives the amateur service a band; this table is how the
//! operators in it agreed to divide the band up so that a CW station and an FM repeater do not
//! land on each other. That is exactly why it resolves into its own lane: it describes the same
//! spectrum the allocation lane already covers, at a finer grain, and overriding the allocation
//! with it would lose the fact that the band is amateur at all.
//!
//! Only the bands a receiver is likely to be pointed at are here — HF through 70 cm. The
//! microwave plans are far longer than they are useful on a ruler.
//!
//! Source: IARU Region 1 HF and VHF/UHF/Microwave band plans (2023 editions).

use sdrmm_wire::{BandLayerKind, BandService};

use super::{
    Entry, Layer,
    mode::{aprs, lsb, morse, nfm, usb},
};

pub(super) static IARU_R1: Layer = Layer {
    id: "iaru-r1",
    name: "Amateur band plan — IARU R1",
    authority: "IARU Region 1",
    source: "IARU R1 HF and VHF/UHF band plans (2023)",
    kind: BandLayerKind::Amateur,
    entries: &[
        // 160 m
        Entry {
            start_hz: 1_810_000.0,
            stop_hz: 1_838_000.0,
            service: BandService::Amateur,
            name: "160 m — CW",
            aliases: &["160 m cw"],
            suggested: Some(morse),
            notes: Some("1836 kHz is the QRP centre of activity."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 1_838_000.0,
            stop_hz: 1_843_000.0,
            service: BandService::Amateur,
            name: "160 m — narrow band digital",
            aliases: &["160 m digital", "ft8"],
            suggested: Some(lsb),
            notes: Some("FT8 at 1840 kHz."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 1_843_000.0,
            stop_hz: 2_000_000.0,
            service: BandService::Amateur,
            name: "160 m — all modes, SSB",
            aliases: &["160 m ssb"],
            suggested: Some(lsb),
            notes: Some("1855 kHz is the SSB QRP centre."),
            ..Entry::ROW
        },
        // 80 m
        Entry {
            start_hz: 3_500_000.0,
            stop_hz: 3_570_000.0,
            service: BandService::Amateur,
            name: "80 m — CW",
            aliases: &["80 m cw"],
            suggested: Some(morse),
            notes: Some(
                "3560 kHz is the QRP centre; the bottom 10 kHz is for intercontinental work.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 3_570_000.0,
            stop_hz: 3_600_000.0,
            service: BandService::Amateur,
            name: "80 m — narrow band digital",
            aliases: &["80 m digital", "ft8"],
            suggested: Some(lsb),
            notes: Some("FT8 at 3573 kHz."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 3_600_000.0,
            stop_hz: 3_800_000.0,
            service: BandService::Amateur,
            name: "80 m — all modes, SSB",
            aliases: &["80 m ssb"],
            suggested: Some(lsb),
            notes: Some("3690 kHz is the SSB QRP centre; 3735 kHz the image (SSTV) centre."),
            ..Entry::ROW
        },
        // 40 m
        Entry {
            start_hz: 7_000_000.0,
            stop_hz: 7_040_000.0,
            service: BandService::Amateur,
            name: "40 m — CW",
            aliases: &["40 m cw"],
            suggested: Some(morse),
            notes: Some("7030 kHz is the QRP centre."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 7_040_000.0,
            stop_hz: 7_060_000.0,
            service: BandService::Amateur,
            name: "40 m — narrow band digital",
            aliases: &["40 m digital"],
            suggested: Some(lsb),
            ..Entry::ROW
        },
        Entry {
            start_hz: 7_060_000.0,
            stop_hz: 7_200_000.0,
            service: BandService::Amateur,
            name: "40 m — all modes, SSB",
            aliases: &["40 m ssb", "ft8"],
            suggested: Some(lsb),
            notes: Some(
                "7090 kHz is the SSB QRP centre, 7110 kHz the emergency centre, and FT8 sits \
                 at 7074 kHz.",
            ),
            ..Entry::ROW
        },
        // 30 m
        Entry {
            start_hz: 10_100_000.0,
            stop_hz: 10_130_000.0,
            service: BandService::Amateur,
            name: "30 m — CW",
            aliases: &["30 m cw"],
            suggested: Some(morse),
            notes: Some("10116 kHz is the QRP centre."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 10_130_000.0,
            stop_hz: 10_150_000.0,
            service: BandService::Amateur,
            name: "30 m — narrow band digital",
            aliases: &["30 m digital", "ft8"],
            suggested: Some(usb),
            notes: Some("FT8 at 10136 kHz. No phone anywhere on this band."),
            ..Entry::ROW
        },
        // 20 m
        Entry {
            start_hz: 14_000_000.0,
            stop_hz: 14_070_000.0,
            service: BandService::Amateur,
            name: "20 m — CW",
            aliases: &["20 m cw"],
            suggested: Some(morse),
            notes: Some("14060 kHz is the QRP centre."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 14_070_000.0,
            stop_hz: 14_099_000.0,
            service: BandService::Amateur,
            name: "20 m — narrow band digital",
            aliases: &["20 m digital", "ft8"],
            suggested: Some(usb),
            notes: Some("FT8 at 14074 kHz — probably the busiest 3 kHz in amateur radio."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 14_099_000.0,
            stop_hz: 14_101_000.0,
            service: BandService::Amateur,
            name: "20 m — IBP beacons",
            aliases: &["beacon", "ibp", "ncdxf"],
            suggested: Some(morse),
            notes: Some(
                "The NCDXF/IARU international beacon project: eighteen stations round the world \
                 transmit in a three-minute rotation, so the band tells you where it is open.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 14_101_000.0,
            stop_hz: 14_350_000.0,
            service: BandService::Amateur,
            name: "20 m — all modes, SSB",
            aliases: &["20 m ssb"],
            suggested: Some(usb),
            notes: Some("14230 kHz is the SSTV calling frequency; 14285 kHz the SSB QRP centre."),
            ..Entry::ROW
        },
        // 17 m
        Entry {
            start_hz: 18_068_000.0,
            stop_hz: 18_095_000.0,
            service: BandService::Amateur,
            name: "17 m — CW",
            aliases: &["17 m cw"],
            suggested: Some(morse),
            ..Entry::ROW
        },
        Entry {
            start_hz: 18_095_000.0,
            stop_hz: 18_109_000.0,
            service: BandService::Amateur,
            name: "17 m — narrow band digital",
            aliases: &["17 m digital", "ft8"],
            suggested: Some(usb),
            notes: Some("FT8 at 18100 kHz."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 18_109_000.0,
            stop_hz: 18_111_000.0,
            service: BandService::Amateur,
            name: "17 m — IBP beacons",
            aliases: &["beacon", "ibp"],
            suggested: Some(morse),
            ..Entry::ROW
        },
        Entry {
            start_hz: 18_111_000.0,
            stop_hz: 18_168_000.0,
            service: BandService::Amateur,
            name: "17 m — all modes, SSB",
            aliases: &["17 m ssb"],
            suggested: Some(usb),
            ..Entry::ROW
        },
        // 15 m
        Entry {
            start_hz: 21_000_000.0,
            stop_hz: 21_070_000.0,
            service: BandService::Amateur,
            name: "15 m — CW",
            aliases: &["15 m cw"],
            suggested: Some(morse),
            notes: Some("21060 kHz is the QRP centre."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 21_070_000.0,
            stop_hz: 21_149_000.0,
            service: BandService::Amateur,
            name: "15 m — narrow band digital",
            aliases: &["15 m digital", "ft8"],
            suggested: Some(usb),
            notes: Some("FT8 at 21074 kHz."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 21_149_000.0,
            stop_hz: 21_151_000.0,
            service: BandService::Amateur,
            name: "15 m — IBP beacons",
            aliases: &["beacon", "ibp"],
            suggested: Some(morse),
            ..Entry::ROW
        },
        Entry {
            start_hz: 21_151_000.0,
            stop_hz: 21_450_000.0,
            service: BandService::Amateur,
            name: "15 m — all modes, SSB",
            aliases: &["15 m ssb"],
            suggested: Some(usb),
            notes: Some("21285 kHz is the SSB QRP centre; 21340 kHz the SSTV calling frequency."),
            ..Entry::ROW
        },
        // 12 m
        Entry {
            start_hz: 24_890_000.0,
            stop_hz: 24_915_000.0,
            service: BandService::Amateur,
            name: "12 m — CW",
            aliases: &["12 m cw"],
            suggested: Some(morse),
            ..Entry::ROW
        },
        Entry {
            start_hz: 24_915_000.0,
            stop_hz: 24_929_000.0,
            service: BandService::Amateur,
            name: "12 m — narrow band digital",
            aliases: &["12 m digital", "ft8"],
            suggested: Some(usb),
            notes: Some("FT8 at 24915 kHz."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 24_929_000.0,
            stop_hz: 24_931_000.0,
            service: BandService::Amateur,
            name: "12 m — IBP beacons",
            aliases: &["beacon", "ibp"],
            suggested: Some(morse),
            ..Entry::ROW
        },
        Entry {
            start_hz: 24_931_000.0,
            stop_hz: 24_990_000.0,
            service: BandService::Amateur,
            name: "12 m — all modes, SSB",
            aliases: &["12 m ssb"],
            suggested: Some(usb),
            ..Entry::ROW
        },
        // 10 m
        Entry {
            start_hz: 28_000_000.0,
            stop_hz: 28_070_000.0,
            service: BandService::Amateur,
            name: "10 m — CW",
            aliases: &["10 m cw"],
            suggested: Some(morse),
            notes: Some("28060 kHz is the QRP centre."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 28_070_000.0,
            stop_hz: 28_190_000.0,
            service: BandService::Amateur,
            name: "10 m — narrow band digital",
            aliases: &["10 m digital", "ft8"],
            suggested: Some(usb),
            notes: Some("FT8 at 28074 kHz."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 28_190_000.0,
            stop_hz: 28_225_000.0,
            service: BandService::Amateur,
            name: "10 m — beacons",
            aliases: &["beacon", "ibp"],
            suggested: Some(morse),
            notes: Some(
                "Regional and IBP beacons. If you hear one, the band is open in that direction.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 28_225_000.0,
            stop_hz: 29_200_000.0,
            service: BandService::Amateur,
            name: "10 m — all modes, SSB",
            aliases: &["10 m ssb"],
            suggested: Some(usb),
            notes: Some("28680 kHz SSTV; 28885 kHz is the liaison frequency for 6 m openings."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 29_200_000.0,
            stop_hz: 29_300_000.0,
            service: BandService::Amateur,
            name: "10 m — digital and packet",
            aliases: &["10 m packet"],
            suggested: Some(nfm),
            ..Entry::ROW
        },
        Entry {
            start_hz: 29_300_000.0,
            stop_hz: 29_510_000.0,
            service: BandService::Amateur,
            name: "10 m — satellite downlink",
            aliases: &["satellite"],
            suggested: Some(usb),
            ..Entry::ROW
        },
        Entry {
            start_hz: 29_510_000.0,
            stop_hz: 29_700_000.0,
            service: BandService::Amateur,
            name: "10 m — FM repeaters and simplex",
            aliases: &["10 m fm"],
            suggested: Some(nfm),
            channel_step_hz: Some(10_000.0),
            notes: Some("29.600 MHz is the FM calling channel."),
            ..Entry::ROW
        },
        // 6 m
        Entry {
            start_hz: 50_000_000.0,
            stop_hz: 50_100_000.0,
            service: BandService::Amateur,
            name: "6 m — CW and beacons",
            aliases: &["6 m cw", "beacon"],
            suggested: Some(morse),
            notes: Some("50.000–50.030 MHz is the beacon sub-band."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 50_100_000.0,
            stop_hz: 50_500_000.0,
            service: BandService::Amateur,
            name: "6 m — SSB and CW",
            aliases: &["6 m ssb", "dx window"],
            suggested: Some(usb),
            notes: Some(
                "50.110 MHz is the intercontinental calling frequency and 50.150 MHz the SSB \
                 centre; 50.313 MHz carries FT8.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 50_500_000.0,
            stop_hz: 51_000_000.0,
            service: BandService::Amateur,
            name: "6 m — all modes and digital",
            aliases: &["6 m digital"],
            suggested: Some(usb),
            ..Entry::ROW
        },
        Entry {
            start_hz: 51_000_000.0,
            stop_hz: 52_000_000.0,
            service: BandService::Amateur,
            name: "6 m — FM and digital voice",
            aliases: &["6 m fm"],
            suggested: Some(nfm),
            channel_step_hz: Some(12_500.0),
            notes: Some("51.510 MHz is the FM calling channel."),
            ..Entry::ROW
        },
        // 2 m
        Entry {
            start_hz: 144_000_000.0,
            stop_hz: 144_150_000.0,
            service: BandService::Amateur,
            name: "2 m — CW and EME",
            aliases: &["2 m cw", "eme", "moonbounce"],
            suggested: Some(morse),
            notes: Some(
                "144.050 MHz is the CW calling frequency; the bottom 35 kHz is the \
                 earth-moon-earth window, where signals arrive below the noise floor.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 144_150_000.0,
            stop_hz: 144_400_000.0,
            service: BandService::Amateur,
            name: "2 m — SSB and MGM",
            aliases: &["2 m ssb"],
            suggested: Some(usb),
            notes: Some("144.300 MHz is the SSB calling frequency; 144.174 MHz carries FT8."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 144_400_000.0,
            stop_hz: 144_490_000.0,
            service: BandService::Amateur,
            name: "2 m — beacons",
            aliases: &["beacon"],
            suggested: Some(morse),
            ..Entry::ROW
        },
        Entry {
            start_hz: 144_490_000.0,
            stop_hz: 144_794_000.0,
            service: BandService::Amateur,
            name: "2 m — all modes",
            aliases: &["2 m"],
            suggested: Some(nfm),
            ..Entry::ROW
        },
        Entry {
            start_hz: 144_794_000.0,
            stop_hz: 144_990_000.0,
            service: BandService::Amateur,
            name: "2 m — APRS and packet",
            aliases: &["aprs", "packet", "ax.25"],
            suggested: Some(aprs),
            notes: Some(
                "144.800 MHz is the Region 1 APRS frequency: AFSK1200 position beacons, \
                 digipeated across the continent.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 144_990_000.0,
            stop_hz: 145_194_000.0,
            service: BandService::Amateur,
            name: "2 m — FM repeater inputs",
            aliases: &["2 m repeater"],
            suggested: Some(nfm),
            channel_step_hz: Some(12_500.0),
            notes: Some("The outputs are 600 kHz higher, in 145.594–145.806 MHz."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 145_194_000.0,
            stop_hz: 145_206_000.0,
            service: BandService::Amateur,
            name: "2 m — space communication",
            aliases: &["satellite"],
            suggested: Some(nfm),
            ..Entry::ROW
        },
        Entry {
            start_hz: 145_206_000.0,
            stop_hz: 145_594_000.0,
            service: BandService::Amateur,
            name: "2 m — FM simplex",
            aliases: &["2 m fm", "simplex"],
            suggested: Some(nfm),
            channel_step_hz: Some(12_500.0),
            notes: Some("145.500 MHz is the FM calling channel — the first place to listen."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 145_594_000.0,
            stop_hz: 145_806_000.0,
            service: BandService::Amateur,
            name: "2 m — FM repeater outputs",
            aliases: &["2 m repeater"],
            suggested: Some(nfm),
            channel_step_hz: Some(12_500.0),
            ..Entry::ROW
        },
        Entry {
            start_hz: 145_806_000.0,
            stop_hz: 146_000_000.0,
            service: BandService::Amateur,
            name: "2 m — satellite",
            aliases: &["satellite", "iss"],
            suggested: Some(nfm),
            notes: Some("Amateur satellite downlinks and the ISS."),
            ..Entry::ROW
        },
        // 70 cm
        Entry {
            start_hz: 430_000_000.0,
            stop_hz: 432_000_000.0,
            service: BandService::Amateur,
            name: "70 cm — all modes and repeater links",
            aliases: &["70 cm"],
            suggested: Some(nfm),
            ..Entry::ROW
        },
        Entry {
            start_hz: 432_000_000.0,
            stop_hz: 432_500_000.0,
            service: BandService::Amateur,
            name: "70 cm — CW, SSB and EME",
            aliases: &["70 cm ssb", "eme"],
            suggested: Some(usb),
            notes: Some("432.200 MHz is the SSB calling frequency; the bottom 25 kHz is EME."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 432_500_000.0,
            stop_hz: 433_000_000.0,
            service: BandService::Amateur,
            name: "70 cm — all modes and beacons",
            aliases: &["70 cm", "beacon"],
            suggested: Some(nfm),
            ..Entry::ROW
        },
        Entry {
            start_hz: 433_000_000.0,
            stop_hz: 433_400_000.0,
            service: BandService::Amateur,
            name: "70 cm — FM repeater outputs",
            aliases: &["70 cm repeater"],
            suggested: Some(nfm),
            channel_step_hz: Some(12_500.0),
            ..Entry::ROW
        },
        Entry {
            start_hz: 433_400_000.0,
            stop_hz: 434_600_000.0,
            service: BandService::Amateur,
            name: "70 cm — FM simplex and digital voice",
            aliases: &["70 cm fm", "simplex", "dmr"],
            suggested: Some(nfm),
            channel_step_hz: Some(12_500.0),
            notes: Some(
                "433.500 MHz is the FM calling channel. This is also where the 433 MHz ISM \
                 devices live, so the noise floor here is not the band's fault.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 434_600_000.0,
            stop_hz: 435_000_000.0,
            service: BandService::Amateur,
            name: "70 cm — all modes and repeater links",
            aliases: &["70 cm"],
            suggested: Some(nfm),
            ..Entry::ROW
        },
        Entry {
            start_hz: 435_000_000.0,
            stop_hz: 438_000_000.0,
            service: BandService::Amateur,
            name: "70 cm — satellite",
            aliases: &["satellite", "iss", "cubesat"],
            suggested: Some(nfm),
            notes: Some("Amateur satellite downlinks and the ISS packet digipeater."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 438_000_000.0,
            stop_hz: 440_000_000.0,
            service: BandService::Amateur,
            name: "70 cm — FM repeater inputs and digital",
            aliases: &["70 cm repeater"],
            suggested: Some(nfm),
            channel_step_hz: Some(12_500.0),
            ..Entry::ROW
        },
    ],
};
