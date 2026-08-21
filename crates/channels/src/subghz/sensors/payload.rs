use sdrmm_dsp::{crc4_msb, crc8_msb, lfsr_digest8, lfsr_digest8_reflect};
use sdrmm_wire::SubghzReading;

pub(super) const PAYLOAD_BYTES: usize = 16;

pub(super) type Payload = [u8; PAYLOAD_BYTES];

fn signed_12(raw: u16) -> i16 {
    ((raw << 4) as i16) >> 4
}

fn plausible(temperature_c: f64, humidity_pct: Option<f64>) -> bool {
    (-50.0..=80.0).contains(&temperature_c)
        && humidity_pct.is_none_or(|humidity| (0.0..=100.0).contains(&humidity))
}

pub(super) fn nexus(b: &Payload) -> Option<SubghzReading> {
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
    let temperature_c = f64::from(signed_12(u16::from(b[1] & 0x0F) << 8 | u16::from(b[2]))) / 10.0;
    Some(SubghzReading {
        model: if humidity == 0 { "Nexus-T" } else { "Nexus-TH" }.to_owned(),
        id: u32::from(b[0]),
        channel: Some(((b[1] & 0x30) >> 4) + 1),
        battery_ok: Some(b[1] & 0x80 != 0),
        temperature_c: Some(temperature_c),
        humidity_pct: (humidity != 0).then(|| f64::from(humidity)),
        ..SubghzReading::default()
    })
}

pub(super) fn acurite_609txc(b: &Payload) -> Option<SubghzReading> {
    let sum = u32::from(b[0]) + u32::from(b[1]) + u32::from(b[2]) + u32::from(b[3]);
    if sum == 0 || (sum & 0xFF) as u8 != b[4] || b[3] > 100 {
        return None;
    }
    let temperature_c = f64::from(signed_12(u16::from(b[1] & 0x0F) << 8 | u16::from(b[2]))) / 10.0;
    Some(SubghzReading {
        model: "Acurite-609TXC".to_owned(),
        id: u32::from(b[0]),
        channel: None,
        battery_ok: Some(b[1] & 0x80 == 0),
        temperature_c: Some(temperature_c),
        humidity_pct: Some(f64::from(b[3])),
        ..SubghzReading::default()
    })
}

pub(super) fn acurite_606tx(b: &Payload) -> Option<SubghzReading> {
    if b[0] == 0 && b[1] == 0 && b[2] == 0 && b[3] == 0 {
        return None;
    }
    if lfsr_digest8(0x98, 0xF1, &b[..3]) != b[3] {
        return None;
    }
    let temperature_c = f64::from(signed_12(u16::from(b[1] & 0x0F) << 8 | u16::from(b[2]))) / 10.0;
    if !plausible(temperature_c, None) {
        return None;
    }
    Some(SubghzReading {
        model: "Acurite-606TX".to_owned(),
        id: u32::from(b[0]),
        channel: Some(((b[1] & 0x30) >> 4) + 1),
        battery_ok: Some(b[1] & 0x80 != 0),
        temperature_c: Some(temperature_c),
        humidity_pct: None,
        ..SubghzReading::default()
    })
}

pub(super) fn lacrosse_tx141th_bv2(b: &Payload) -> Option<SubghzReading> {
    if lfsr_digest8_reflect(0x31, 0xF4, &b[..4]) != b[4] || b[0] == 0 {
        return None;
    }
    if b[3] == 0 || b[3] > 100 {
        return None;
    }
    let temperature_c = (f64::from(u16::from(b[1] & 0x0F) << 8 | u16::from(b[2])) - 500.0) / 10.0;
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
        ..SubghzReading::default()
    })
}

pub(super) fn prologue(b: &Payload) -> Option<SubghzReading> {
    let subtype = b[0] >> 4;
    if subtype != 0x9 && subtype != 0x5 {
        return None;
    }
    let temperature_c = f64::from(signed_12(u16::from(b[2]) << 4 | u16::from(b[3] >> 4))) / 10.0;
    let raw_humidity = (b[3] & 0x0F) << 4 | b[4] >> 4;
    let humidity_pct = (raw_humidity != 0xCC).then(|| f64::from(raw_humidity));
    if !plausible(temperature_c, humidity_pct) {
        return None;
    }
    Some(SubghzReading {
        model: "Prologue-TH".to_owned(),
        id: u32::from((b[0] & 0x0F) << 4 | (b[1] & 0xF0) >> 4),
        channel: Some((b[1] & 0x03) + 1),
        battery_ok: Some(b[1] & 0x08 != 0),
        temperature_c: Some(temperature_c),
        humidity_pct,
        ..SubghzReading::default()
    })
}

pub(super) fn infactory(b: &Payload) -> Option<SubghzReading> {
    let channel = b[4] & 0x03;
    if channel == 0 {
        return None;
    }
    let message = [b[0], (b[1] & 0x0F) | (b[4] & 0x0F) << 4, b[2], b[3]];
    if crc4_msb(0x13, 0, &message) ^ (b[4] >> 4) != b[1] >> 4 {
        return None;
    }
    let humidity = f64::from((b[3] & 0x0F) * 10 + (b[4] >> 4));
    let raw = u32::from(b[2]) << 4 | u32::from(b[3] >> 4);
    let temperature_c = (f64::from(raw) - 1_220.0) / 18.0;
    if !plausible(temperature_c, Some(humidity)) {
        return None;
    }
    Some(SubghzReading {
        model: "inFactory-TH".to_owned(),
        id: u32::from(b[0]),
        channel: Some(channel),
        battery_ok: Some(b[1] & 0x04 == 0),
        temperature_c: Some(temperature_c),
        humidity_pct: Some(humidity),
        ..SubghzReading::default()
    })
}

