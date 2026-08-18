use sdrmm_dsp::{crc4_msb, crc8_msb, lfsr_digest8, lfsr_digest8_reflect};
use sdrmm_wire::SubghzReading;

pub(super) const PAYLOAD_BYTES: usize = 8;

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
    if b[0] & 0x0F != 0x05 {
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
