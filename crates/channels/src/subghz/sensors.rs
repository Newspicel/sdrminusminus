mod payload;

use sdrmm_wire::{SubghzEncoding, SubghzReading};

use self::payload::{PAYLOAD_BYTES, Payload};
use super::slicer::{Coding, Framing, Recode, recode, slice};

pub(super) struct Match {
    pub encoding: SubghzEncoding,
    pub bits: Vec<bool>,
    pub reading: SubghzReading,
    pub short_us: u32,
    pub repeats: u32,
}

enum Layout {
    Row {
        min_bits: usize,
        max_bits: usize,
        skip_bits: usize,
    },
    After {
        sync: u32,
        sync_bits: usize,
        bits: usize,
        recode: Recode,
    },
}

struct Device {
    framing: Framing,
    layout: Layout,
    invert: bool,
    read: fn(&Payload) -> Option<SubghzReading>,
}

const fn ppm(
    short_us: u32,
    long_us: u32,
    gap_us: u32,
    reset_us: u32,
    tolerance_us: u32,
) -> Framing {
    Framing {
        coding: Coding::Ppm { short_us, long_us },
        gap_us,
        reset_us,
        tolerance_us,
    }
}

const fn row(min_bits: usize, max_bits: usize) -> Layout {
    Layout::Row {
        min_bits,
        max_bits,
        skip_bits: 0,
    }
}

const fn pwm(
    short_us: u32,
    long_us: u32,
    sync_us: u32,
    gap_us: u32,
    reset_us: u32,
    tolerance_us: u32,
) -> Framing {
    Framing {
        coding: Coding::Pwm {
            short_us,
            long_us,
            sync_us,
        },
        gap_us,
        reset_us,
        tolerance_us,
    }
}