pub(super) fn fineoffset_wh2(b: &Payload) -> Option<SubghzReading> {
    if b[0] != 0xFF || b[1] >> 4 != 0x4 {
        return None;
    }
    let body = [b[1], b[2], b[3], b[4]];
    if crc8_msb(0x31, 0x00, &body) != b[5] {
        return None;
    }
    let magnitude = u16::from(b[2] & 0x0F) << 8 | u16::from(b[3]);
    let temperature_c = if magnitude & 0x800 == 0 {
        f64::from(magnitude) / 10.0
    } else {
        f64::from(magnitude & 0x7FF) / -10.0
    };
    let humidity = f64::from(b[4]);
    if !plausible(temperature_c, Some(humidity)) {
        return None;
    }
    Some(SubghzReading {
        model: "FineOffset-WH2".to_owned(),
        id: u32::from((b[1] & 0x0F) << 4 | (b[2] & 0xF0) >> 4),
        channel: None,
        battery_ok: None,
        temperature_c: Some(temperature_c),
        humidity_pct: Some(humidity),
        ..SubghzReading::default()
    })
}

pub(super) fn ambientweather_f007th(b: &Payload) -> Option<SubghzReading> {
    if b[0] & 0xF0 != 0x40 {
        return None;
    }
    if lfsr_digest8(0x98, 0x3E, &b[..5]) ^ 0x64 != b[5] {
        return None;
    }
    let raw = u32::from(b[2] & 0x0F) << 8 | u32::from(b[3]);
    let temperature_c = (f64::from(raw) - 720.0) / 18.0;
    let humidity = f64::from(b[4]);
    if !plausible(temperature_c, Some(humidity)) {
        return None;
    }
    Some(SubghzReading {
        model: "AmbientWeather-F007TH".to_owned(),
        id: u32::from(b[1]),
        channel: Some(((b[2] & 0x70) >> 4) + 1),
        battery_ok: Some(b[2] & 0x80 == 0),
        temperature_c: Some(temperature_c),
        humidity_pct: Some(humidity),
        ..SubghzReading::default()
    })
}

pub(super) fn ambientweather_wh31e(b: &Payload) -> Option<SubghzReading> {
    if b[0] != 0x30 {
        return None;
    }
    if crc8_msb(0x31, 0x00, &b[..6]) != 0 {
        return None;
    }
    let sum = b[..6].iter().fold(0u32, |acc, &byte| acc + u32::from(byte));
    if (sum & 0xFF) as u8 != b[6] {
        return None;
    }
    let raw = u32::from(b[2] & 0x03) << 8 | u32::from(b[3]);
    let temperature_c = (f64::from(raw) - 400.0) / 10.0;
    let humidity = f64::from(b[4]);
    if !plausible(temperature_c, Some(humidity)) {
        return None;
    }
    Some(SubghzReading {
        model: "AmbientWeather-WH31E".to_owned(),
        id: u32::from(b[1]),
        channel: Some(((b[2] & 0x70) >> 4) + 1),
        battery_ok: Some(b[2] & 0x04 == 0),
        temperature_c: Some(temperature_c),
        humidity_pct: Some(humidity),
        ..SubghzReading::default()
    })
}

pub(super) fn wt450(b: &Payload) -> Option<SubghzReading> {
    if b[0] >> 4 != 0xC {
        return None;
    }
    let mut parity = b[..5].iter().fold(0u8, |acc, &byte| acc ^ byte);
    parity ^= parity >> 4;
    parity ^= parity >> 2;
    if parity & 0x03 != 0 {
        return None;
    }
    let whole = i32::from((b[2] & 0x0F) << 4 | b[3] >> 4);
    let temperature_c = f64::from(whole - 50) + f64::from(b[3] & 0x0F) / 16.0;
    let humidity = f64::from((b[1] & 0x07) << 4 | b[2] >> 4);
    if !plausible(temperature_c, Some(humidity)) {
        return None;
    }
    Some(SubghzReading {
        model: "WT450-TH".to_owned(),
        id: u32::from(b[0] & 0x0F),
        channel: Some((b[1] >> 6) + 1),
        battery_ok: Some(b[1] & 0x08 == 0),
        temperature_c: Some(temperature_c),
        humidity_pct: Some(humidity),
        ..SubghzReading::default()
    })
}

pub(super) fn rubicson(b: &Payload) -> Option<SubghzReading> {
    if b[3] & 0xF0 != 0xF0 {
        return None;
    }
    let checked = [
        b[0],
        b[1],
        b[2],
        b[3] & 0xF0,
        (b[3] & 0x0F) << 4 | (b[4] & 0xF0) >> 4,
    ];
    if crc8_msb(0x31, 0x6C, &checked) != 0 {
        return None;
    }
    let temperature_c = f64::from(signed_12(u16::from(b[1] & 0x0F) << 8 | u16::from(b[2]))) / 10.0;
    if !plausible(temperature_c, None) {
        return None;
    }
    Some(SubghzReading {
        model: "Rubicson-Temperature".to_owned(),
        id: u32::from(b[0]),
        channel: Some(((b[1] & 0x30) >> 4) + 1),
        battery_ok: Some(b[1] & 0x80 != 0),
        temperature_c: Some(temperature_c),
        ..SubghzReading::default()
    })
}

pub(super) fn kedsum(b: &Payload) -> Option<SubghzReading> {
    if crc4_msb(0x3, 0, &b[..4]) ^ (b[4] >> 4) != b[4] & 0x0F {
        return None;
    }
    let raw = u32::from(b[2] & 0x0F) << 8 | u32::from(b[2] & 0xF0) | u32::from(b[1] & 0x0F);
    let temperature_c = (f64::from(raw) - 1_220.0) / 18.0;
    let humidity = f64::from((b[3] & 0x0F) << 4 | (b[3] & 0xF0) >> 4);
    if !plausible(temperature_c, Some(humidity)) {
        return None;
    }
    Some(SubghzReading {
        model: "Kedsum-TH".to_owned(),
        id: u32::from(b[0]),
        channel: Some(((b[1] & 0x30) >> 4) + 1),
        battery_ok: Some(b[1] >> 6 == 2),
        temperature_c: Some(temperature_c),
        humidity_pct: Some(humidity),
        ..SubghzReading::default()
    })
}

