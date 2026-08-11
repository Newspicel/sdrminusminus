//! The ITU layers: one global backdrop plus the three regional refinements.
//!
//! [`GLOBAL`] is deliberately continuous from 9 kHz to 6 GHz. It is the layer that answers "what
//! is this?" everywhere, so a gap in it is a stretch of spectrum the ruler draws blank — and it
//! carries the bands the Radio Regulations allocate identically in all three regions, which is
//! most of HF and the aeronautical, maritime and satellite tables.
//!
//! The regional layers carry only what actually differs. That is not a shortcut: most-specific-
//! wins means a coarse regional row would erase a specific global one underneath it, so a
//! refinement must be narrower than what it refines (`mod.rs` asserts this).
//!
//! Source: ITU Radio Regulations, Article 5 (Edition of 2020), Table of Frequency Allocations.
//! Curated — the full table is thousands of rows of footnotes, and an operator wants the band's
//! name, not its footnote list.

use sdrmm_wire::{BandLayerKind, BandService};

use super::{
    Entry, Layer,
    mode::{adsb, ais, am, lsb, morse, navtex, nfm, subghz, usb, wfm},
};

pub(super) static GLOBAL: Layer = Layer {
    id: "world",
    name: "ITU world table",
    authority: "ITU",
    source: "Radio Regulations, Article 5 (Edition of 2020) — curated extract",
    kind: BandLayerKind::World,
    entries: GLOBAL_ENTRIES,
};