const SENSORS: &[Device] = &[
    Device {
        framing: Framing {
            coding: Coding::Pcm {
                short_us: 56,
                long_us: 56,
            },
            gap_us: 1_800,
            reset_us: 1_500,
            tolerance_us: 0,
        },
        layout: Layout::After {
            sync: 0x00AA_2DD4,
            sync_bits: 24,
            bits: 56,
            recode: Recode::None,
        },
        invert: false,
        read: payload::ambientweather_wh31e,
    },
    Device {
        framing: Framing {
            coding: Coding::Pcm {
                short_us: 52,
                long_us: 52,
            },
            gap_us: 0,
            reset_us: 150,
            tolerance_us: 0,
        },
        layout: Layout::After {
            sync: 0x0000_AAA9,
            sync_bits: 16,
            bits: 72,
            recode: Recode::Manchester,
        },
        invert: false,
        read: payload::tpms_renault,
    },
    Device {
        framing: Framing {
            coding: Coding::Pcm {
                short_us: 52,
                long_us: 52,
            },
            gap_us: 0,
            reset_us: 150,
            tolerance_us: 0,
        },
        layout: Layout::After {
            sync: 0x0000_054F,
            sync_bits: 11,
            bits: 72,
            recode: Recode::Differential,
        },
        invert: false,
        read: payload::tpms_toyota,
    },
    Device {
        framing: Framing {
            coding: Coding::Manchester { short_us: 500 },
            gap_us: 0,
            reset_us: 2_400,
            tolerance_us: 0,
        },
        layout: Layout::After {
            sync: 0x0000_0145,
            sync_bits: 12,
            bits: 48,
            recode: Recode::None,
        },
        invert: false,
        read: payload::ambientweather_f007th,
    },
    Device {
        framing: Framing {
            coding: Coding::Pwm {
                short_us: 500,
                long_us: 1_500,
                sync_us: 0,
            },
            gap_us: 0,
            reset_us: 1_200,
            tolerance_us: 160,
        },
        layout: row(48, 48),
        invert: false,
        read: payload::fineoffset_wh2,
    },
    Device {
        framing: Framing {
            coding: Coding::Pwm {
                short_us: 208,
                long_us: 417,
                sync_us: 833,
            },
            gap_us: 625,
            reset_us: 1_700,
            tolerance_us: 0,
        },
        layout: row(40, 41),
        invert: true,
        read: payload::lacrosse_tx141th_bv2,
    },
    Device {
        framing: Framing {
            coding: Coding::Dmc {
                short_us: 976,
                long_us: 1_952,
            },
            gap_us: 0,
            reset_us: 18_000,
            tolerance_us: 100,
        },
        layout: row(36, 36),
        invert: false,
        read: payload::wt450,
    },
    Device {
        framing: ppm(2_000, 4_000, 0, 5_000, 750),
        layout: row(40, 42),
        invert: false,
        read: payload::infactory,
    },
    Device {
        framing: ppm(1_000, 2_000, 3_000, 10_000, 0),
        layout: row(40, 40),
        invert: false,
        read: payload::acurite_609txc,
    },
    Device {
        framing: ppm(2_000, 4_000, 7_000, 10_000, 0),
        layout: row(32, 33),
        invert: false,
        read: payload::acurite_606tx,
    },
    Device {
        framing: pwm(252, 612, 860, 750, 62_990, 0),
        layout: row(40, 40),
        invert: true,
        read: payload::auriol_hg02832,
    },
    Device {
        framing: pwm(680, 1_850, 10_000, 4_000, 30_000, 0),
        layout: row(49, 49),
        invert: false,
        read: payload::wt0124,
    },
    Device {
        framing: pwm(544, 932, 0, 10_000, 31_000, 0),
        layout: row(48, 48),
        invert: false,
        read: payload::opus_xt300,
    },
    Device {
        framing: ppm(2_000, 4_000, 4_400, 9_400, 0),
        layout: Layout::Row {
            min_bits: 42,
            max_bits: 42,
            skip_bits: 2,
        },
        invert: false,
        read: payload::kedsum,
    },
    Device {
        framing: ppm(1_000, 2_000, 3_000, 4_800, 0),
        layout: row(36, 38),
        invert: false,
        read: payload::rubicson,
    },
    Device {
        framing: ppm(2_000, 4_000, 5_000, 9_200, 0),
        layout: row(36, 37),
        invert: false,
        read: payload::springfield,
    },
    Device {
        framing: ppm(1_000, 2_000, 3_000, 5_000, 0),
        layout: row(36, 37),
        invert: false,
        read: payload::nexus,
    },
    Device {
        framing: ppm(2_000, 4_000, 7_000, 10_000, 0),
        layout: row(36, 37),
        invert: false,
        read: payload::prologue,
    },
];

pub(super) fn identify(edges_us: &[u32]) -> Option<Match> {
    SENSORS
        .iter()
        .find_map(|device| read_device(device, edges_us))
}

struct Candidate {
    bits: Vec<bool>,
    repeats: u32,
}

fn read_device(device: &Device, edges_us: &[u32]) -> Option<Match> {
    let rows = slice(&device.framing, edges_us);
    let mut best: Option<Match> = None;
    for candidate in tally(rows, device) {
        let Some(reading) = (device.read)(&pack(&candidate.bits)) else {
            continue;
        };
        if best
            .as_ref()
            .is_some_and(|held| held.repeats >= candidate.repeats)
        {
            continue;
        }
        best = Some(Match {
            encoding: encoding_of(device.framing.coding),
            bits: candidate.bits,
            reading,
            short_us: short_of(device.framing.coding),
            repeats: candidate.repeats,
        });
    }
    best
}

fn encoding_of(coding: Coding) -> SubghzEncoding {
    match coding {
        Coding::Pcm { .. } => SubghzEncoding::Pcm,
        Coding::Ppm { .. } => SubghzEncoding::Ppm,
        Coding::Pwm { .. } => SubghzEncoding::Pwm,
        Coding::Manchester { .. } => SubghzEncoding::Manchester,
        Coding::Dmc { .. } => SubghzEncoding::Dmc,
    }
}

fn short_of(coding: Coding) -> u32 {
    match coding {
        Coding::Pcm { short_us, .. }
        | Coding::Ppm { short_us, .. }
        | Coding::Pwm { short_us, .. }
        | Coding::Manchester { short_us }
        | Coding::Dmc { short_us, .. } => short_us,
    }
}