pub(super) fn springfield(b: &Payload) -> Option<SubghzReading> {
    let head =
        u32::from(b[0]) << 24 | u32::from(b[1]) << 16 | u32::from(b[2]) << 8 | u32::from(b[3]);
    if head == 0 || head == u32::MAX {
        return None;
    }
    let folded = b[..4].iter().fold(0u8, |acc, &byte| acc ^ byte);
    if (folded >> 4) ^ (folded & 0x0F) != 0 {
        return None;
    }
    let temperature_c = f64::from(signed_12(u16::from(b[1] & 0x0F) << 8 | u16::from(b[2]))) / 10.0;
    let moisture = f64::from((b[3] >> 4) * 10);
    if !(-30.0..=70.0).contains(&temperature_c) || moisture > 100.0 {
        return None;
    }
    Some(SubghzReading {
        model: "Springfield-Soil".to_owned(),
        id: u32::from(b[0]),
        channel: Some(((b[1] >> 4) & 0x03) + 1),
        battery_ok: Some(b[1] & 0x80 == 0),
        temperature_c: Some(temperature_c),
        moisture_pct: Some(moisture),
        ..SubghzReading::default()
    })
}

pub(super) fn auriol_hg02832(b: &Payload) -> Option<SubghzReading> {
    let folded = b[0] ^ b[1] ^ b[2] ^ b[3];
    if crc8_msb(0x31, 0x53, &[folded]) ^ b[4] != 0 {
        return None;
    }
    let temperature_c = f64::from(signed_12(u16::from(b[2] & 0x0F) << 8 | u16::from(b[3]))) / 10.0;
    let humidity = f64::from(b[1]);
    if !plausible(temperature_c, Some(humidity)) {
        return None;
    }
    Some(SubghzReading {
        model: "Auriol-HG02832".to_owned(),
        id: u32::from(b[0]),
        channel: Some(((b[2] & 0x30) >> 4) + 1),
        battery_ok: Some(b[2] & 0x80 == 0),
        temperature_c: Some(temperature_c),
        humidity_pct: Some(humidity),
        ..SubghzReading::default()
    })
}

pub(super) fn wt0124(b: &Payload) -> Option<SubghzReading> {
    if b[0] >> 4 != 0x5 {
        return None;
    }
    if b[..4].iter().fold(0u8, |acc, &byte| acc ^ byte) != b[4] {
        return None;
    }
    let mut sum = b[..4].iter().fold(0u32, |acc, &byte| acc + u32::from(byte));
    sum += sum >> 8;
    sum += u32::from(b[4]);
    if (sum & 0xFF) as u8 != b[5] {
        return None;
    }
    let raw = u32::from(b[1] & 0x0F) << 8 | u32::from(b[2]);
    let temperature_c = (f64::from(raw) - 2_448.0) / 10.0;
    if !plausible(temperature_c, None) {
        return None;
    }
    Some(SubghzReading {
        model: "WT0124-Pool".to_owned(),
        id: u32::from((b[0] & 0x0F) << 4 | (b[1] & 0x0F)),
        channel: Some((b[3] >> 4) & 0x03),
        temperature_c: Some(temperature_c),
        ..SubghzReading::default()
    })
}

pub(super) fn opus_xt300(b: &Payload) -> Option<SubghzReading> {
    if b[0] != 0xFF {
        return None;
    }
    let sum = b[1..5]
        .iter()
        .fold(0u32, |acc, &byte| acc + u32::from(byte))
        & 0xFF;
    if sum == 0 || sum as u8 != b[5] {
        return None;
    }
    let temperature_c = f64::from(i16::from(b[3]) - 40);
    let moisture = f64::from(b[2]);
    if temperature_c > 100.0 || moisture > 101.0 {
        return None;
    }
    Some(SubghzReading {
        model: "Opus-XT300".to_owned(),
        id: 0,
        channel: Some(b[1] & 0x03),
        temperature_c: Some(temperature_c),
        moisture_pct: Some(moisture),
        ..SubghzReading::default()
    })
}

pub(super) fn tpms_renault(b: &Payload) -> Option<SubghzReading> {
    if crc8_msb(0x07, 0x00, &b[..8]) != b[8] {
        return None;
    }
    let raw = u32::from(b[0] & 0x03) << 8 | u32::from(b[1]);
    let pressure_kpa = f64::from(raw) * 0.75;
    let temperature_c = f64::from(i16::from(b[2]) - 30);
    if !plausible(temperature_c, None) || pressure_kpa > 900.0 {
        return None;
    }
    Some(SubghzReading {
        model: "Renault-TPMS".to_owned(),
        id: u32::from(b[5]) << 16 | u32::from(b[4]) << 8 | u32::from(b[3]),
        temperature_c: Some(temperature_c),
        pressure_kpa: Some(pressure_kpa),
        ..SubghzReading::default()
    })
}

pub(super) fn tpms_toyota(b: &Payload) -> Option<SubghzReading> {
    if crc8_msb(0x07, 0x80, &b[..8]) != b[8] {
        return None;
    }
    let pressure = u16::from(b[4] & 0x7F) << 1 | u16::from(b[5] >> 7);
    if pressure != u16::from(b[7] ^ 0xFF) {
        return None;
    }
    let raw_temperature = u16::from(b[5] & 0x7F) << 1 | u16::from(b[6] >> 7);
    let temperature_c = f64::from(raw_temperature) - 40.0;
    let pressure_kpa = (f64::from(pressure) * 0.25 - 7.0) * 6.894_757_293_168_361;
    if !plausible(temperature_c, None) || !(0.0..=900.0).contains(&pressure_kpa) {
        return None;
    }
    Some(SubghzReading {
        model: "Toyota-TPMS".to_owned(),
        id: u32::from(b[0]) << 24 | u32::from(b[1]) << 16 | u32::from(b[2]) << 8 | u32::from(b[3]),
        temperature_c: Some(temperature_c),
        pressure_kpa: Some(pressure_kpa),
        ..SubghzReading::default()
    })
}