static GLOBAL_ENTRIES: &[Entry] = &[
    Entry {
        start_hz: 9_000.0,
        stop_hz: 135_700.0,
        service: BandService::Other,
        name: "VLF/LF fixed and maritime mobile",
        aliases: &["vlf", "very low frequency"],
        notes: Some(
            "The standard-time transmitters live here: WWVB and MSF on 60 kHz, DCF77 on \
             77.5 kHz. Most receivers need direct sampling or an upconverter to reach it.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 135_700.0,
        stop_hz: 137_800.0,
        service: BandService::Amateur,
        name: "2200 m amateur",
        aliases: &["2200 m", "136 khz", "amateur lf"],
        suggested: Some(morse),
        notes: Some("1 W EIRP in most administrations: CW, WSPR and other very slow modes."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 137_800.0,
        stop_hz: 283_500.0,
        service: BandService::Navigation,
        name: "LF radionavigation and fixed",
        aliases: &["longwave", "lf"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 283_500.0,
        stop_hz: 315_000.0,
        service: BandService::Navigation,
        name: "Maritime radio beacons and DGPS",
        aliases: &["dgps", "radio beacon"],
        notes: Some("Differential GPS corrections, MSK at 100–200 bit/s."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 315_000.0,
        stop_hz: 415_000.0,
        service: BandService::Navigation,
        name: "Aeronautical non-directional beacons",
        aliases: &["ndb", "non-directional beacon"],
        suggested: Some(morse),
        notes: Some("A keyed carrier repeating a two- or three-letter Morse ident."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 415_000.0,
        stop_hz: 472_000.0,
        service: BandService::Maritime,
        name: "Maritime mobile (MF)",
        aliases: &["maritime mf"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 472_000.0,
        stop_hz: 479_000.0,
        service: BandService::Amateur,
        name: "630 m amateur",
        aliases: &["630 m", "475 khz"],
        suggested: Some(morse),
        notes: Some("5 W EIRP in most administrations: CW, WSPR and FT8."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 479_000.0,
        stop_hz: 489_000.0,
        service: BandService::Maritime,
        name: "Maritime mobile (MF)",
        aliases: &["maritime mf"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 489_000.0,
        stop_hz: 491_000.0,
        service: BandService::Maritime,
        name: "NAVTEX — 490 kHz",
        aliases: &["navtex", "national navtex"],
        suggested: Some(navtex),
        channel_step_hz: None,
        notes: Some(
            "National-language navigational warnings. SITOR-B, 100 baud FSK with a 170 Hz shift.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 491_000.0,
        stop_hz: 517_000.0,
        service: BandService::Maritime,
        name: "Maritime mobile (MF)",
        aliases: &["maritime mf"],
        notes: Some("500 kHz was the international distress and calling frequency until 1999."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 517_000.0,
        stop_hz: 519_000.0,
        service: BandService::Maritime,
        name: "NAVTEX — 518 kHz",
        aliases: &["navtex", "international navtex"],
        suggested: Some(navtex),
        notes: Some("The international NAVTEX service, always in English. SITOR-B, 100 baud FSK."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 519_000.0,
        stop_hz: 526_500.0,
        service: BandService::Maritime,
        name: "Maritime mobile (MF)",
        aliases: &["maritime mf"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 526_500.0,
        stop_hz: 1_606_500.0,
        service: BandService::Broadcast,
        name: "Medium-wave broadcast",
        aliases: &["mw", "medium wave", "am broadcast", "am radio"],
        suggested: Some(am),
        channel_step_hz: Some(9_000.0),
        notes: Some("9 kHz raster in Regions 1 and 3. Groundwave by day, skywave after dark."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 1_606_500.0,
        stop_hz: 1_800_000.0,
        service: BandService::Maritime,
        name: "Maritime mobile (1.6 MHz)",
        aliases: &["maritime mf"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 1_800_000.0,
        stop_hz: 2_000_000.0,
        service: BandService::Amateur,
        name: "160 m amateur",
        aliases: &["160 m", "top band", "1.8 mhz"],
        suggested: Some(lsb),
        notes: Some("Lower sideband by convention. A night band: daytime absorption kills it."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 2_000_000.0,
        stop_hz: 2_300_000.0,
        service: BandService::Maritime,
        name: "Maritime mobile (2 MHz)",
        aliases: &["maritime mf"],
        suggested: Some(lsb),
        notes: Some("2182 kHz is the MF distress and calling frequency."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 2_300_000.0,
        stop_hz: 2_495_000.0,
        service: BandService::Broadcast,
        name: "120 m tropical broadcast",
        aliases: &["120 m", "tropical band"],
        suggested: Some(am),
        ..Entry::ROW
    },
    Entry {
        start_hz: 2_495_000.0,
        stop_hz: 2_505_000.0,
        service: BandService::Science,
        name: "Standard frequency and time — 2.5 MHz",
        aliases: &["time signal", "wwv", "standard frequency"],
        suggested: Some(am),
        ..Entry::ROW
    },
    Entry {
        start_hz: 2_505_000.0,
        stop_hz: 2_850_000.0,
        service: BandService::Other,
        name: "HF fixed and mobile",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 2_850_000.0,
        stop_hz: 3_155_000.0,
        service: BandService::Aeronautical,
        name: "HF aeronautical mobile (route)",
        aliases: &["hf air", "oceanic", "aeronautical hf"],
        suggested: Some(usb),
        notes: Some("Upper sideband. Long-haul oceanic and polar air traffic control."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 3_155_000.0,
        stop_hz: 3_200_000.0,
        service: BandService::Other,
        name: "HF fixed and mobile",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 3_200_000.0,
        stop_hz: 3_400_000.0,
        service: BandService::Broadcast,
        name: "90 m tropical broadcast",
        aliases: &["90 m", "tropical band"],
        suggested: Some(am),
        ..Entry::ROW
    },
    Entry {
        start_hz: 3_400_000.0,
        stop_hz: 3_500_000.0,
        service: BandService::Aeronautical,
        name: "HF aeronautical mobile (route)",
        aliases: &["hf air", "aeronautical hf"],
        suggested: Some(usb),
        ..Entry::ROW
    },
    Entry {
        start_hz: 3_500_000.0,
        stop_hz: 3_900_000.0,
        service: BandService::Other,
        name: "HF fixed, mobile and amateur (80 m)",
        aliases: &["80 m", "hf utility"],
        notes: Some(
            "How much of this is amateur depends on the ITU region: 3.5–3.8 MHz in Region 1, \
             3.5–4.0 in Region 2, 3.5–3.9 in Region 3.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 3_900_000.0,
        stop_hz: 4_000_000.0,
        service: BandService::Broadcast,
        name: "75 m broadcast",
        aliases: &["75 m"],
        suggested: Some(am),
        ..Entry::ROW
    },
    Entry {
        start_hz: 4_000_000.0,
        stop_hz: 4_063_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 4_063_000.0,
        stop_hz: 4_438_000.0,
        service: BandService::Maritime,
        name: "Maritime mobile (4 MHz)",
        aliases: &["maritime hf"],
        suggested: Some(usb),
        notes: Some("4207.5 kHz is the 4 MHz DSC calling channel."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 4_438_000.0,
        stop_hz: 4_650_000.0,
        service: BandService::Other,
        name: "HF fixed and mobile",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 4_650_000.0,
        stop_hz: 4_750_000.0,
        service: BandService::Aeronautical,
        name: "HF aeronautical mobile (route)",
        aliases: &["hf air", "aeronautical hf"],
        suggested: Some(usb),
        ..Entry::ROW
    },
    Entry {
        start_hz: 4_750_000.0,
        stop_hz: 5_060_000.0,
        service: BandService::Broadcast,
        name: "60 m tropical broadcast",
        aliases: &["60 m band", "tropical band"],
        suggested: Some(am),
        ..Entry::ROW
    },
    Entry {
        start_hz: 5_060_000.0,
        stop_hz: 5_351_500.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 5_351_500.0,
        stop_hz: 5_366_500.0,
        service: BandService::Amateur,
        name: "60 m amateur",
        aliases: &["60 m", "5 mhz amateur"],
        suggested: Some(usb),
        notes: Some(
            "The WRC-15 secondary allocation: 15 W EIRP, and a band the amateur service shares \
             with fixed and mobile users who were there first.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 5_366_500.0,
        stop_hz: 5_450_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 5_450_000.0,
        stop_hz: 5_730_000.0,
        service: BandService::Aeronautical,
        name: "HF aeronautical mobile (5 MHz)",
        aliases: &["hf air", "aeronautical hf"],
        suggested: Some(usb),
        ..Entry::ROW
    },
    Entry {
        start_hz: 5_730_000.0,
        stop_hz: 5_900_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 5_900_000.0,
        stop_hz: 6_200_000.0,
        service: BandService::Broadcast,
        name: "49 m shortwave broadcast",
        aliases: &["49 m", "shortwave", "sw broadcast"],
        suggested: Some(am),
        channel_step_hz: Some(5_000.0),
        ..Entry::ROW
    },
    Entry {
        start_hz: 6_200_000.0,
        stop_hz: 6_525_000.0,
        service: BandService::Maritime,
        name: "Maritime mobile (6 MHz)",
        aliases: &["maritime hf"],
        suggested: Some(usb),
        ..Entry::ROW
    },
    Entry {
        start_hz: 6_525_000.0,
        stop_hz: 6_765_000.0,
        service: BandService::Aeronautical,
        name: "HF aeronautical mobile (6 MHz)",
        aliases: &["hf air", "aeronautical hf"],
        suggested: Some(usb),
        ..Entry::ROW
    },
    Entry {
        start_hz: 6_765_000.0,
        stop_hz: 6_795_000.0,
        service: BandService::Ism,
        name: "ISM — 6.78 MHz",
        aliases: &["ism"],
        notes: Some("RR 5.138: industrial, scientific and medical."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 6_795_000.0,
        stop_hz: 7_000_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 7_000_000.0,
        stop_hz: 7_200_000.0,
        service: BandService::Amateur,
        name: "40 m amateur",
        aliases: &["40 m", "7 mhz"],
        suggested: Some(lsb),
        notes: Some(
            "Lower sideband. 7.2–7.3 MHz is amateur in Region 2 and shortwave broadcast \
             everywhere else, which is why 40 m sounds different depending on where you are.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 7_200_000.0,
        stop_hz: 7_450_000.0,
        service: BandService::Broadcast,
        name: "41 m shortwave broadcast",
        aliases: &["41 m", "shortwave"],
        suggested: Some(am),
        channel_step_hz: Some(5_000.0),
        ..Entry::ROW
    },
    Entry {
        start_hz: 7_450_000.0,
        stop_hz: 8_100_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 8_100_000.0,
        stop_hz: 8_815_000.0,
        service: BandService::Maritime,
        name: "Maritime mobile (8 MHz)",
        aliases: &["maritime hf"],
        suggested: Some(usb),
        notes: Some("8414.5 kHz is the 8 MHz DSC calling channel."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 8_815_000.0,
        stop_hz: 9_040_000.0,
        service: BandService::Aeronautical,
        name: "HF aeronautical mobile (8.8 MHz)",
        aliases: &["hf air", "aeronautical hf"],
        suggested: Some(usb),
        ..Entry::ROW
    },
    Entry {
        start_hz: 9_040_000.0,
        stop_hz: 9_400_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 9_400_000.0,
        stop_hz: 9_900_000.0,
        service: BandService::Broadcast,
        name: "31 m shortwave broadcast",
        aliases: &["31 m", "shortwave"],
        suggested: Some(am),
        channel_step_hz: Some(5_000.0),
        ..Entry::ROW
    },
    Entry {
        start_hz: 9_900_000.0,
        stop_hz: 9_995_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 9_995_000.0,
        stop_hz: 10_005_000.0,
        service: BandService::Science,
        name: "Standard frequency and time — 10 MHz",
        aliases: &["time signal", "wwv", "standard frequency"],
        suggested: Some(am),
        ..Entry::ROW
    },
    Entry {
        start_hz: 10_005_000.0,
        stop_hz: 10_100_000.0,
        service: BandService::Aeronautical,
        name: "HF aeronautical mobile (route)",
        aliases: &["hf air", "aeronautical hf"],
        suggested: Some(usb),
        ..Entry::ROW
    },
    Entry {
        start_hz: 10_100_000.0,
        stop_hz: 10_150_000.0,
        service: BandService::Amateur,
        name: "30 m amateur",
        aliases: &["30 m", "10 mhz"],
        suggested: Some(usb),
        notes: Some(
            "A WARC band: CW and narrow digital only by IARU convention, no contests, and \
             usually a 200 W ceiling.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 10_150_000.0,
        stop_hz: 11_175_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 11_175_000.0,
        stop_hz: 11_400_000.0,
        service: BandService::Aeronautical,
        name: "HF aeronautical mobile (off-route)",
        aliases: &["hf air", "hfgcs", "milair hf"],
        suggested: Some(usb),
        notes: Some("11175 kHz is the primary US Air Force HFGCS channel."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 11_400_000.0,
        stop_hz: 11_600_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 11_600_000.0,
        stop_hz: 12_100_000.0,
        service: BandService::Broadcast,
        name: "25 m shortwave broadcast",
        aliases: &["25 m", "shortwave"],
        suggested: Some(am),
        channel_step_hz: Some(5_000.0),
        ..Entry::ROW
    },
    Entry {
        start_hz: 12_100_000.0,
        stop_hz: 12_230_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 12_230_000.0,
        stop_hz: 13_200_000.0,
        service: BandService::Maritime,
        name: "Maritime mobile (12 MHz)",
        aliases: &["maritime hf"],
        suggested: Some(usb),
        ..Entry::ROW
    },
    Entry {
        start_hz: 13_200_000.0,
        stop_hz: 13_360_000.0,
        service: BandService::Aeronautical,
        name: "HF aeronautical mobile (13 MHz)",
        aliases: &["hf air", "aeronautical hf"],
        suggested: Some(usb),
        ..Entry::ROW
    },
    Entry {
        start_hz: 13_360_000.0,
        stop_hz: 13_410_000.0,
        service: BandService::Science,
        name: "Radio astronomy — 13.36 MHz",
        aliases: &["radio astronomy", "passive band"],
        notes: Some("A passive band: no emission is permitted."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 13_410_000.0,
        stop_hz: 13_553_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 13_553_000.0,
        stop_hz: 13_567_000.0,
        service: BandService::Ism,
        name: "ISM — 13.56 MHz",
        aliases: &["ism", "rfid", "nfc"],
        notes: Some("RR 5.150. NFC and HF RFID."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 13_567_000.0,
        stop_hz: 13_570_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 13_570_000.0,
        stop_hz: 13_870_000.0,
        service: BandService::Broadcast,
        name: "22 m shortwave broadcast",
        aliases: &["22 m", "shortwave"],
        suggested: Some(am),
        channel_step_hz: Some(5_000.0),
        ..Entry::ROW
    },
    Entry {
        start_hz: 13_870_000.0,
        stop_hz: 14_000_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 14_000_000.0,
        stop_hz: 14_350_000.0,
        service: BandService::Amateur,
        name: "20 m amateur",
        aliases: &["20 m", "14 mhz"],
        suggested: Some(usb),
        notes: Some(
            "Upper sideband above 10 MHz by convention. The band that is open somewhere \
             almost all the time.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 14_350_000.0,
        stop_hz: 14_990_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 14_990_000.0,
        stop_hz: 15_010_000.0,
        service: BandService::Science,
        name: "Standard frequency and time — 15 MHz",
        aliases: &["time signal", "wwv", "standard frequency"],
        suggested: Some(am),
        ..Entry::ROW
    },
    Entry {
        start_hz: 15_010_000.0,
        stop_hz: 15_100_000.0,
        service: BandService::Aeronautical,
        name: "HF aeronautical mobile (off-route)",
        aliases: &["hf air", "milair hf"],
        suggested: Some(usb),
        ..Entry::ROW
    },
    Entry {
        start_hz: 15_100_000.0,
        stop_hz: 15_800_000.0,
        service: BandService::Broadcast,
        name: "19 m shortwave broadcast",
        aliases: &["19 m", "shortwave"],
        suggested: Some(am),
        channel_step_hz: Some(5_000.0),
        ..Entry::ROW
    },
    Entry {
        start_hz: 15_800_000.0,
        stop_hz: 16_360_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 16_360_000.0,
        stop_hz: 17_410_000.0,
        service: BandService::Maritime,
        name: "Maritime mobile (16 MHz)",
        aliases: &["maritime hf"],
        suggested: Some(usb),
        ..Entry::ROW
    },
    Entry {
        start_hz: 17_410_000.0,
        stop_hz: 17_480_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 17_480_000.0,
        stop_hz: 17_900_000.0,
        service: BandService::Broadcast,
        name: "16 m shortwave broadcast",
        aliases: &["16 m", "shortwave"],
        suggested: Some(am),
        channel_step_hz: Some(5_000.0),
        ..Entry::ROW
    },
    Entry {
        start_hz: 17_900_000.0,
        stop_hz: 18_030_000.0,
        service: BandService::Aeronautical,
        name: "HF aeronautical mobile (17.9 MHz)",
        aliases: &["hf air", "aeronautical hf"],
        suggested: Some(usb),
        ..Entry::ROW
    },
    Entry {
        start_hz: 18_030_000.0,
        stop_hz: 18_068_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 18_068_000.0,
        stop_hz: 18_168_000.0,
        service: BandService::Amateur,
        name: "17 m amateur",
        aliases: &["17 m", "18 mhz"],
        suggested: Some(usb),
        notes: Some("A WARC band: no contesting by IARU convention, so it stays civil."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 18_168_000.0,
        stop_hz: 18_900_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 18_900_000.0,
        stop_hz: 19_020_000.0,
        service: BandService::Broadcast,
        name: "15 m shortwave broadcast",
        aliases: &["15 m broadcast", "shortwave"],
        suggested: Some(am),
        ..Entry::ROW
    },
    Entry {
        start_hz: 19_020_000.0,
        stop_hz: 19_990_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 19_990_000.0,
        stop_hz: 20_010_000.0,
        service: BandService::Science,
        name: "Standard frequency and time — 20 MHz",
        aliases: &["time signal", "wwv", "standard frequency"],
        suggested: Some(am),
        ..Entry::ROW
    },
    Entry {
        start_hz: 20_010_000.0,
        stop_hz: 21_000_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 21_000_000.0,
        stop_hz: 21_450_000.0,
        service: BandService::Amateur,
        name: "15 m amateur",
        aliases: &["15 m", "21 mhz"],
        suggested: Some(usb),
        notes: Some("A daytime DX band that follows the solar cycle closely."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 21_450_000.0,
        stop_hz: 21_850_000.0,
        service: BandService::Broadcast,
        name: "13 m shortwave broadcast",
        aliases: &["13 m", "shortwave"],
        suggested: Some(am),
        channel_step_hz: Some(5_000.0),
        ..Entry::ROW
    },
    Entry {
        start_hz: 21_850_000.0,
        stop_hz: 21_924_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 21_924_000.0,
        stop_hz: 22_000_000.0,
        service: BandService::Aeronautical,
        name: "HF aeronautical mobile (route)",
        aliases: &["hf air", "aeronautical hf"],
        suggested: Some(usb),
        ..Entry::ROW
    },
    Entry {
        start_hz: 22_000_000.0,
        stop_hz: 22_855_000.0,
        service: BandService::Maritime,
        name: "Maritime mobile (22 MHz)",
        aliases: &["maritime hf"],
        suggested: Some(usb),
        ..Entry::ROW
    },
    Entry {
        start_hz: 22_855_000.0,
        stop_hz: 23_200_000.0,
        service: BandService::Other,
        name: "HF fixed",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 23_200_000.0,
        stop_hz: 23_350_000.0,
        service: BandService::Aeronautical,
        name: "HF aeronautical mobile (off-route)",
        aliases: &["hf air", "milair hf"],
        suggested: Some(usb),
        ..Entry::ROW
    },
    Entry {
        start_hz: 23_350_000.0,
        stop_hz: 24_890_000.0,
        service: BandService::Other,
        name: "HF fixed and mobile",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 24_890_000.0,
        stop_hz: 24_990_000.0,
        service: BandService::Amateur,
        name: "12 m amateur",
        aliases: &["12 m", "24 mhz"],
        suggested: Some(usb),
        notes: Some("A WARC band, and the quietest of the high bands."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 24_990_000.0,
        stop_hz: 25_010_000.0,
        service: BandService::Science,
        name: "Standard frequency and time — 25 MHz",
        aliases: &["time signal", "wwv", "standard frequency"],
        suggested: Some(am),
        ..Entry::ROW
    },
    Entry {
        start_hz: 25_010_000.0,
        stop_hz: 25_070_000.0,
        service: BandService::Mobile,
        name: "Land mobile (25 MHz)",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 25_070_000.0,
        stop_hz: 25_210_000.0,
        service: BandService::Maritime,
        name: "Maritime mobile (25 MHz)",
        aliases: &["maritime hf"],
        suggested: Some(usb),
        ..Entry::ROW
    },
    Entry {
        start_hz: 25_210_000.0,
        stop_hz: 25_670_000.0,
        service: BandService::Other,
        name: "HF fixed and mobile",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 25_670_000.0,
        stop_hz: 26_100_000.0,
        service: BandService::Broadcast,
        name: "11 m shortwave broadcast",
        aliases: &["11 m broadcast", "shortwave"],
        suggested: Some(am),
        ..Entry::ROW
    },
    Entry {
        start_hz: 26_100_000.0,
        stop_hz: 26_957_000.0,
        service: BandService::Other,
        name: "HF fixed and mobile",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 26_957_000.0,
        stop_hz: 27_283_000.0,
        service: BandService::Ism,
        name: "ISM — 27 MHz",
        aliases: &["ism", "27 mhz"],
        notes: Some(
            "RR 5.150. The Citizens' Band channel plans sit inside it, as does a great deal of \
             radio control and industrial heating.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 27_283_000.0,
        stop_hz: 28_000_000.0,
        service: BandService::Other,
        name: "HF fixed and mobile",
        aliases: &["hf utility"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 28_000_000.0,
        stop_hz: 29_700_000.0,
        service: BandService::Amateur,
        name: "10 m amateur",
        aliases: &["10 m", "28 mhz"],
        suggested: Some(usb),
        notes: Some(
            "Dead at solar minimum and worldwide at maximum. 29.6 MHz is the FM calling channel.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 29_700_000.0,
        stop_hz: 40_660_000.0,
        service: BandService::Mobile,
        name: "VHF low-band land mobile",
        aliases: &["low band", "vhf low"],
        suggested: Some(nfm),
        notes: Some(
            "Long-range fleet, utility and forestry radio. Sporadic-E carries it across a \
             continent for weeks at a time in early summer.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 40_660_000.0,
        stop_hz: 40_700_000.0,
        service: BandService::Ism,
        name: "ISM — 40.68 MHz",
        aliases: &["ism", "radio control"],
        notes: Some("RR 5.150. Model control and low-power telemetry."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 40_700_000.0,
        stop_hz: 47_000_000.0,
        service: BandService::Mobile,
        name: "VHF low-band land mobile",
        aliases: &["low band", "vhf low"],
        suggested: Some(nfm),
        ..Entry::ROW
    },
    Entry {
        start_hz: 47_000_000.0,
        stop_hz: 68_000_000.0,
        service: BandService::Other,
        name: "VHF fixed, mobile and broadcast",
        aliases: &["band i", "vhf low"],
        notes: Some(
            "The old television Band I in Region 1; 50–54 MHz is the 6 m amateur band in \
             Regions 2 and 3, and 50–52 MHz in most of Region 1 since WRC-19.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 68_000_000.0,
        stop_hz: 74_800_000.0,
        service: BandService::Mobile,
        name: "VHF land mobile",
        aliases: &["vhf mobile"],
        suggested: Some(nfm),
        ..Entry::ROW
    },
    Entry {
        start_hz: 74_800_000.0,
        stop_hz: 75_200_000.0,
        service: BandService::Aeronautical,
        name: "Marker beacons — 75 MHz",
        aliases: &["marker beacon", "outer marker"],
        suggested: Some(am),
        notes: Some("Airport marker beacons, keyed at 400, 1300 or 3000 Hz."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 75_200_000.0,
        stop_hz: 87_500_000.0,
        service: BandService::Mobile,
        name: "VHF land mobile",
        aliases: &["vhf mobile"],
        suggested: Some(nfm),
        ..Entry::ROW
    },
    Entry {
        start_hz: 87_500_000.0,
        stop_hz: 108_000_000.0,
        service: BandService::Broadcast,
        name: "FM broadcast",
        aliases: &["fm", "broadcast fm", "band ii", "ukw"],
        suggested: Some(wfm),
        channel_step_hz: Some(100_000.0),
        notes: Some(
            "±75 kHz deviation, a 19 kHz stereo pilot, and RDS on a 57 kHz subcarrier. \
             Region 2 starts at 88.0 MHz with a 200 kHz raster.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 108_000_000.0,
        stop_hz: 117_975_000.0,
        service: BandService::Navigation,
        name: "VOR and ILS localizer",
        aliases: &["vor", "ils", "localizer"],
        suggested: Some(am),
        channel_step_hz: Some(50_000.0),
        notes: Some("Each carries a Morse ident you can read straight off the AM audio."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 117_975_000.0,
        stop_hz: 137_000_000.0,
        service: BandService::Aeronautical,
        name: "Airband — civil aviation voice",
        aliases: &["airband", "air band", "aircraft", "atc", "tower"],
        suggested: Some(am),
        channel_step_hz: Some(25_000.0),
        notes: Some(
            "AM, so a stronger station does not capture a weaker one. 25 kHz spacing, 8.33 kHz \
             across much of Europe. 121.5 MHz is the emergency frequency and is normally silent.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 137_000_000.0,
        stop_hz: 138_000_000.0,
        service: BandService::Satellite,
        name: "Meteorological satellite downlink",
        aliases: &["noaa apt", "weather satellite", "meteor", "apt", "lrpt"],
        suggested: Some(wfm),
        notes: Some(
            "NOAA APT at 137.100, 137.620 and 137.9125 MHz; Meteor-M LRPT nearby. \
             Right-hand circular polarisation, so a linear antenna loses 3 dB.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 138_000_000.0,
        stop_hz: 144_000_000.0,
        service: BandService::Mobile,
        name: "VHF land and government mobile",
        aliases: &["vhf mobile"],
        suggested: Some(nfm),
        ..Entry::ROW
    },
    Entry {
        start_hz: 144_000_000.0,
        stop_hz: 146_000_000.0,
        service: BandService::Amateur,
        name: "2 m amateur",
        aliases: &["2 m", "144 mhz", "two metres", "vhf amateur"],
        suggested: Some(nfm),
        channel_step_hz: Some(12_500.0),
        notes: Some(
            "145.500 MHz is the Region 1 FM calling channel and 144.800 MHz carries APRS. \
             Regions 2 and 3 have 2 MHz more, up to 148 MHz.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 146_000_000.0,
        stop_hz: 156_000_000.0,
        service: BandService::Mobile,
        name: "VHF land mobile",
        aliases: &["vhf mobile"],
        suggested: Some(nfm),
        ..Entry::ROW
    },
    Entry {
        start_hz: 156_000_000.0,
        stop_hz: 161_962_500.0,
        service: BandService::Maritime,
        name: "Marine VHF",
        aliases: &["marine vhf", "marine", "vhf marine", "boat", "channel 16"],
        suggested: Some(nfm),
        channel_step_hz: Some(25_000.0),
        notes: Some(
            "Channel 16 (156.800 MHz) is distress, safety and calling; channel 70 \
             (156.525 MHz) is DSC data only and carries no voice.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 161_962_500.0,
        stop_hz: 162_037_500.0,
        service: BandService::Maritime,
        name: "AIS — 161.975 and 162.025 MHz",
        aliases: &["ais", "ship tracking", "vessel tracking"],
        suggested: Some(ais),
        notes: Some(
            "AIS 1 and AIS 2, 9600 baud GMSK in 25 kHz channels. Both are decoded at once if \
             the receiver's span covers them.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 162_037_500.0,
        stop_hz: 174_000_000.0,
        service: BandService::Mobile,
        name: "VHF land mobile and government",
        aliases: &["vhf mobile"],
        suggested: Some(nfm),
        ..Entry::ROW
    },
    Entry {
        start_hz: 174_000_000.0,
        stop_hz: 230_000_000.0,
        service: BandService::Broadcast,
        name: "VHF Band III — DAB and television",
        aliases: &["band iii", "dab", "dab+", "digital radio"],
        notes: Some(
            "DAB+ ensembles occupy 1.536 MHz each on the 5A–13F block raster in Region 1; \
             Region 2 uses the band for television channels 7–13.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 230_000_000.0,
        stop_hz: 328_600_000.0,
        service: BandService::Aeronautical,
        name: "UHF military air band",
        aliases: &["milair", "uhf milair", "military air"],
        suggested: Some(am),
        channel_step_hz: Some(25_000.0),
        notes: Some(
            "AM like the civil band, running 225–400 MHz in most administrations. \
             243.0 MHz is the military emergency frequency.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 328_600_000.0,
        stop_hz: 335_400_000.0,
        service: BandService::Navigation,
        name: "ILS glide path",
        aliases: &["glideslope", "glide path", "ils"],
        notes: Some("Paired with a localizer in 108–112 MHz; the pairing is fixed by ICAO."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 335_400_000.0,
        stop_hz: 399_900_000.0,
        service: BandService::Aeronautical,
        name: "UHF military air band",
        aliases: &["milair", "uhf milair", "military air"],
        suggested: Some(am),
        channel_step_hz: Some(25_000.0),
        ..Entry::ROW
    },
    Entry {
        start_hz: 399_900_000.0,
        stop_hz: 401_000_000.0,
        service: BandService::Satellite,
        name: "Satellite radionavigation and meteorological satellite",
        aliases: &["argos", "satellite navigation"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 401_000_000.0,
        stop_hz: 406_000_000.0,
        service: BandService::Science,
        name: "Meteorological aids — radiosondes",
        aliases: &["radiosonde", "weather balloon", "rs41", "sonde"],
        notes: Some(
            "Vaisala RS41 and Graw DFM sondes transmit GFSK here twice a day from every \
             upper-air station in the world.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 406_000_000.0,
        stop_hz: 406_100_000.0,
        service: BandService::Satellite,
        name: "Emergency beacons — 406 MHz",
        aliases: &["epirb", "elt", "plb", "cospas-sarsat", "distress beacon"],
        notes: Some(
            "COSPAS-SARSAT distress beacons. Listening is legal everywhere; transmitting \
             anything here is not.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 406_100_000.0,
        stop_hz: 410_000_000.0,
        service: BandService::Science,
        name: "Radio astronomy — 406 MHz",
        aliases: &["radio astronomy"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 410_000_000.0,
        stop_hz: 430_000_000.0,
        service: BandService::Mobile,
        name: "UHF land mobile",
        aliases: &["uhf mobile", "pmr"],
        suggested: Some(nfm),
        ..Entry::ROW
    },
    Entry {
        start_hz: 430_000_000.0,
        stop_hz: 440_000_000.0,
        service: BandService::Amateur,
        name: "70 cm amateur",
        aliases: &["70 cm", "432 mhz", "uhf amateur"],
        suggested: Some(nfm),
        channel_step_hz: Some(12_500.0),
        notes: Some(
            "Allocated to the amateur service in all three regions; Regions 2 and 3 extend it \
             to 420–450 MHz. Shared with ISM devices in Region 1.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 440_000_000.0,
        stop_hz: 470_000_000.0,
        service: BandService::Mobile,
        name: "UHF land mobile",
        aliases: &["uhf mobile", "pmr", "business radio"],
        suggested: Some(nfm),
        ..Entry::ROW
    },
    Entry {
        start_hz: 470_000_000.0,
        stop_hz: 694_000_000.0,
        service: BandService::Broadcast,
        name: "UHF television",
        aliases: &["uhf tv", "dvb-t", "atsc", "television"],
        channel_step_hz: Some(8_000_000.0),
        notes: Some("8 MHz channels in Regions 1 and 3, 6 MHz in Region 2."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 694_000_000.0,
        stop_hz: 790_000_000.0,
        service: BandService::Mobile,
        name: "700 MHz mobile broadband",
        aliases: &["lte 700", "band 28", "5g"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 790_000_000.0,
        stop_hz: 862_000_000.0,
        service: BandService::Mobile,
        name: "800 MHz mobile broadband",
        aliases: &["lte 800", "band 20"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 862_000_000.0,
        stop_hz: 890_000_000.0,
        service: BandService::Mobile,
        name: "Land mobile and short-range devices (860–890 MHz)",
        aliases: &["srd", "uhf mobile"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 890_000_000.0,
        stop_hz: 960_000_000.0,
        service: BandService::Mobile,
        name: "900 MHz cellular",
        aliases: &["gsm 900", "band 8", "cellular"],
        notes: Some("GSM/LTE in Region 1; Region 2 puts the 902–928 MHz ISM band here instead."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 960_000_000.0,
        stop_hz: 1_089_000_000.0,
        service: BandService::Aeronautical,
        name: "DME and TACAN",
        aliases: &["dme", "tacan", "distance measuring"],
        channel_step_hz: Some(1_000_000.0),
        notes: Some("Pulse pairs on a 1 MHz raster, paired with a VOR or ILS."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 1_089_000_000.0,
        stop_hz: 1_091_000_000.0,
        service: BandService::Aeronautical,
        name: "ADS-B and Mode S — 1090 MHz",
        aliases: &["ads-b", "adsb", "mode s", "1090", "aircraft", "transponder"],
        suggested: Some(adsb),
        notes: Some(
            "Transponder replies and extended squitter, 1 Mbit/s PPM. Needs the receiver's own \
             samples at 2 Msps or more, so the channel takes IQ rather than a narrow passband.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 1_091_000_000.0,
        stop_hz: 1_164_000_000.0,
        service: BandService::Aeronautical,
        name: "DME and TACAN",
        aliases: &["dme", "tacan"],
        channel_step_hz: Some(1_000_000.0),
        ..Entry::ROW
    },
    Entry {
        start_hz: 1_164_000_000.0,
        stop_hz: 1_240_000_000.0,
        service: BandService::Navigation,
        name: "GNSS — L5, E5 and L2",
        aliases: &["gps", "gnss", "galileo", "glonass", "beidou"],
        notes: Some("GPS L5 at 1176.45 MHz, Galileo E5, GLONASS L2 around 1246 MHz."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 1_240_000_000.0,
        stop_hz: 1_300_000_000.0,
        service: BandService::Amateur,
        name: "23 cm amateur",
        aliases: &["23 cm", "1296", "1.2 ghz"],
        notes: Some(
            "Secondary to radionavigation, and increasingly squeezed by Galileo. Narrowband \
             work clusters around 1296 MHz.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 1_300_000_000.0,
        stop_hz: 1_400_000_000.0,
        service: BandService::Navigation,
        name: "L-band air-route surveillance radar",
        aliases: &["radar", "arsr"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 1_400_000_000.0,
        stop_hz: 1_427_000_000.0,
        service: BandService::Science,
        name: "Radio astronomy — hydrogen line",
        aliases: &["hydrogen line", "21 cm", "h1", "radio astronomy"],
        notes: Some(
            "Neutral hydrogen radiates at 1420.406 MHz. A passive band worldwide: no emission \
             is permitted, which is why it is quiet enough to hear the galaxy in.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 1_427_000_000.0,
        stop_hz: 1_518_000_000.0,
        service: BandService::Mobile,
        name: "L-band fixed and mobile",
        aliases: &["l band"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 1_518_000_000.0,
        stop_hz: 1_559_000_000.0,
        service: BandService::Satellite,
        name: "Mobile satellite downlink — Inmarsat",
        aliases: &["inmarsat", "aero", "std-c", "satcom"],
        notes: Some("1544–1545 MHz is reserved for distress and safety traffic."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 1_559_000_000.0,
        stop_hz: 1_610_000_000.0,
        service: BandService::Navigation,
        name: "GNSS — L1 and E1",
        aliases: &["gps", "gps l1", "gnss", "galileo", "glonass"],
        notes: Some("GPS L1 at 1575.42 MHz, GLONASS L1 around 1602 MHz."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 1_610_000_000.0,
        stop_hz: 1_626_500_000.0,
        service: BandService::Satellite,
        name: "Mobile satellite — Iridium",
        aliases: &["iridium", "satcom"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 1_626_500_000.0,
        stop_hz: 1_710_000_000.0,
        service: BandService::Satellite,
        name: "Mobile satellite uplink",
        aliases: &["satcom", "inmarsat"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 1_710_000_000.0,
        stop_hz: 1_880_000_000.0,
        service: BandService::Mobile,
        name: "1800 MHz mobile broadband",
        aliases: &["gsm 1800", "dcs", "band 3", "lte"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 1_880_000_000.0,
        stop_hz: 1_920_000_000.0,
        service: BandService::Mobile,
        name: "DECT",
        aliases: &["dect", "cordless phone"],
        channel_step_hz: Some(1_728_000.0),
        notes: Some("Cordless telephony: ten 1.728 MHz carriers, TDMA."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 1_920_000_000.0,
        stop_hz: 2_170_000_000.0,
        service: BandService::Mobile,
        name: "2100 MHz mobile broadband",
        aliases: &["umts", "band 1", "3g", "lte"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 2_170_000_000.0,
        stop_hz: 2_300_000_000.0,
        service: BandService::Satellite,
        name: "S-band space operation and telemetry",
        aliases: &["s band", "cubesat", "telemetry", "space research"],
        notes: Some("2200–2290 MHz is the space-research downlink most cubesats use."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 2_300_000_000.0,
        stop_hz: 2_400_000_000.0,
        service: BandService::Mobile,
        name: "S-band fixed and mobile",
        aliases: &["s band"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 2_400_000_000.0,
        stop_hz: 2_500_000_000.0,
        service: BandService::Ism,
        name: "ISM — 2.4 GHz",
        aliases: &["wifi", "wi-fi", "bluetooth", "zigbee", "2.4 ghz", "ism"],
        channel_step_hz: Some(5_000_000.0),
        notes: Some(
            "RR 5.150. Wi-Fi, Bluetooth, Zigbee and microwave ovens, and the lower half is also \
             the 13 cm amateur band.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 2_500_000_000.0,
        stop_hz: 2_690_000_000.0,
        service: BandService::Mobile,
        name: "2.6 GHz mobile broadband",
        aliases: &["band 7", "lte 2600"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 2_690_000_000.0,
        stop_hz: 2_700_000_000.0,
        service: BandService::Science,
        name: "Radio astronomy — 2.69 GHz",
        aliases: &["radio astronomy", "passive band"],
        notes: Some("A passive band: no emission is permitted."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 2_700_000_000.0,
        stop_hz: 3_100_000_000.0,
        service: BandService::Navigation,
        name: "S-band radar",
        aliases: &["radar", "asr", "weather radar"],
        notes: Some("Airport surveillance and weather radar."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 3_100_000_000.0,
        stop_hz: 3_400_000_000.0,
        service: BandService::Navigation,
        name: "Radiolocation — 3 GHz",
        aliases: &["radar", "radiolocation"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 3_400_000_000.0,
        stop_hz: 3_800_000_000.0,
        service: BandService::Mobile,
        name: "3.5 GHz mobile broadband",
        aliases: &["5g", "c-band", "n78", "cbrs"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 3_800_000_000.0,
        stop_hz: 4_200_000_000.0,
        service: BandService::Satellite,
        name: "C-band satellite downlink",
        aliases: &["c band", "satellite tv"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 4_200_000_000.0,
        stop_hz: 4_400_000_000.0,
        service: BandService::Aeronautical,
        name: "Radio altimeters",
        aliases: &["radalt", "radio altimeter"],
        notes: Some(
            "Aircraft radio altimeters — the band whose neighbours the 5G C-band rollout \
             argued about.",
        ),
        ..Entry::ROW
    },
    Entry {
        start_hz: 4_400_000_000.0,
        stop_hz: 4_990_000_000.0,
        service: BandService::Other,
        name: "Fixed and mobile (4.4–5.0 GHz)",
        aliases: &["fixed link"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 4_990_000_000.0,
        stop_hz: 5_000_000_000.0,
        service: BandService::Science,
        name: "Radio astronomy — 4.99 GHz",
        aliases: &["radio astronomy"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 5_000_000_000.0,
        stop_hz: 5_150_000_000.0,
        service: BandService::Aeronautical,
        name: "Microwave landing system",
        aliases: &["mls", "aeronautical"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 5_150_000_000.0,
        stop_hz: 5_350_000_000.0,
        service: BandService::Ism,
        name: "Wi-Fi 5 GHz — lower",
        aliases: &["wifi", "wi-fi", "u-nii-1", "u-nii-2", "5 ghz"],
        channel_step_hz: Some(20_000_000.0),
        ..Entry::ROW
    },
    Entry {
        start_hz: 5_350_000_000.0,
        stop_hz: 5_470_000_000.0,
        service: BandService::Navigation,
        name: "Radiolocation — 5.4 GHz",
        aliases: &["radar", "radiolocation"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 5_470_000_000.0,
        stop_hz: 5_725_000_000.0,
        service: BandService::Ism,
        name: "Wi-Fi 5 GHz — upper",
        aliases: &["wifi", "wi-fi", "u-nii-2c", "dfs", "5 ghz"],
        channel_step_hz: Some(20_000_000.0),
        notes: Some("Dynamic frequency selection is mandatory: the radar here has priority."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 5_725_000_000.0,
        stop_hz: 5_875_000_000.0,
        service: BandService::Ism,
        name: "ISM — 5.8 GHz",
        aliases: &["wifi", "5.8 ghz", "fpv", "ism", "6 cm"],
        notes: Some("RR 5.150. Also the 6 cm amateur band and where analogue FPV video lives."),
        ..Entry::ROW
    },
    Entry {
        start_hz: 5_875_000_000.0,
        stop_hz: 5_925_000_000.0,
        service: BandService::Mobile,
        name: "Intelligent transport systems",
        aliases: &["its-g5", "c-v2x", "v2x"],
        ..Entry::ROW
    },
    Entry {
        start_hz: 5_925_000_000.0,
        stop_hz: 6_000_000_000.0,
        service: BandService::Ism,
        name: "Wi-Fi 6 GHz",
        aliases: &["wifi 6e", "wi-fi 6e", "u-nii-5"],
        ..Entry::ROW
    },
];

pub(super) static ITU_R1: Layer = Layer {
    id: "itu-r1",
    name: "ITU Region 1",
    authority: "ITU",
    source: "Radio Regulations, Article 5 — Region 1 column",
    kind: BandLayerKind::World,
    entries: &[
        Entry {
            start_hz: 148_500.0,
            stop_hz: 283_500.0,
            service: BandService::Broadcast,
            name: "Long-wave broadcast",
            aliases: &["lw", "long wave", "langwelle"],
            suggested: Some(am),
            channel_step_hz: Some(9_000.0),
            notes: Some(
                "A Region 1 speciality. 9 kHz raster; several transmitters also carry a time \
                 code, as 162 kHz did for France.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 1_810_000.0,
            stop_hz: 2_000_000.0,
            service: BandService::Amateur,
            name: "160 m amateur (Region 1)",
            aliases: &["160 m", "top band"],
            suggested: Some(lsb),
            notes: Some("Region 1 starts at 1810 kHz, not 1800."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 3_500_000.0,
            stop_hz: 3_800_000.0,
            service: BandService::Amateur,
            name: "80 m amateur (Region 1)",
            aliases: &["80 m", "3.5 mhz"],
            suggested: Some(lsb),
            notes: Some("300 kHz here against 500 kHz in Region 2; 3.8–3.9 MHz is broadcast."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 50_000_000.0,
            stop_hz: 52_000_000.0,
            service: BandService::Amateur,
            name: "6 m amateur (Region 1)",
            aliases: &["6 m", "50 mhz", "magic band"],
            suggested: Some(usb),
            notes: Some(
                "WRC-19 harmonised 50–52 MHz across Region 1; some administrations allow up to \
                 54 MHz. Sporadic-E turns it into a worldwide band for hours at a time.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 65_800_000.0,
            stop_hz: 74_000_000.0,
            service: BandService::Broadcast,
            name: "OIRT FM broadcast",
            aliases: &["oirt", "fm oirt", "eastern fm"],
            suggested: Some(wfm),
            notes: Some("The legacy Eastern-European FM band; still on air in a few places."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 433_050_000.0,
            stop_hz: 434_790_000.0,
            service: BandService::Ism,
            name: "ISM — 433 MHz",
            aliases: &["433", "433 mhz", "ism 433", "srd", "remote control"],
            suggested: Some(subghz),
            notes: Some(
                "RR 5.138, Region 1 only. Car keys, tyre-pressure sensors, weather stations, \
                 doorbells — and it sits on top of the 70 cm amateur band.",
            ),
            ..Entry::ROW
        },
        Entry {
            start_hz: 890_000_000.0,
            stop_hz: 915_000_000.0,
            service: BandService::Mobile,
            name: "GSM 900 uplink",
            aliases: &["gsm900", "band 8 uplink"],
            notes: Some("Handset to base station. Where Region 2 has its 915 MHz ISM band."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 925_000_000.0,
            stop_hz: 960_000_000.0,
            service: BandService::Mobile,
            name: "GSM 900 downlink",
            aliases: &["gsm900", "band 8 downlink"],
            ..Entry::ROW
        },
        Entry {
            start_hz: 2_300_000_000.0,
            stop_hz: 2_400_000_000.0,
            service: BandService::Amateur,
            name: "13 cm amateur (Region 1)",
            aliases: &["13 cm", "2.3 ghz"],
            notes: Some("Secondary, and shared with the 2.4 GHz ISM band above 2400 MHz."),
            ..Entry::ROW
        },
    ],
};

pub(super) static ITU_R2: Layer = Layer {
    id: "itu-r2",
    name: "ITU Region 2",
    authority: "ITU",
    source: "Radio Regulations, Article 5 — Region 2 column",
    kind: BandLayerKind::World,
    entries: &[
        Entry {
            start_hz: 525_000.0,
            stop_hz: 1_705_000.0,
            service: BandService::Broadcast,
            name: "Medium-wave broadcast (Region 2)",
            aliases: &["mw", "am broadcast", "am radio"],
            suggested: Some(am),
            channel_step_hz: Some(10_000.0),
            notes: Some("10 kHz raster, and the band runs 100 kHz higher than elsewhere."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 3_500_000.0,
            stop_hz: 4_000_000.0,
            service: BandService::Amateur,
            name: "80/75 m amateur (Region 2)",
            aliases: &["80 m", "75 m"],
            suggested: Some(lsb),
            notes: Some("A full 500 kHz, with the SSB end above 3.8 MHz."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 7_200_000.0,
            stop_hz: 7_300_000.0,
            service: BandService::Amateur,
            name: "40 m amateur (Region 2 extension)",
            aliases: &["40 m"],
            suggested: Some(lsb),
            notes: Some("Shortwave broadcast to the rest of the world, amateur here."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 50_000_000.0,
            stop_hz: 54_000_000.0,
            service: BandService::Amateur,
            name: "6 m amateur (Region 2)",
            aliases: &["6 m", "magic band"],
            suggested: Some(usb),
            ..Entry::ROW
        },
        Entry {
            start_hz: 146_000_000.0,
            stop_hz: 148_000_000.0,
            service: BandService::Amateur,
            name: "2 m amateur (Region 2 extension)",
            aliases: &["2 m"],
            suggested: Some(nfm),
            channel_step_hz: Some(15_000.0),
            ..Entry::ROW
        },
        Entry {
            start_hz: 420_000_000.0,
            stop_hz: 430_000_000.0,
            service: BandService::Amateur,
            name: "70 cm amateur (Region 2, lower)",
            aliases: &["70 cm", "440"],
            suggested: Some(nfm),
            ..Entry::ROW
        },
        Entry {
            start_hz: 440_000_000.0,
            stop_hz: 450_000_000.0,
            service: BandService::Amateur,
            name: "70 cm amateur (Region 2, upper)",
            aliases: &["70 cm", "440"],
            suggested: Some(nfm),
            channel_step_hz: Some(25_000.0),
            ..Entry::ROW
        },
        Entry {
            start_hz: 470_000_000.0,
            stop_hz: 608_000_000.0,
            service: BandService::Broadcast,
            name: "UHF television (Region 2)",
            aliases: &["uhf tv", "atsc"],
            channel_step_hz: Some(6_000_000.0),
            notes: Some("6 MHz channels, 14–36 after the repack."),
            ..Entry::ROW
        },
        Entry {
            start_hz: 614_000_000.0,
            stop_hz: 698_000_000.0,
            service: BandService::Broadcast,
            name: "UHF television (Region 2, upper)",
            aliases: &["uhf tv", "atsc"],
            channel_step_hz: Some(6_000_000.0),
            ..Entry::ROW
        },
        Entry {
            start_hz: 902_000_000.0,
            stop_hz: 928_000_000.0,
            service: BandService::Ism,
            name: "ISM — 915 MHz",
            aliases: &["915", "ism 915", "33 cm", "lora", "srd"],
            suggested: Some(subghz),
            notes: Some(
                "RR 5.150 in Region 2, and the reason 915 MHz hardware does not work in Europe. \
                 Also the 33 cm amateur band.",
            ),
            ..Entry::ROW
        },
    ],
};

pub(super) static ITU_R3: Layer = Layer {
    id: "itu-r3",
    name: "ITU Region 3",
    authority: "ITU",
    source: "Radio Regulations, Article 5 — Region 3 column",
    kind: BandLayerKind::World,
    entries: &[
        Entry {
            start_hz: 3_500_000.0,
            stop_hz: 3_900_000.0,
            service: BandService::Amateur,
            name: "80 m amateur (Region 3)",
            aliases: &["80 m"],
            suggested: Some(lsb),
            ..Entry::ROW
        },
        Entry {
            start_hz: 50_000_000.0,
            stop_hz: 54_000_000.0,
            service: BandService::Amateur,
            name: "6 m amateur (Region 3)",
            aliases: &["6 m", "magic band"],
            suggested: Some(usb),
            ..Entry::ROW
        },
        Entry {
            start_hz: 146_000_000.0,
            stop_hz: 148_000_000.0,
            service: BandService::Amateur,
            name: "2 m amateur (Region 3 extension)",
            aliases: &["2 m"],
            suggested: Some(nfm),
            ..Entry::ROW
        },
        Entry {
            start_hz: 420_000_000.0,
            stop_hz: 430_000_000.0,
            service: BandService::Amateur,
            name: "70 cm amateur (Region 3, lower)",
            aliases: &["70 cm"],
            suggested: Some(nfm),
            ..Entry::ROW
        },
        Entry {
            start_hz: 440_000_000.0,
            stop_hz: 450_000_000.0,
            service: BandService::Amateur,
            name: "70 cm amateur (Region 3, upper)",
            aliases: &["70 cm"],
            suggested: Some(nfm),
            ..Entry::ROW
        },
    ],
};
