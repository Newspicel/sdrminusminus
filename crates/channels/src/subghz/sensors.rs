use sdrmm_dsp::{crc8_msb, lfsr_digest8_reflect};
use sdrmm_wire::{SubghzEncoding, SubghzReading};

const TOLERANCE: f64 = 0.3;
const PAYLOAD_BYTES: usize = 5;

pub(super) struct Match {
    pub encoding: SubghzEncoding,
    pub bits: Vec<bool>,
    pub reading: SubghzReading,
    pub short_us: u32,
    pub repeats: u32,
}

enum Coding {
    Ppm { short_us: u32, long_us: u32 },
    Pwm { short_us: u32, long_us: u32 },
}

struct Sensor {
    coding: Coding,
    bits: usize,
    read: fn(&[u8; PAYLOAD_BYTES]) -> Option<SubghzReading>,
}

const SENSORS: &[Sensor] = &[
    Sensor {
        coding: Coding::Ppm {
            short_us: 1_000,
            long_us: 2_000,
        },
        bits: 36,
        read: nexus,
    },
    Sensor {
        coding: Coding::Ppm {
            short_us: 1_000,
            long_us: 2_000,
        },
        bits: 40,
        read: acurite_609txc,
    },
    Sensor {
        coding: Coding::Pwm {
            short_us: 208,
            long_us: 417,
        },
        bits: 40,
        read: lacrosse_tx141th_bv2,
    },
];

pub(super) fn identify(edges_us: &[u32]) -> Option<Match> {
    SENSORS
        .iter()
        .find_map(|sensor| read_sensor(sensor, edges_us))
}

fn read_sensor(sensor: &Sensor, edges_us: &[u32]) -> Option<Match> {
    let rows = split_rows(&sensor.coding, edges_us, sensor.bits);
    let mut best: Option<Match> = None;
    for row in dedupe(rows) {
        let Some(reading) = (sensor.read)(&pack(&row.bits)) else {
            continue;
        };
        if best
            .as_ref()
            .is_some_and(|held| held.repeats >= row.repeats)
        {
            continue;
        }
        best = Some(Match {
            encoding: match sensor.coding {
                Coding::Ppm { .. } => SubghzEncoding::Ppm,
                Coding::Pwm { .. } => SubghzEncoding::Pwm,
            },
            bits: row.bits,
            reading,
            short_us: match sensor.coding {
                Coding::Ppm { short_us, .. } | Coding::Pwm { short_us, .. } => short_us,
            },
            repeats: row.repeats,
        });
    }
    best
}

struct Row {
    bits: Vec<bool>,
    repeats: u32,
}

fn symbol(measured: u32, short_us: u32, long_us: u32) -> Option<bool> {
    let measured = f64::from(measured);
    let short_us = f64::from(short_us);
    let long_us = f64::from(long_us);
    if measured < short_us * (1.0 - TOLERANCE) || measured > long_us * (1.0 + TOLERANCE) {
        return None;
    }
    Some((measured - short_us).abs() > (measured - long_us).abs())
}

fn split_rows(coding: &Coding, edges_us: &[u32], want: usize) -> Vec<Vec<bool>> {
    let (short_us, long_us, gap_carries_bit) = match *coding {
        Coding::Ppm { short_us, long_us } => (short_us, long_us, true),
        Coding::Pwm { short_us, long_us } => (short_us, long_us, false),
    };
    let mut rows = Vec::new();
    let mut row: Vec<bool> = Vec::new();
    let mut index = 0;
    while index < edges_us.len() {
        let measured = if gap_carries_bit {
            match edges_us.get(index + 1) {
                Some(&gap) => gap,
                None => break,
            }
        } else {
            edges_us[index]
        };
        match symbol(measured, short_us, long_us) {
            Some(bit) => row.push(bit),
            None => close_row(&mut rows, &mut row, want),
        }
        index += 2;
    }
    close_row(&mut rows, &mut row, want);
    rows
}

fn close_row(rows: &mut Vec<Vec<bool>>, row: &mut Vec<bool>, want: usize) {
    let taken = std::mem::take(row);
    if (want..=want + 1).contains(&taken.len()) {
        rows.push(taken[..want].to_vec());
    }
}

fn dedupe(rows: Vec<Vec<bool>>) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    for bits in rows {
        match out.iter_mut().find(|held| held.bits == bits) {
            Some(held) => held.repeats += 1,
            None => out.push(Row { bits, repeats: 1 }),
        }
    }
    out
}

fn pack(bits: &[bool]) -> [u8; PAYLOAD_BYTES] {
    let mut out = [0u8; PAYLOAD_BYTES];
    for (index, &bit) in bits.iter().take(PAYLOAD_BYTES * 8).enumerate() {
        if bit {
            out[index / 8] |= 0x80 >> (index % 8);
        }
    }
    out
}