const WS2032_PREAMBLE: u8 = 0xF5;

pub(super) fn ws2032(b: &Payload) -> Option<SubghzReading> {
    let mut frame = [0u8; 14];
    frame[0] = WS2032_PREAMBLE;
    frame[1..].copy_from_slice(&b[..13]);
    let sum = frame[..12]
        .iter()
        .fold(0u32, |acc, &byte| acc + u32::from(byte));
    if sum == 0 || (sum & 0xFF) as u8 != frame[12] {
        return None;
    }
    if crc8_msb(0x31, 0x00, &frame) != 0 {
        return None;
    }
    let magnitude = f64::from(u16::from(frame[4] & 0x07) << 8 | u16::from(frame[5])) / 10.0;
    let temperature_c = if frame[4] & 0x08 == 0 {
        magnitude
    } else {
        -magnitude
    };
    let humidity = f64::from(frame[6]);
    if !plausible(temperature_c, Some(humidity)) {
        return None;
    }
    Some(SubghzReading {
        model: "WS2032".to_owned(),
        id: u32::from(frame[1]) << 8 | u32::from(frame[2]),
        battery_ok: Some(frame[3] & 0x01 == 0),
        temperature_c: Some(temperature_c),
        humidity_pct: Some(humidity),
        wind_avg_kmh: Some(f64::from(frame[7]) * 0.43 * 3.6),
        wind_max_kmh: Some(f64::from(frame[8]) * 0.43 * 3.6),
        wind_dir_deg: Some(f64::from(frame[4] >> 4) * 22.5),
        ..SubghzReading::default()
    })
}

pub(super) fn emos_e6016_rain(b: &Payload) -> Option<SubghzReading> {
    if b[0] != 0xAA || b[1] != 0xA5 || b[2] != 0x8A {
        return None;
    }
    let sum = b[..8].iter().fold(0u32, |acc, &byte| acc + u32::from(byte));
    if (sum & 0xFF) as u8 != b[8] {
        return None;
    }
    let tips = u32::from(b[6] & 0x0F) << 8 | u32::from(b[7]);
    Some(SubghzReading {
        model: "EMOS-E6016R".to_owned(),
        id: u32::from(b[3]),
        battery_ok: Some(b[4] >> 6 != 0),
        rain_mm: Some(f64::from(tips) * 0.7),
        ..SubghzReading::default()
    })
}

pub(super) fn geevon_tx163(b: &Payload) -> Option<SubghzReading> {
    if b[5] != 0xAA || b[6] != 0x55 || b[7] != 0xAA {
        return None;
    }
    if crc8_msb(0x31, 0x7B, &b[..9]) != 0 {
        return None;
    }
    let raw = u32::from(b[2]) << 4 | u32::from(b[3] >> 4);
    let temperature_c = (f64::from(raw) - 500.0) / 10.0;
    let humidity = f64::from(b[4]);
    if !plausible(temperature_c, Some(humidity)) {
        return None;
    }
    Some(SubghzReading {
        model: "Geevon-TX163".to_owned(),
        id: u32::from(b[0]),
        channel: Some(((b[1] & 0x30) >> 4) + 1),
        battery_ok: Some(b[1] & 0x80 == 0),
        temperature_c: Some(temperature_c),
        humidity_pct: Some(humidity),
        ..SubghzReading::default()
    })
}

