//! United States — FCC.
//!
//! Sits directly on the ITU Region 2 layer, with no supranational layer between: there is no
//! American CEPT. Curated to what differs from the ITU table and is worth naming.
//!
//! Source: 47 CFR §2.106 (Table of Frequency Allocations) plus the service rules in Parts 15,
//! 73, 87, 95 and 97.

use sdrmm_wire::{BandLayerKind, BandService};

use super::{
    Entry, Layer,
    mode::{am, nfm, wfm},
};

pub(super) static FCC: Layer = Layer {
    id: "us",
    name: "United States — FCC",
    authority: "FCC",
    source: "47 CFR §2.106 and Parts 15/73/87/95/97 — curated extract",
    kind: BandLayerKind::Regulatory,
    entries: &[
        Entry {
            start_hz: 26_965_000.0,
            stop_hz: 27_405_000.0,
            service: BandService::Mobile,
            name: "Citizens Band — 40 channels",
            aliases: &["cb", "citizens band", "part 95"],
            suggested: Some(am),
            channel_step_hz: Some(10_000.0),
            notes: Some(
                "4 W AM carrier, 12 W PEP SSB. Channel 9 (27.065 MHz) is emergency, channel 19 \
                 (27.185 MHz) the highway channel.",
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
            channel_step_hz: Some(200_000.0),
            notes: Some(
                "200 kHz raster on odd tenths of a megahertz. 88.1–91.9 MHz is reserved for \
                 non-commercial educational stations.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 162_400_000.0,
            stop_hz: 162_550_000.0,
            service: BandService::Broadcast,
            name: "NOAA Weather Radio",
            aliases: &["noaa weather", "weather radio", "wx", "nws"],
            suggested: Some(nfm),
            channel_step_hz: Some(25_000.0),
            notes: Some(
                "Seven channels of continuous forecast voice, plus SAME alert tones that also \
                 drive the emergency alert system.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 174_000_000.0,
            stop_hz: 216_000_000.0,
            service: BandService::Broadcast,
            name: "VHF television — channels 7–13",
            aliases: &["vhf tv", "atsc", "television"],
            channel_step_hz: Some(6_000_000.0),
            notes: Some(
                "Region 2 uses Band III for television, where Region 1 put DAB — the same \
                 spectrum, a completely different sound on a scope.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 462_537_500.0,
            stop_hz: 462_737_500.0,
            service: BandService::Mobile,
            name: "FRS and GMRS — 462 MHz",
            aliases: &["frs", "gmrs", "walkie talkie", "part 95"],
            suggested: Some(nfm),
            channel_step_hz: Some(25_000.0),
            notes: Some(
                "Shared channels: FRS is licence-free at 2 W, GMRS needs a licence for the \
                 high-power and repeater channels.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 467_537_500.0,
            stop_hz: 467_737_500.0,
            service: BandService::Mobile,
            name: "FRS and GMRS — 467 MHz",
            aliases: &["frs", "gmrs", "walkie talkie", "part 95"],
            suggested: Some(nfm),
            channel_step_hz: Some(25_000.0),
            notes: Some("The interstitial FRS channels and the GMRS repeater inputs."),
            ..Entry::ROW
        },
    ],
};