fn signed_12(raw: u16) -> i16 {
    ((raw << 4) as i16) >> 4
}

fn nexus(b: &[u8; PAYLOAD_BYTES]) -> Option<SubghzReading> {
    if b[3] & 0xF0 != 0xF0 || b[1] & 0x30 == 0x30 {
        return None;
    }
    if (b[0] == 0x00 && b[2] == 0x00 && b[3] == 0x00)
        || (b[0] == 0xFF && b[2] == 0xFF && b[3] == 0xFF)
    {
        return None;
    }
    let rubicson = [
        b[0],
        b[1],
        b[2],
        b[3] & 0xF0,
        (b[3] & 0x0F) << 4 | (b[4] & 0xF0) >> 4,
    ];
    if crc8_msb(0x31, 0x6C, &rubicson) == 0 {
        return None;
    }
    let humidity = (b[3] & 0x0F) << 4 | b[4] >> 4;
    if humidity > 100 {
        return None;
    }
    let temperature_c = f64::from(signed_12(u16::from(b[1] & 0x0F) << 8 | u16::from(b[2]))) * 0.1;
    Some(SubghzReading {
        model: if humidity == 0 { "Nexus-T" } else { "Nexus-TH" }.to_owned(),
        id: u32::from(b[0]),
        channel: Some((b[1] & 0x30) >> 4).map(|code| code + 1),
        battery_ok: Some(b[1] & 0x80 != 0),
        temperature_c: Some(temperature_c),
        humidity_pct: (humidity != 0).then(|| f64::from(humidity)),
    })
}

fn acurite_609txc(b: &[u8; PAYLOAD_BYTES]) -> Option<SubghzReading> {
    let sum = u32::from(b[0]) + u32::from(b[1]) + u32::from(b[2]) + u32::from(b[3]);
    if sum == 0 || (sum & 0xFF) as u8 != b[4] || b[3] > 100 {
        return None;
    }
    let temperature_c = f64::from(signed_12(u16::from(b[1] & 0x0F) << 8 | u16::from(b[2]))) * 0.1;
    Some(SubghzReading {
        model: "Acurite-609TXC".to_owned(),
        id: u32::from(b[0]),
        channel: None,
        battery_ok: Some(b[1] & 0x80 == 0),
        temperature_c: Some(temperature_c),
        humidity_pct: Some(f64::from(b[3])),
    })
}