fn matches_sync(row: &[bool], start: usize, sync: u32, sync_bits: usize) -> bool {
    (0..sync_bits).all(|offset| row[start + offset] == (sync >> (sync_bits - 1 - offset) & 1 == 1))
}

fn found_rows(row: &[bool], device: &Device) -> Vec<Vec<bool>> {
    match device.layout {
        Layout::Row {
            min_bits,
            max_bits,
            skip_bits,
        } => {
            if (min_bits..=max_bits).contains(&row.len()) {
                vec![row[skip_bits..min_bits].to_vec()]
            } else {
                Vec::new()
            }
        }
        Layout::After {
            sync,
            sync_bits,
            bits,
            recode: mode,
        } => {
            let mut out = Vec::new();
            let span = sync_bits + bits;
            let mut start = 0;
            while start + span <= row.len() {
                if !matches_sync(row, start, sync, sync_bits) {
                    start += 1;
                    continue;
                }
                let body = recode(mode, &row[start + sync_bits..]);
                if body.len() >= bits {
                    out.push(body[..bits].to_vec());
                }
                start += span;
            }
            out
        }
    }
}

fn tally(rows: Vec<Vec<bool>>, device: &Device) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for row in rows {
        for found in found_rows(&row, device) {
            let bits: Vec<bool> = found.iter().map(|&bit| bit != device.invert).collect();
            match out.iter_mut().find(|held| held.bits == bits) {
                Some(held) => held.repeats += 1,
                None => out.push(Candidate { bits, repeats: 1 }),
            }
        }
    }
    out
}

