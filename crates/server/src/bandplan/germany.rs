//! Germany — Bundesnetzagentur.
//!
//! A curated extract of the Frequenzplan, not a transcription of it: the entries here are the
//! ones a receiver in Germany actually lands on and that differ from the CEPT layer underneath.
//! German names travel as aliases so the explorer answers "UKW" and "BOS" as well as their
//! English equivalents — the UI itself is English until localization ships.
//!
//! Source: BNetzA Frequenzplan (Stand 2024) and the Allgemeinzuteilungen it references.

use sdrmm_wire::{BandLayerKind, BandService};

use super::{
    Entry, Layer,
    mode::{nfm, usb, wfm},
};

pub(super) static BNETZA: Layer = Layer {
    id: "de",
    name: "Germany — BNetzA",
    authority: "Bundesnetzagentur",
    source: "Frequenzplan (Stand 2024) and Allgemeinzuteilungen — curated extract",
    kind: BandLayerKind::Regulatory,
    entries: &[
        Entry {
            start_hz: 77_000.0,
            stop_hz: 78_000.0,
            service: BandService::Science,
            name: "DCF77 — time signal",
            aliases: &["dcf77", "time signal", "funkuhr", "radio clock"],
            notes: Some(
                "77.5 kHz from Mainflingen: the amplitude-keyed code every German radio clock \
                 listens to, one bit a second.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 26_565_000.0,
            stop_hz: 26_955_000.0,
            service: BandService::Mobile,
            name: "CB channels 41–80",
            aliases: &["cb", "cb funk", "citizens band"],
            suggested: Some(nfm),
            channel_step_hz: Some(10_000.0),
            notes: Some(
                "Germany licenses 80 CB channels rather than the CEPT 40; 41–80 are FM only.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 70_150_000.0,
            stop_hz: 70_180_000.0,
            service: BandService::Amateur,
            name: "4 m amateur",
            aliases: &["4 m", "70 mhz"],
            suggested: Some(usb),
            notes: Some(
                "A 30 kHz secondary allocation, CW and SSB only — a sliver next to the UK's \
                 500 kHz on the same band.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 75_200_000.0,
            stop_hz: 87_255_000.0,
            service: BandService::Mobile,
            name: "BOS 4 m band",
            aliases: &["bos", "4 m bos", "behördenfunk", "emergency services"],
            suggested: Some(nfm),
            channel_step_hz: Some(20_000.0),
            notes: Some(
                "Legacy analogue public-safety radio. Largely migrated to BOS-Digitalfunk in \
                 380–400 MHz, but fire brigades still run paging and fallback traffic here.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 87_500_000.0,
            stop_hz: 108_000_000.0,
            service: BandService::Broadcast,
            name: "UKW broadcast",
            aliases: &["ukw", "fm", "radio"],
            suggested: Some(wfm),
            channel_step_hz: Some(100_000.0),
            notes: Some("The public and private FM networks, with RDS on essentially all of them."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 165_210_000.0,
            stop_hz: 169_380_000.0,
            service: BandService::Mobile,
            name: "BOS 2 m band",
            aliases: &["bos", "2 m bos", "behördenfunk", "emergency services"],
            suggested: Some(nfm),
            channel_step_hz: Some(20_000.0),
            notes: Some("Analogue public-safety VHF, and where POCSAG fire paging still runs."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 174_000_000.0,
            stop_hz: 230_000_000.0,
            service: BandService::Broadcast,
            name: "DAB+ — VHF Band III",
            aliases: &["dab", "dab+", "digitalradio"],
            notes: Some(
                "The national ensemble sits on block 5C (178.352 MHz); the Länder ensembles \
                 fill the rest.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 380_000_000.0,
            stop_hz: 400_000_000.0,
            service: BandService::Mobile,
            name: "BOS-Digitalfunk (TETRA)",
            aliases: &["bos", "digitalfunk", "tetra", "emergency services"],
            notes: Some(
                "The national public-safety TETRA network. Encrypted, so a scope shows the \
                 carriers and nothing else.",
            ),
            ..Entry::ROW
        },
    ],
};