pub(super) fn rubicson_pool(b: &Payload) -> Option<SubghzReading> {
    if b[3] & 0x0F != 0 || b[5] != 0 {
        return None;
    }
    if b[0] == 0 && b[2] == 0 && b[4] == 0 {
        return None;
    }
    if crc8_msb(0x31, 0x00, &b[..4]) != b[4] {
        return None;
    }
    let raw = u32::from(b[2] & 0x7F) << 4 | u32::from(b[3] >> 4);
    let temperature_c = (f64::from(raw) - 1_024.0) / 10.0;
    if !plausible(temperature_c, None) {
        return None;
    }
    Some(SubghzReading {
        model: "Rubicson-48942".to_owned(),
        id: u32::from(b[0] & 0x0F) << 6 | u32::from((b[1] & 0xFC) >> 2),
        channel: Some((b[0] >> 4) + 1),
        battery_ok: Some(b[2] & 0x80 == 0),
        temperature_c: Some(temperature_c),
        ..SubghzReading::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(bytes: &[u8]) -> Payload {
        let mut out = [0u8; PAYLOAD_BYTES];
        out[..bytes.len()].copy_from_slice(bytes);
        out
    }

    #[test]
    fn a_nexus_payload_reads_back_the_values_it_was_built_from() {
        let reading = nexus(&payload(&[0x8F, 0x80, 0xD5, 0xF2, 0xF0])).expect("valid Nexus");
        assert_eq!(reading.model, "Nexus-TH");
        assert_eq!(reading.id, 0x8F);
        assert_eq!(reading.channel, Some(1));
        assert_eq!(reading.battery_ok, Some(true));
        assert_eq!(reading.temperature_c, Some(21.3));
        assert_eq!(reading.humidity_pct, Some(47.0));
    }

    #[test]
    fn a_nexus_payload_carries_temperature_below_freezing_as_a_signed_value() {
        let reading = nexus(&payload(&[0x5C, 0x2F, 0xB5, 0xF5, 0x80])).expect("valid Nexus");
        assert_eq!(reading.temperature_c, Some(-7.5));
        assert_eq!(reading.humidity_pct, Some(88.0));
        assert_eq!(reading.channel, Some(3));
        assert_eq!(reading.battery_ok, Some(false));
    }

    #[test]
    fn a_nexus_sensor_without_a_hygrometer_reports_only_temperature() {
        let reading = nexus(&payload(&[0xA3, 0x90, 0x1E, 0xF0, 0x00])).expect("valid Nexus");
        assert_eq!(reading.model, "Nexus-T");
        assert_eq!(reading.temperature_c, Some(3.0));
        assert_eq!(reading.humidity_pct, None);
    }

    #[test]
    fn nexus_rejects_a_payload_whose_constant_nibble_is_wrong() {
        assert!(nexus(&payload(&[0x8F, 0x80, 0xD5, 0xE2, 0xF0])).is_none());
    }

    #[test]
    fn nexus_rejects_the_fourth_channel_its_dial_cannot_select() {
        assert!(nexus(&payload(&[0x8F, 0xB0, 0xD5, 0xF2, 0xF0])).is_none());
    }

    #[test]
    fn nexus_rejects_a_humidity_no_hygrometer_could_report() {
        assert!(nexus(&payload(&[0x8F, 0x80, 0xD5, 0xFF, 0xF0])).is_none());
    }

    #[test]
    fn an_acurite_609_payload_reads_back_the_values_it_was_built_from() {
        let reading =
            acurite_609txc(&payload(&[0xD4, 0xA0, 0xF6, 0x3D, 0xA7])).expect("valid Acurite");
        assert_eq!(reading.model, "Acurite-609TXC");
        assert_eq!(reading.id, 0xD4);
        assert_eq!(reading.battery_ok, Some(false));
        assert_eq!(reading.temperature_c, Some(24.6));
        assert_eq!(reading.humidity_pct, Some(61.0));
    }

    #[test]
    fn acurite_609_rejects_a_payload_whose_checksum_does_not_add_up() {
        assert!(acurite_609txc(&payload(&[0x2A, 0x2F, 0xB5, 0x58, 0x67])).is_none());
        assert_eq!(
            acurite_609txc(&payload(&[0x2A, 0x2F, 0xB5, 0x58, 0x66]))
                .expect("valid Acurite")
                .temperature_c,
            Some(-7.5)
        );
    }

    #[test]
    fn an_acurite_606_payload_reads_back_the_values_it_was_built_from() {
        let reading = acurite_606tx(&payload(&[0x7B, 0x80, 0xBB, 0x76])).expect("valid Acurite");
        assert_eq!(reading.model, "Acurite-606TX");
        assert_eq!(reading.id, 0x7B);
        assert_eq!(reading.channel, Some(1));
        assert_eq!(reading.battery_ok, Some(true));
        assert_eq!(reading.temperature_c, Some(18.7));
        assert_eq!(reading.humidity_pct, None);
    }

    #[test]
    fn an_acurite_606_reads_a_temperature_below_freezing_and_a_flat_battery() {
        let reading = acurite_606tx(&payload(&[0x2E, 0x2F, 0x84, 0x0D])).expect("valid Acurite");
        assert_eq!(reading.temperature_c, Some(-12.4));
        assert_eq!(reading.battery_ok, Some(false));
        assert_eq!(reading.channel, Some(3));
    }

    #[test]
    fn acurite_606_rejects_every_single_bit_flip_in_its_payload() {
        let clean = [0x7Bu8, 0x80, 0xBB, 0x76];
        for bit in 0..32 {
            let mut damaged = clean;
            damaged[bit / 8] ^= 0x80 >> (bit % 8);
            assert!(
                acurite_606tx(&payload(&damaged)).is_none(),
                "bit {bit} slipped through"
            );
        }
    }

    #[test]
    fn a_lacrosse_payload_reads_back_the_values_it_was_built_from() {
        let reading = lacrosse_tx141th_bv2(&payload(&[0x8F, 0x12, 0xC9, 0x2F, 0x9F]))
            .expect("valid LaCrosse");
        assert_eq!(reading.model, "LaCrosse-TX141THBv2");
        assert_eq!(reading.id, 0x8F);
        assert_eq!(reading.channel, Some(1));
        assert_eq!(reading.battery_ok, Some(true));
        assert_eq!(reading.temperature_c, Some(21.3));
        assert_eq!(reading.humidity_pct, Some(47.0));
    }

    #[test]
    fn a_lacrosse_payload_offsets_temperature_below_freezing() {
        let reading = lacrosse_tx141th_bv2(&payload(&[0x2A, 0xA1, 0xA9, 0x58, 0x23]))
            .expect("valid LaCrosse");
        assert_eq!(reading.temperature_c, Some(-7.5));
        assert_eq!(reading.channel, Some(2));
        assert_eq!(reading.battery_ok, Some(false));
    }

    #[test]
    fn lacrosse_rejects_every_single_bit_flip_in_its_payload() {
        let clean = [0x8Fu8, 0x12, 0xC9, 0x2F, 0x9F];
        for bit in 0..40 {
            let mut damaged = clean;
            damaged[bit / 8] ^= 0x80 >> (bit % 8);
            assert!(
                lacrosse_tx141th_bv2(&payload(&damaged)).is_none(),
                "bit {bit} slipped through"
            );
        }
    }

    #[test]
    fn a_prologue_payload_reads_back_the_values_it_was_built_from() {
        let reading = prologue(&payload(&[0x9C, 0x79, 0x0E, 0xA3, 0x70])).expect("valid Prologue");
        assert_eq!(reading.model, "Prologue-TH");
        assert_eq!(reading.id, 0xC7);
        assert_eq!(reading.channel, Some(2));
        assert_eq!(reading.battery_ok, Some(true));
        assert_eq!(reading.temperature_c, Some(23.4));
        assert_eq!(reading.humidity_pct, Some(55.0));
    }

    #[test]
    fn a_prologue_sensor_without_a_hygrometer_sends_the_fixed_humidity_byte() {
        let reading = prologue(&payload(&[0x53, 0x10, 0xFC, 0xCC, 0xC0])).expect("valid Prologue");
        assert_eq!(reading.temperature_c, Some(-5.2));
        assert_eq!(reading.humidity_pct, None);
        assert_eq!(reading.battery_ok, Some(false));
    }

    #[test]
    fn prologue_rejects_a_payload_whose_type_nibble_is_neither_of_its_two() {
        assert!(prologue(&payload(&[0x7C, 0x79, 0x0E, 0xA3, 0x70])).is_none());
    }

    #[test]
    fn an_infactory_payload_reads_back_the_values_it_was_built_from() {
        let reading =
            infactory(&payload(&[0x0F, 0x80, 0x65, 0x06, 0x23])).expect("valid inFactory");
        assert_eq!(reading.model, "inFactory-TH");
        assert_eq!(reading.id, 0x0F);
        assert_eq!(reading.channel, Some(3));
        assert_eq!(reading.temperature_c, Some(22.0));
        assert_eq!(reading.humidity_pct, Some(62.0));
    }

    #[test]
    fn infactory_rejects_a_payload_whose_crc_nibble_is_wrong() {
        assert!(infactory(&payload(&[0x0F, 0x90, 0x65, 0x06, 0x23])).is_none());
    }

    #[test]
    fn a_fine_offset_payload_reads_back_the_values_it_was_built_from() {
        let reading =
            fineoffset_wh2(&payload(&[0xFF, 0x45, 0xA0, 0xC3, 0x49, 0xEB])).expect("valid WH2");
        assert_eq!(reading.model, "FineOffset-WH2");
        assert_eq!(reading.id, 0x5A);
        assert_eq!(reading.temperature_c, Some(19.5));
        assert_eq!(reading.humidity_pct, Some(73.0));
    }

    #[test]
    fn fine_offset_reads_a_negative_temperature_as_sign_and_magnitude() {
        let reading =
            fineoffset_wh2(&payload(&[0xFF, 0x45, 0xA8, 0x26, 0x5B, 0x99])).expect("valid WH2");
        assert_eq!(reading.temperature_c, Some(-3.8));
        assert_eq!(reading.humidity_pct, Some(91.0));
    }

    #[test]
    fn fine_offset_rejects_a_row_that_does_not_open_with_its_preamble() {
        assert!(fineoffset_wh2(&payload(&[0xFE, 0x45, 0xA0, 0xC3, 0x49, 0xEB])).is_none());
    }

    #[test]
    fn an_f007th_payload_reads_back_the_values_it_was_built_from() {
        let reading = ambientweather_f007th(&payload(&[0x45, 0x93, 0x24, 0x41, 0x30, 0x6F]))
            .expect("valid F007TH");
        assert_eq!(reading.model, "AmbientWeather-F007TH");
        assert_eq!(reading.id, 0x93);
        assert_eq!(reading.channel, Some(3));
        assert_eq!(reading.battery_ok, Some(true));
        assert_eq!(reading.temperature_c, Some(20.5));
        assert_eq!(reading.humidity_pct, Some(48.0));
    }

    #[test]
    fn f007th_rejects_every_single_bit_flip_in_its_payload() {
        let clean = [0x45u8, 0x93, 0x24, 0x41, 0x30, 0x6F];
        for bit in 0..48 {
            let mut damaged = clean;
            damaged[bit / 8] ^= 0x80 >> (bit % 8);
            assert!(
                ambientweather_f007th(&payload(&damaged)).is_none(),
                "bit {bit} slipped through"
            );
        }
    }

    #[test]
    fn a_wh31e_payload_reads_back_the_values_it_was_built_from() {
        let reading = ambientweather_wh31e(&payload(&[0x30, 0xC3, 0x82, 0x73, 0x33, 0xD0, 0xEB]))
            .expect("valid WH31E");
        assert_eq!(reading.model, "AmbientWeather-WH31E");
        assert_eq!(reading.id, 0xC3);
        assert_eq!(reading.channel, Some(1));
        assert_eq!(reading.battery_ok, Some(true));
        assert_eq!(reading.temperature_c, Some(22.7));
        assert_eq!(reading.humidity_pct, Some(51.0));
    }

    #[test]
    fn a_wh31e_reads_a_temperature_below_freezing_and_a_flat_battery() {
        let reading = ambientweather_wh31e(&payload(&[0x30, 0x44, 0xA5, 0x35, 0x54, 0xD6, 0x78]))
            .expect("valid WH31E");
        assert_eq!(reading.temperature_c, Some(-9.1));
        assert_eq!(reading.channel, Some(3));
        assert_eq!(reading.battery_ok, Some(false));
        assert_eq!(reading.humidity_pct, Some(84.0));
    }

    #[test]
    fn wh31e_rejects_a_payload_whose_trailing_sum_is_wrong() {
        assert!(
            ambientweather_wh31e(&payload(&[0x30, 0xC3, 0x82, 0x73, 0x33, 0xD0, 0xEC])).is_none()
        );
    }

    #[test]
    fn a_rubicson_payload_reads_back_the_values_it_was_built_from() {
        let reading = rubicson(&payload(&[0xB4, 0x80, 0xA8, 0xFA, 0xA0])).expect("valid Rubicson");
        assert_eq!(reading.model, "Rubicson-Temperature");
        assert_eq!(reading.id, 0xB4);
        assert_eq!(reading.channel, Some(1));
        assert_eq!(reading.battery_ok, Some(true));
        assert_eq!(reading.temperature_c, Some(16.8));
    }

    #[test]
    fn a_rubicson_reads_a_deep_frost_and_its_third_channel() {
        let reading = rubicson(&payload(&[0x7E, 0x2F, 0x2B, 0xFB, 0x30])).expect("valid Rubicson");
        assert_eq!(reading.temperature_c, Some(-21.3));
        assert_eq!(reading.channel, Some(3));
        assert_eq!(reading.battery_ok, Some(false));
    }

    #[test]
    fn rubicson_rejects_every_single_bit_flip_in_its_payload() {
        let clean = [0xB4u8, 0x80, 0xA8, 0xFA, 0xA0];
        for bit in 0..36 {
            let mut damaged = clean;
            damaged[bit / 8] ^= 0x80 >> (bit % 8);
            assert!(
                rubicson(&payload(&damaged)).is_none(),
                "bit {bit} slipped through"
            );
        }
    }

    #[test]
    fn a_kedsum_payload_reads_back_the_values_it_was_built_from() {
        let reading = kedsum(&payload(&[0x3C, 0x90, 0x56, 0xE3, 0x02])).expect("valid Kedsum");
        assert_eq!(reading.model, "Kedsum-TH");
        assert_eq!(reading.id, 0x3C);
        assert_eq!(reading.channel, Some(2));
        assert_eq!(reading.battery_ok, Some(true));
        assert_eq!(reading.temperature_c, Some(22.0));
        assert_eq!(reading.humidity_pct, Some(62.0));
    }

    #[test]
    fn kedsum_rejects_a_payload_whose_crc_nibble_is_wrong() {
        assert!(kedsum(&payload(&[0x3C, 0x90, 0x56, 0xE3, 0x03])).is_none());
    }

    #[test]
    fn a_springfield_payload_reports_soil_moisture_beside_temperature() {
        let reading =
            springfield(&payload(&[0x66, 0x10, 0xC2, 0x69, 0x00])).expect("valid Springfield");
        assert_eq!(reading.model, "Springfield-Soil");
        assert_eq!(reading.temperature_c, Some(19.4));
        assert_eq!(reading.moisture_pct, Some(60.0));
        assert_eq!(reading.humidity_pct, None);
        assert_eq!(reading.channel, Some(2));
    }

    #[test]
    fn springfield_rejects_a_payload_whose_folded_parity_does_not_close() {
        assert!(springfield(&payload(&[0x66, 0x10, 0xC2, 0x68, 0x00])).is_none());
    }

    #[test]
    fn an_auriol_payload_reads_back_the_values_it_was_built_from() {
        let reading =
            auriol_hg02832(&payload(&[0x91, 0x26, 0x00, 0xF1, 0xB6])).expect("valid Auriol");
        assert_eq!(reading.model, "Auriol-HG02832");
        assert_eq!(reading.id, 0x91);
        assert_eq!(reading.channel, Some(1));
        assert_eq!(reading.temperature_c, Some(24.1));
        assert_eq!(reading.humidity_pct, Some(38.0));
    }

    #[test]
    fn auriol_rejects_every_single_bit_flip_in_its_payload() {
        let clean = [0x91u8, 0x26, 0x00, 0xF1, 0xB6];
        for bit in 0..40 {
            let mut damaged = clean;
            damaged[bit / 8] ^= 0x80 >> (bit % 8);
            assert!(
                auriol_hg02832(&payload(&damaged)).is_none(),
                "bit {bit} slipped through"
            );
        }
    }

    #[test]
    fn a_wt0124_payload_passes_both_of_the_checks_it_carries() {
        let reading =
            wt0124(&payload(&[0x55, 0xCA, 0x99, 0x20, 0x26, 0xFF, 0x00])).expect("valid WT0124");
        assert_eq!(reading.model, "WT0124-Pool");
        assert_eq!(reading.id, 0x5A);
        assert_eq!(reading.channel, Some(2));
        assert_eq!(reading.temperature_c, Some(26.5));
    }

    #[test]
    fn wt0124_rejects_a_payload_that_passes_the_xor_but_not_the_sum() {
        assert!(wt0124(&payload(&[0x55, 0xCA, 0x99, 0x20, 0x26, 0xFE, 0x00])).is_none());
    }

    #[test]
    fn an_opus_payload_reports_soil_moisture_and_a_whole_degree() {
        let reading =
            opus_xt300(&payload(&[0xFF, 0x51, 0x2F, 0x3D, 0x00, 0xBD])).expect("valid Opus XT300");
        assert_eq!(reading.model, "Opus-XT300");
        assert_eq!(reading.temperature_c, Some(21.0));
        assert_eq!(reading.moisture_pct, Some(47.0));
        assert_eq!(reading.channel, Some(1));
    }

    #[test]
    fn opus_rejects_a_payload_whose_running_sum_is_wrong() {
        assert!(opus_xt300(&payload(&[0xFF, 0x51, 0x2F, 0x3D, 0x00, 0xBE])).is_none());
    }

    #[test]
    fn the_rubicson_family_is_not_read_as_the_nexus_it_shares_a_layout_with() {
        let frame = payload(&[0xB4, 0x80, 0xA8, 0xFA, 0xA0]);
        assert!(rubicson(&frame).is_some());
        assert!(
            nexus(&frame).is_none(),
            "Nexus must defer to the family whose CRC closes"
        );
    }

    #[test]
    fn a_renault_tyre_sensor_reports_pressure_in_kilopascals() {
        let reading = tpms_renault(&payload(&[
            0x35, 0x34, 0x30, 0x93, 0x1A, 0x4C, 0xFF, 0xFF, 0x04,
        ]))
        .expect("valid Renault TPMS");
        assert_eq!(reading.model, "Renault-TPMS");
        assert_eq!(reading.id, 0x4C_1A93);
        assert_eq!(reading.temperature_c, Some(18.0));
        assert_eq!(reading.pressure_kpa, Some(231.0));
        assert_eq!(reading.humidity_pct, None);
    }

    #[test]
    fn a_toyota_tyre_sensor_converts_its_pressure_out_of_psi() {
        let reading = tpms_toyota(&payload(&[
            0x1A, 0x2B, 0x3C, 0x4D, 0x4E, 0x1F, 0x00, 0x63, 0xFD,
        ]))
        .expect("valid Toyota TPMS");
        assert_eq!(reading.model, "Toyota-TPMS");
        assert_eq!(reading.id, 0x1A2B_3C4D);
        assert_eq!(reading.temperature_c, Some(22.0));
        let kpa = reading.pressure_kpa.expect("a pressure");
        assert!((kpa - 220.63).abs() < 0.01, "pressure {kpa} kPa");
    }

    #[test]
    fn toyota_rejects_a_frame_whose_two_pressure_copies_disagree() {
        assert!(
            tpms_toyota(&payload(&[
                0x1A, 0x2B, 0x3C, 0x4D, 0x4E, 0x1F, 0x00, 0x64, 0xFD
            ]))
            .is_none()
        );
    }

    #[test]
    fn a_ws2032_reports_wind_beside_its_temperature() {
        let reading = ws2032(&payload(&[
            0x2C, 0x41, 0x00, 0x60, 0xAE, 0x3F, 0x0C, 0x15, 0x00, 0x01, 0x31, 0x02, 0xDB,
        ]))
        .expect("valid WS2032");
        assert_eq!(reading.model, "WS2032");
        assert_eq!(reading.id, 0x2C41);
        assert_eq!(reading.temperature_c, Some(17.4));
        assert_eq!(reading.humidity_pct, Some(63.0));
        assert_eq!(reading.wind_dir_deg, Some(135.0));
        let avg = reading.wind_avg_kmh.expect("an average wind speed");
        let gust = reading.wind_max_kmh.expect("a gust");
        assert!((avg - 18.576).abs() < 1e-3, "average {avg}");
        assert!(gust > avg, "gust {gust} should exceed the average {avg}");
    }

    #[test]
    fn ws2032_rejects_a_frame_whose_trailing_crc_does_not_close() {
        assert!(
            ws2032(&payload(&[
                0x2C, 0x41, 0x00, 0x60, 0xAE, 0x3F, 0x0C, 0x15, 0x00, 0x01, 0x31, 0x02, 0xDC,
            ]))
            .is_none()
        );
    }

    #[test]
    fn an_emos_rain_gauge_reports_millimetres_and_no_temperature() {
        let reading = emos_e6016_rain(&payload(&[
            0xAA, 0xA5, 0x8A, 0x34, 0xC0, 0x00, 0x01, 0x2B, 0xF9,
        ]))
        .expect("valid EMOS rain gauge");
        assert_eq!(reading.model, "EMOS-E6016R");
        assert_eq!(reading.id, 0x34);
        assert_eq!(reading.rain_mm, Some(209.29999999999998));
        assert_eq!(reading.temperature_c, None);
        assert_eq!(reading.battery_ok, Some(true));
    }

    #[test]
    fn a_geevon_payload_matches_the_worked_example_its_spec_publishes() {
        let reading = geevon_tx163(&payload(&[
            0x87, 0x00, 0x29, 0xE0, 0x2B, 0xAA, 0x55, 0xAA, 0x69,
        ]))
        .expect("valid Geevon");
        assert_eq!(reading.model, "Geevon-TX163");
        assert_eq!(reading.id, 0x87);
        assert_eq!(reading.channel, Some(1));
        assert_eq!(reading.temperature_c, Some(17.0));
        assert_eq!(reading.humidity_pct, Some(43.0));
    }

    #[test]
    fn geevon_rejects_a_frame_whose_fixed_bytes_are_not_where_they_belong() {
        assert!(
            geevon_tx163(&payload(&[
                0x87, 0x00, 0x29, 0xE0, 0x2B, 0xAB, 0x55, 0xAA, 0x69
            ]))
            .is_none()
        );
    }

    #[test]
    fn a_rubicson_pool_payload_reads_back_the_values_it_was_built_from() {
        let reading =
            rubicson_pool(&payload(&[0x1A, 0x9C, 0x4E, 0xF0, 0x4F, 0x00])).expect("valid pool");
        assert_eq!(reading.model, "Rubicson-48942");
        assert_eq!(reading.id, 0x2A7);
        assert_eq!(reading.channel, Some(2));
        assert_eq!(reading.temperature_c, Some(23.9));
    }

    #[test]
    fn rubicson_pool_rejects_a_frame_whose_crc_byte_is_wrong() {
        assert!(rubicson_pool(&payload(&[0x1A, 0x9C, 0x4E, 0xF0, 0x50, 0x00])).is_none());
    }

    #[test]
    fn a_wt450_payload_reads_back_the_values_it_was_built_from() {
        let reading = wt450(&payload(&[0xC3, 0x42, 0xD4, 0x78, 0x50])).expect("valid WT450");
        assert_eq!(reading.model, "WT450-TH");
        assert_eq!(reading.id, 3);
        assert_eq!(reading.channel, Some(2));
        assert_eq!(reading.battery_ok, Some(true));
        assert_eq!(reading.temperature_c, Some(21.5));
        assert_eq!(reading.humidity_pct, Some(45.0));
    }

    #[test]
    fn a_wt450_reads_a_fractional_temperature_below_freezing() {
        let reading = wt450(&payload(&[0xC9, 0x0D, 0x82, 0xDC, 0x90])).expect("valid WT450");
        assert_eq!(reading.temperature_c, Some(-4.25));
        assert_eq!(reading.humidity_pct, Some(88.0));
        assert_eq!(reading.battery_ok, Some(false));
        assert_eq!(reading.channel, Some(1));
    }

    #[test]
    fn wt450_rejects_a_payload_whose_parity_bits_do_not_close() {
        assert!(wt450(&payload(&[0xC3, 0x42, 0xD4, 0x78, 0x60])).is_none());
    }
}
