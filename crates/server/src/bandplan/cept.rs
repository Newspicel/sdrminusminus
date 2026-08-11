//! European harmonisation (CEPT/ECC), the layer between the ITU table and a national plan.
//!
//! Only the bands CEPT actually harmonises: the licence-exempt allocations an operator is most
//! likely to point a receiver at, plus the public-safety band that is the same shape in every
//! member country even though each one runs its own network.
//!
//! Source: ERC Recommendation 70-03 (short-range devices) and the ECC decisions it cites.

use sdrmm_wire::{BandLayerKind, BandService};

use super::{
    Entry, Layer,
    mode::{am, nfm, subghz},
};

pub(super) static CEPT: Layer = Layer {
    id: "cept",
    name: "Europe — CEPT",
    authority: "CEPT/ECC",
    source: "ERC/REC 70-03 and related ECC decisions — curated extract",
    kind: BandLayerKind::Regulatory,
    entries: &[
        Entry {
            start_hz: 26_960_000.0,
            stop_hz: 27_410_000.0,
            service: BandService::Mobile,
            name: "CB — 40 CEPT channels",
            aliases: &["cb", "citizens band", "11 m"],
            suggested: Some(am),
            channel_step_hz: Some(10_000.0),
            notes: Some(
                "4 W AM and FM, 12 W PEP SSB. Channel 9 (27.065 MHz) is the emergency channel \
                 and channel 19 the road channel.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 169_400_000.0,
            stop_hz: 169_812_500.0,
            service: BandService::Mobile,
            name: "Wireless M-Bus and assistive listening — 169 MHz",
            aliases: &["m-bus", "wmbus", "169", "smart meter"],
            suggested: Some(subghz),
            notes: Some("Long-range utility metering, and hearing-aid assistive listening."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 380_000_000.0,
            stop_hz: 400_000_000.0,
            service: BandService::Mobile,
            name: "TETRA — public safety",
            aliases: &["tetra", "emergency services", "public safety"],
            notes: Some(
                "Digital trunked radio for police, fire and ambulance: π/4-DQPSK in 25 kHz \
                 carriers. Encrypted end to end, so only the presence of a carrier is visible.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 446_000_000.0,
            stop_hz: 446_200_000.0,
            service: BandService::Mobile,
            name: "PMR446 — licence-exempt",
            aliases: &["pmr446", "pmr", "walkie talkie", "handheld"],
            suggested: Some(nfm),
            channel_step_hz: Some(12_500.0),
            notes: Some(
                "16 analogue FM channels at 0.5 W ERP, no licence anywhere in CEPT. dPMR and \
                 DMR digital channels share the same 200 kHz.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 863_000_000.0,
            stop_hz: 870_000_000.0,
            service: BandService::Ism,
            name: "SRD — 868 MHz",
            aliases: &[
                "868",
                "srd",
                "lora",
                "lorawan",
                "sigfox",
                "wmbus",
                "smart home",
            ],
            suggested: Some(subghz),
            notes: Some(
                "ERC/REC 70-03. LoRaWAN, wireless M-Bus, alarms and sensors, all duty-cycle \
                 limited. Europe's answer to Region 2's 915 MHz.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 5_795_000_000.0,
            stop_hz: 5_815_000_000.0,
            service: BandService::Mobile,
            name: "Road tolling — DSRC",
            aliases: &["dsrc", "toll", "tolling"],
            notes: Some("CEN DSRC beacons at motorway gantries."),
            ..Entry::ROW
        },
    ],
};
