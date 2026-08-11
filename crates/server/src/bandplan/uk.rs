//! United Kingdom — Ofcom.
//!
//! Curated the same way as the German layer: only what differs from CEPT and is worth naming on
//! a ruler. The UK is the interesting Region 1 case because several of its allocations are
//! genuinely its own — the 27/81 CB channels and a full 500 kHz at 70 MHz among them.
//!
//! Source: Ofcom UK Frequency Allocation Table (Issue 22) and the relevant licence-exempt
//! regulations.

use sdrmm_wire::{BandLayerKind, BandService};

use super::{
    Entry, Layer,
    mode::{nfm, wfm},
};

pub(super) static OFCOM: Layer = Layer {
    id: "gb",
    name: "United Kingdom — Ofcom",
    authority: "Ofcom",
    source: "UK Frequency Allocation Table, Issue 22 — curated extract",
    kind: BandLayerKind::Regulatory,
    entries: &[
        Entry {
            start_hz: 27_601_250.0,
            stop_hz: 27_991_250.0,
            service: BandService::Mobile,
            name: "CB 27/81 — UK FM channels",
            aliases: &["cb", "27/81", "citizens band"],
            suggested: Some(nfm),
            channel_step_hz: Some(10_000.0),
            notes: Some(
                "The UK's own 40-channel FM allocation from 1981, still authorised alongside \
                 the CEPT channels below it.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 70_000_000.0,
            stop_hz: 70_500_000.0,
            service: BandService::Amateur,
            name: "4 m amateur",
            aliases: &["4 m", "70 mhz", "four metres"],
            suggested: Some(nfm),
            channel_step_hz: Some(12_500.0),
            notes: Some(
                "A Region 1 speciality and the UK has the widest slice of it: 70.450 MHz is \
                 the FM calling channel, 70.200 MHz the SSB centre.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 88_000_000.0,
            stop_hz: 108_000_000.0,
            service: BandService::Broadcast,
            name: "FM broadcast",
            aliases: &["fm", "radio"],
            suggested: Some(wfm),
            channel_step_hz: Some(100_000.0),
            notes: Some("The BBC national networks cluster between 88 and 94.6 MHz."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 174_000_000.0,
            stop_hz: 230_000_000.0,
            service: BandService::Broadcast,
            name: "DAB — VHF Band III",
            aliases: &["dab", "digital radio"],
            notes: Some(
                "BBC National DAB on block 12B (225.648 MHz), Digital One on 11D, and a dense \
                 layer of local multiplexes.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 380_000_000.0,
            stop_hz: 400_000_000.0,
            service: BandService::Mobile,
            name: "Airwave — emergency services",
            aliases: &["airwave", "tetra", "emergency services"],
            notes: Some("The national public-safety TETRA network."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 457_500_000.0,
            stop_hz: 464_000_000.0,
            service: BandService::Mobile,
            name: "Business radio — simple UK and simple light",
            aliases: &["business radio", "simple light", "site radio"],
            suggested: Some(nfm),
            channel_step_hz: Some(12_500.0),
            notes: Some(
                "Ofcom's shared business channels: taxis, security, events and building sites.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 470_000_000.0,
            stop_hz: 694_000_000.0,
            service: BandService::Broadcast,
            name: "UHF television — Freeview",
            aliases: &["freeview", "dvb-t2", "uhf tv"],
            channel_step_hz: Some(8_000_000.0),
            notes: Some("DVB-T2, channels 21–48 after the 700 MHz clearance."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 863_000_000.0,
            stop_hz: 865_000_000.0,
            service: BandService::Ism,
            name: "Wireless microphones — 863–865 MHz",
            aliases: &["radio mic", "wireless microphone", "in-ear"],
            notes: Some("Licence-exempt radio mics and in-ear monitors, 10 mW ERP."),
            ..Entry::ROW
        },
    ],
};