fn lacrosse_tx141th_bv2(b: &[u8; PAYLOAD_BYTES]) -> Option<SubghzReading> {
    if lfsr_digest8_reflect(0x31, 0xF4, &b[..4]) != b[4] || b[0] == 0 {
        return None;
    }
    if b[3] == 0 || b[3] > 100 {
        return None;
    }
    let temperature_c = (f64::from(u16::from(b[1] & 0x0F) << 8 | u16::from(b[2])) - 500.0) * 0.1;
    if !(-40.0..=140.0).contains(&temperature_c) {
        return None;
    }
    Some(SubghzReading {
        model: "LaCrosse-TX141THBv2".to_owned(),
        id: u32::from(b[0]),
        channel: Some((b[1] & 0x30) >> 4),
        battery_ok: Some(b[1] & 0x80 == 0),
        temperature_c: Some(temperature_c),
        humidity_pct: Some(f64::from(b[3])),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits_of(bytes: &[u8], count: usize) -> Vec<bool> {
        (0..count)
            .map(|index| bytes[index / 8] >> (7 - index % 8) & 1 == 1)
            .collect()
    }

    #[test]
    fn a_nexus_payload_reads_back_the_values_it_was_built_from() {
        let reading = nexus(&[0x8F, 0x80, 0xD5, 0xF2, 0xF0]).expect("valid Nexus payload");
        assert_eq!(reading.model, "Nexus-TH");
        assert_eq!(reading.id, 0x8F);
        assert_eq!(reading.channel, Some(1));
        assert_eq!(reading.battery_ok, Some(true));
        assert_eq!(reading.temperature_c, Some(21.3));
        assert_eq!(reading.humidity_pct, Some(47.0));
    }

    #[test]
    fn a_nexus_payload_carries_temperature_below_freezing_as_a_signed_value() {
        let reading = nexus(&[0x5C, 0x2F, 0xB5, 0xF5, 0x80]).expect("valid Nexus payload");
        assert_eq!(reading.temperature_c, Some(-7.5));
        assert_eq!(reading.humidity_pct, Some(88.0));
        assert_eq!(reading.channel, Some(3));
        assert_eq!(reading.battery_ok, Some(false));
    }

    #[test]
    fn a_nexus_sensor_without_a_hygrometer_reports_only_temperature() {
        let reading = nexus(&[0xA3, 0x90, 0x1E, 0xF0, 0x00]).expect("valid Nexus payload");
        assert_eq!(reading.model, "Nexus-T");
        assert_eq!(reading.temperature_c, Some(3.0));
        assert_eq!(reading.humidity_pct, None);
    }

    #[test]
    fn nexus_rejects_a_payload_whose_constant_nibble_is_wrong() {
        assert!(nexus(&[0x8F, 0x80, 0xD5, 0xE2, 0xF0]).is_none());
    }

    #[test]
    fn nexus_rejects_the_fourth_channel_its_dial_cannot_select() {
        assert!(nexus(&[0x8F, 0xB0, 0xD5, 0xF2, 0xF0]).is_none());
    }

    #[test]
    fn nexus_rejects_a_humidity_no_hygrometer_could_report() {
        assert!(nexus(&[0x8F, 0x80, 0xD5, 0xFF, 0xF0]).is_none());
    }

    #[test]
    fn an_acurite_payload_reads_back_the_values_it_was_built_from() {
        let reading = acurite_609txc(&[0xD4, 0xA0, 0xF6, 0x3D, 0xA7]).expect("valid Acurite");
        assert_eq!(reading.model, "Acurite-609TXC");
        assert_eq!(reading.id, 0xD4);
        assert_eq!(reading.battery_ok, Some(false));
        assert_eq!(reading.temperature_c, Some(24.6));
        assert_eq!(reading.humidity_pct, Some(61.0));
    }

    #[test]
    fn acurite_rejects_a_payload_whose_checksum_does_not_add_up() {
        assert!(acurite_609txc(&[0x2A, 0x2F, 0xB5, 0x58, 0x67]).is_none());
        assert_eq!(
            acurite_609txc(&[0x2A, 0x2F, 0xB5, 0x58, 0x66])
                .expect("valid Acurite")
                .temperature_c,
            Some(-7.5)
        );
    }

    #[test]
    fn a_lacrosse_payload_reads_back_the_values_it_was_built_from() {
        let reading =
            lacrosse_tx141th_bv2(&[0x8F, 0x12, 0xC9, 0x2F, 0x9F]).expect("valid LaCrosse");
        assert_eq!(reading.model, "LaCrosse-TX141THBv2");
        assert_eq!(reading.id, 0x8F);
        assert_eq!(reading.channel, Some(1));
        assert_eq!(reading.battery_ok, Some(true));
        assert_eq!(reading.temperature_c, Some(21.3));
        assert_eq!(reading.humidity_pct, Some(47.0));
    }

    #[test]
    fn a_lacrosse_payload_offsets_temperature_below_freezing() {
        let reading =
            lacrosse_tx141th_bv2(&[0x2A, 0xA1, 0xA9, 0x58, 0x23]).expect("valid LaCrosse");
        assert_eq!(reading.temperature_c, Some(-7.5));
        assert_eq!(reading.channel, Some(2));
        assert_eq!(reading.battery_ok, Some(false));
    }

    #[test]
    fn lacrosse_rejects_every_single_bit_flip_in_its_payload() {
        let clean = [0x8F, 0x12, 0xC9, 0x2F, 0x9F];
        for bit in 0..40 {
            let mut damaged = clean;
            damaged[bit / 8] ^= 0x80 >> (bit % 8);
            assert!(
                lacrosse_tx141th_bv2(&damaged).is_none(),
                "bit {bit} slipped through"
            );
        }
    }

    #[test]
    fn a_ppm_burst_splits_into_one_row_per_repeat_on_its_sync_gap() {
        let payload = [0x8F, 0x80, 0xD5, 0xF2, 0xF0];
        let bits = bits_of(&payload, 36);
        let mut edges = Vec::new();
        for _ in 0..12 {
            for &bit in &bits {
                edges.push(500);
                edges.push(if bit { 2_000 } else { 1_000 });
            }
            edges.push(500);
            edges.push(4_000);
        }
        edges.pop();
        let found = identify(&edges).expect("a Nexus burst");
        assert_eq!(found.encoding, SubghzEncoding::Ppm);
        assert_eq!(found.bits, bits);
        assert_eq!(found.repeats, 12);
        assert_eq!(found.reading.temperature_c, Some(21.3));
    }

    #[test]
    fn a_pwm_burst_survives_the_preamble_that_separates_its_packets() {
        let payload = [0x8F, 0x12, 0xC9, 0x2F, 0x9F];
        let bits = bits_of(&payload, 40);
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
    fn a_burst_of_the_wrong_length_matches_no_sensor_in_the_table() {
        let edges: Vec<u32> = std::iter::repeat_n([500, 1_000], 20).flatten().collect();
        assert!(identify(&edges).is_none());
    }
}