fn pack(bits: &[bool]) -> Payload {
    let mut out = [0u8; PAYLOAD_BYTES];
    for (index, &bit) in bits.iter().take(PAYLOAD_BYTES * 8).enumerate() {
        if bit {
            out[index / 8] |= 0x80 >> (index % 8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits_of(bytes: &[u8], count: usize) -> Vec<bool> {
        (0..count)
            .map(|index| bytes[index / 8] >> (7 - index % 8) & 1 == 1)
            .collect()
    }

    fn ppm_edges(
        bytes: &[u8],
        count: usize,
        short: u32,
        long: u32,
        sync: u32,
        repeats: u32,
    ) -> Vec<u32> {
        let bits = bits_of(bytes, count);
        let mut edges = Vec::new();
        for _ in 0..repeats {
            for &bit in &bits {
                edges.push(500);
                edges.push(if bit { long } else { short });
            }
            edges.push(500);
            edges.push(sync);
        }
        edges.pop();
        edges
    }

    fn nrz_edges(bits: &[bool]) -> Vec<u32> {
        let mut edges = Vec::new();
        let mut run = 0u32;
        let mut level = true;
        for &bit in bits {
            if bit == level {
                run += 56;
            } else {
                edges.push(run);
                level = bit;
                run = 56;
            }
        }
        edges.push(run);
        if !bits.first().copied().unwrap_or(true) {
            edges.remove(0);
        }
        edges
    }

    #[test]
    fn a_ppm_burst_splits_into_one_row_per_repeat_on_its_sync_gap() {
        let edges = ppm_edges(&[0x8F, 0x80, 0xD5, 0xF2, 0xF0], 36, 1_000, 2_000, 4_000, 12);
        let found = identify(&edges).expect("a Nexus burst");
        assert_eq!(found.encoding, SubghzEncoding::Ppm);
        assert_eq!(found.repeats, 12);
        assert_eq!(found.reading.temperature_c, Some(21.3));
    }

    #[test]
    fn a_pwm_burst_survives_the_preamble_that_separates_its_packets() {
        let bits = bits_of(&[0x8F, 0x12, 0xC9, 0x2F, 0x9F], 40);
        let mut edges = Vec::new();
        for _ in 0..12 {
            for _ in 0..4 {
                edges.push(833);
                edges.push(833);
            }
            for &bit in &bits {
                let (pulse, gap) = if bit { (417, 208) } else { (208, 417) };
                edges.push(pulse);
                edges.push(gap);
            }
        }
        edges.pop();
        let found = identify(&edges).expect("a LaCrosse burst");
        assert_eq!(found.encoding, SubghzEncoding::Pwm);
        assert_eq!(found.repeats, 12);
        assert_eq!(found.reading.model, "LaCrosse-TX141THBv2");
        assert_eq!(found.reading.humidity_pct, Some(47.0));
    }

    #[test]
    fn two_sensors_that_share_a_pulse_coding_are_told_apart_by_their_payloads() {
        let acurite = ppm_edges(&[0x7B, 0x80, 0xBB, 0x76], 32, 2_000, 4_000, 9_000, 6);
        let found = identify(&acurite).expect("an Acurite 606 burst");
        assert_eq!(found.reading.model, "Acurite-606TX");
        assert_eq!(found.reading.temperature_c, Some(18.7));

        let prologue = ppm_edges(&[0x9C, 0x79, 0x0E, 0xA3, 0x70], 36, 2_000, 4_000, 9_000, 5);
        let found = identify(&prologue).expect("a Prologue burst");
        assert_eq!(found.reading.model, "Prologue-TH");
        assert_eq!(found.reading.humidity_pct, Some(55.0));
    }

    #[test]
    fn an_infactory_burst_is_read_through_its_own_wider_tolerance() {
        let edges = ppm_edges(&[0x0F, 0x80, 0x65, 0x06, 0x23], 40, 2_000, 4_000, 16_000, 6);
        let found = identify(&edges).expect("an inFactory burst");
        assert_eq!(found.reading.model, "inFactory-TH");
        assert_eq!(found.reading.temperature_c, Some(22.0));
    }

    #[test]
    fn a_fine_offset_burst_is_read_off_its_pulse_widths() {
        let bits = bits_of(&[0xFF, 0x45, 0xA0, 0xC3, 0x49, 0xEB], 48);
        let mut edges = Vec::new();
        for &bit in &bits {
            edges.push(if bit { 544 } else { 1_524 });
            edges.push(1_036);
        }
        edges.pop();
        let found = identify(&edges).expect("a Fine Offset burst");
        assert_eq!(found.reading.model, "FineOffset-WH2");
        assert_eq!(found.reading.temperature_c, Some(19.5));
    }

    #[test]
    fn an_fsk_sensor_is_found_after_its_sync_word() {
        let mut bits = [true, false].repeat(24);
        bits.extend(bits_of(
            &[0x2D, 0xD4, 0x30, 0xC3, 0x82, 0x73, 0x33, 0xD0, 0xEB],
            72,
        ));
        let edges = nrz_edges(&bits);
        let found = identify(&edges).expect("a WH31E burst");
        assert_eq!(found.encoding, SubghzEncoding::Pcm);
        assert_eq!(found.reading.model, "AmbientWeather-WH31E");
        assert_eq!(found.reading.temperature_c, Some(22.7));
        assert_eq!(found.reading.humidity_pct, Some(51.0));
    }

    #[test]
    fn a_dmc_sensor_is_read_from_its_half_bit_and_full_bit_symbols() {
        let bits = bits_of(&[0xC3, 0x42, 0xD4, 0x78, 0x50], 36);
        let mut edges = Vec::new();
        for &bit in &bits {
            if bit {
                edges.push(976);
                edges.push(976);
            } else {
                edges.push(1_952);
            }
        }
        let found = identify(&edges).expect("a WT450 burst");
        assert_eq!(found.encoding, SubghzEncoding::Dmc);
        assert_eq!(found.reading.model, "WT450-TH");
        assert_eq!(found.reading.temperature_c, Some(21.5));
    }

    #[test]
    fn a_burst_of_the_wrong_length_matches_no_sensor_in_the_table() {
        let edges: Vec<u32> = std::iter::repeat_n([500, 1_000], 20).flatten().collect();
        assert!(identify(&edges).is_none());
    }
}
