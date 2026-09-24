use sdrmm_dsp::crc16_x25;
use serde_json::{Value, json};

use super::acars_block::{self, AcarsBlock};

pub(super) const SU_LEN: usize = 12;
const PENDING_MAX_AGE: u32 = 10;
const ISU: u8 = 0x71;
const SSU_MASK: u8 = 0xC0;
const RECEIVE_BASE_MHZ: f64 = 1510.0;
const TRANSMIT_BASE_MHZ: f64 = 1611.5;
const RETURN_OFFSET_MHZ: f64 = 101.5;
const CHANNEL_STEP_MHZ: f64 = 0.0025;

pub(super) fn su_crc_ok(su: &[u8]) -> bool {
    if su.len() != SU_LEN {
        return false;
    }
    if su.iter().all(|&byte| byte == 0) {
        return true;
    }
    crc16_x25(&su[..10]) == u16::from_le_bytes([su[10], su[11]])
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct AeroUserData {
    pub aes_id: String,
    pub ges_id: u8,
    pub qno: u8,
    pub refno: u8,
    pub data: Vec<u8>,
}

struct PendingIsu {
    aes_id: u32,
    ges_id: u8,
    qno: u8,
    refno: u8,
    seq_remaining: u8,
    last_ssu_octets: u8,
    data: Vec<u8>,
    age: u32,
}

impl PendingIsu {
    fn finish(self) -> AeroUserData {
        AeroUserData {
            aes_id: format!("{:06X}", self.aes_id),
            ges_id: self.ges_id,
            qno: self.qno,
            refno: self.refno,
            data: self.data,
        }
    }
}

#[derive(Default)]
pub(super) struct Reassembler {
    pending: Vec<PendingIsu>,
}

impl Reassembler {
    pub(super) fn push(&mut self, su: &[u8]) -> Option<AeroUserData> {
        let kind = *su.first()?;
        for pending in &mut self.pending {
            pending.age += 1;
        }
        self.pending.retain(|pending| pending.age < PENDING_MAX_AGE);
        if kind == ISU {
            return self.start(su);
        }
        if kind & SSU_MASK == SSU_MASK {
            return self.extend(su);
        }
        None
    }

    fn start(&mut self, su: &[u8]) -> Option<AeroUserData> {
        let isu = PendingIsu {
            aes_id: u32::from_be_bytes([0, su[1], su[2], su[3]]),
            ges_id: su[4],
            qno: su[5] >> 4,
            refno: su[5] & 0x0F,
            seq_remaining: su[6] & 0x3F,
            last_ssu_octets: (su[7] >> 4) & 0x0F,
            data: su[8..10].to_vec(),
            age: 0,
        };
        if isu.seq_remaining == 0 {
            return Some(isu.finish());
        }
        self.pending.push(isu);
        None
    }

    fn extend(&mut self, su: &[u8]) -> Option<AeroUserData> {
        let seq = su[0] & 0x3F;
        let qno = su[1] >> 4;
        let refno = su[1] & 0x0F;
        let index = self.pending.iter().position(|pending| {
            pending.seq_remaining == seq + 1 && pending.qno == qno && pending.refno == refno
        })?;
        let pending = &mut self.pending[index];
        pending.seq_remaining = seq;
        pending.age = 0;
        if seq == 0 {
            let take = usize::from(pending.last_ssu_octets.min(8));
            pending.data.extend_from_slice(&su[2..2 + take]);
            return Some(self.pending.swap_remove(index).finish());
        }
        pending.data.extend_from_slice(&su[2..10]);
        None
    }
}

pub(super) fn parse_acars(data: &[u8]) -> Option<AcarsBlock> {
    let start = data.iter().position(|&byte| byte != 0xFF)?;
    acars_block::parse(&data[start..])
}

pub(super) fn parse_p_su(su: &[u8]) -> Option<Value> {
    if su.len() < SU_LEN {
        return None;
    }
    match su[0] {
        0x31..=0x34 => Some(c_assignment(su)),
        0x10..=0x17 => log_control(su),
        0x21 => Some(channel_pair(su, "call-announcement")),
        0x51 => Some(t_channel_assignment(su)),
        0x05 => Some(smc_channels(su)),
        0x07 => Some(named("ges-beam-support")),
        0x0A => Some(named("broadcast-index")),
        0x0C => Some(satellite_id(su)),
        0x28 => Some(named("eirp-table-broadcast")),
        0x40 => Some(pr_control_isu(su)),
        0x41 => Some(named("t-channel-control-isu")),
        0x61 => Some(named("request-for-acknowledgement")),
        0x62 => Some(named("acknowledge")),
        0x74 => Some(short_lsdu(3)),
        0x76 => Some(short_lsdu(4)),
        _ => None,
    }
}

pub(super) fn p_su_kind(value: &Value) -> String {
    value["su_type"].as_str().unwrap_or("aero-su").to_owned()
}

fn named(su_type: &str) -> Value {
    json!({ "su_type": su_type })
}

fn short_lsdu(octets: u8) -> Value {
    json!({ "su_type": "short-lsdu", "lsdu_octets": octets })
}

fn channel_mhz(high: u8, low: u8) -> f64 {
    f64::from((u32::from(high & 0x7F) << 8) | u32::from(low)) * CHANNEL_STEP_MHZ
}

fn aes_ges(su: &[u8]) -> (String, u8) {
    (
        format!("{:06X}", u32::from_be_bytes([0, su[1], su[2], su[3]])),
        su[4],
    )
}

fn c_assignment(su: &[u8]) -> Value {
    let service = match su[0] {
        0x31 => "distress",
        0x32 => "flight-safety",
        0x33 => "other-safety",
        _ => "non-safety",
    };
    let mut value = channel_pair(su, "c-channel-assignment");
    value["service"] = json!(service);
    value
}

fn channel_pair(su: &[u8], su_type: &str) -> Value {
    let (aes_id, ges_id) = aes_ges(su);
    json!({
        "su_type": su_type,
        "aes_id": aes_id,
        "ges_id": ges_id,
        "receive_mhz": channel_mhz(su[6], su[7]) + RECEIVE_BASE_MHZ,
        "transmit_mhz": channel_mhz(su[8], su[9]) + TRANSMIT_BASE_MHZ,
        "receive_spotbeam": su[6] & 0x80 != 0,
        "transmit_spotbeam": su[8] & 0x80 != 0,
    })
}

fn log_control(su: &[u8]) -> Option<Value> {
    let (event, direction) = match su[0] {
        0x10 => ("log-on-request", "aes-to-ges"),
        0x12 => ("log-off-request", "aes-to-ges"),
        0x11 => ("log-on-confirm", "ges-to-aes"),
        0x13 => ("log-on-reject", "ges-to-aes"),
        0x14 => ("log-on-interrogation", "ges-to-aes"),
        0x16 => ("log-on-prompt", "ges-to-aes"),
        0x17 => ("data-channel-reassignment", "ges-to-aes"),
        0x15 => ("log-on-log-off-acknowledge", "either"),
        _ => return None,
    };
    let (aes_id, ges_id) = aes_ges(su);
    Some(json!({
        "su_type": "log-control",
        "su_type_hex": format!("0x{:02X}", su[0]),
        "event": event,
        "direction": direction,
        "aes_id": aes_id,
        "ges_id": ges_id,
    }))
}

fn t_channel_assignment(su: &[u8]) -> Value {
    let (aes_id, ges_id) = aes_ges(su);
    json!({
        "su_type": "t-channel-assignment",
        "aes_id": aes_id,
        "ges_id": ges_id,
    })
}

fn control_isu_bitrate(code: u8) -> Option<u32> {
    match code {
        0 => Some(600),
        1 => Some(1200),
        2 => Some(2400),
        3 => Some(4800),
        4 => Some(6000),
        5 => Some(5250),
        6 => Some(10500),
        7 => Some(8400),
        9 => Some(21000),
        _ => None,
    }
}

fn pr_control_isu(su: &[u8]) -> Value {
    let mut value = json!({
        "su_type": "pr-channel-control-isu",
        "ges_id": su[4],
        "pd_mhz": channel_mhz(su[8], su[9]) + RECEIVE_BASE_MHZ,
        "spotbeam": su[8] & 0x80 != 0,
    });
    if let Some(bit_rate) = control_isu_bitrate((su[7] >> 4) & 0x0F) {
        value["bit_rate"] = json!(bit_rate);
    }
    value
}

fn satellite_id(su: &[u8]) -> Value {
    let byte3 = u16::from(su[2]);
    let byte4 = u16::from(su[3]);
    let longitude = f64::from(su[5]) * 1.5;
    let (longitude_deg, longitude_dir) = if longitude > 180.0 {
        (360.0 - longitude, "W")
    } else {
        (longitude, "E")
    };
    let mut value = json!({
        "su_type": "satellite-id",
        "seq": (byte3 >> 2) & 0x3F,
        "satellite_id": ((byte3 << 4) & 0x30) | ((byte4 >> 4) & 0x0F),
        "longitude_deg": longitude_deg,
        "longitude_dir": longitude_dir,
        "psmc1_mhz": channel_mhz(su[6], su[7]) + RECEIVE_BASE_MHZ,
        "psmc1_spotbeam": su[6] & 0x80 != 0,
    });
    let second = channel_mhz(su[8], su[9]);
    if second != 0.0 {
        value["psmc2_mhz"] = json!(second + RECEIVE_BASE_MHZ);
        value["psmc2_spotbeam"] = json!(su[8] & 0x80 != 0);
    }
    value
}

fn smc_channels(su: &[u8]) -> Value {
    let lsu = su[2] & 0x03;
    let raw = |high: u8, low: u8| f64::from((u32::from(high) << 8) | u32::from(low));
    let mut frequencies = [
        raw(su[4], su[5]) * CHANNEL_STEP_MHZ + RECEIVE_BASE_MHZ,
        raw(su[6], su[7]) * CHANNEL_STEP_MHZ + RECEIVE_BASE_MHZ,
        raw(su[8], su[9]) * CHANNEL_STEP_MHZ + RECEIVE_BASE_MHZ,
    ];
    let names = match lsu {
        0 | 1 => ["psmc_rx", "rsmc0_tx", "rsmc1_tx"],
        2 => ["rsmc2_tx", "rsmc3_tx", "rsmc4_tx"],
        _ => ["rsmc5_tx", "rsmc6_tx", "rsmc7_tx"],
    };
    let first_return = if lsu <= 1 { 1 } else { 0 };
    for frequency in &mut frequencies[first_return..] {
        *frequency += RETURN_OFFSET_MHZ;
    }
    json!({
        "su_type": "smc-channels",
        "seq": (su[2] >> 2) & 0x3F,
        "lsu": lsu,
        "ges_id": su[3],
        "channels": [
            { "name": names[0], "mhz": frequencies[0] },
            { "name": names[1], "mhz": frequencies[1] },
            { "name": names[2], "mhz": frequencies[2] },
        ],
    })
}

#[cfg(test)]
pub(super) use builders::{build_isu_chain, fill_su, su_with_crc};

#[cfg(test)]
mod builders {
    use super::*;

    pub fn su_with_crc(mut su10: Vec<u8>) -> Vec<u8> {
        let crc = crc16_x25(&su10);
        su10.extend(crc.to_le_bytes());
        su10
    }

    pub fn fill_su() -> Vec<u8> {
        su_with_crc(vec![0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0])
    }

    pub fn build_isu_chain(
        aes_id: u32,
        ges_id: u8,
        qno: u8,
        refno: u8,
        data: &[u8],
    ) -> Vec<Vec<u8>> {
        let rest = data.len().saturating_sub(2);
        let ssu_count = rest.div_ceil(8);
        let last_octets = if rest == 0 {
            0
        } else {
            rest - (ssu_count - 1) * 8
        };
        let aes = aes_id.to_be_bytes();
        let reference = (qno << 4) | (refno & 0x0F);
        let mut units = vec![su_with_crc(vec![
            ISU,
            aes[1],
            aes[2],
            aes[3],
            ges_id,
            reference,
            ssu_count as u8 & 0x3F,
            ((last_octets as u8) << 4) & 0xF0,
            data.first().copied().unwrap_or(0),
            data.get(1).copied().unwrap_or(0),
        ])];
        for index in 0..ssu_count {
            let mut ssu = vec![SSU_MASK | (ssu_count - 1 - index) as u8, reference];
            let offset = 2 + index * 8;
            ssu.extend((0..8).map(|byte| data.get(offset + byte).copied().unwrap_or(0)));
            units.push(su_with_crc(ssu));
        }
        units
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(type_byte: u8) -> Value {
        let mut su10 = vec![0u8; 10];
        su10[0] = type_byte;
        parse_p_su(&su_with_crc(su10)).unwrap_or(Value::Null)
    }

    fn with_addressing(type_byte: u8, aes: [u8; 3], ges: u8) -> Vec<u8> {
        let mut su10 = vec![0u8; 10];
        su10[0] = type_byte;
        su10[1..4].copy_from_slice(&aes);
        su10[4] = ges;
        su10
    }

    #[test]
    fn aero_type_enumerators_match_jaero() {
        for reserved in [0x00u8, 0x01, 0x18, 0x19, 0x26, 0x30, 0x71] {
            assert!(p(reserved).is_null(), "P 0x{reserved:02X}");
        }
        assert_eq!(p(0x05)["su_type"], "smc-channels");
        assert_eq!(p(0x07)["su_type"], "ges-beam-support");
        assert_eq!(p(0x0A)["su_type"], "broadcast-index");
        assert_eq!(p(0x0C)["su_type"], "satellite-id");
        assert_eq!(p(0x21)["su_type"], "call-announcement");
        assert_eq!(p(0x28)["su_type"], "eirp-table-broadcast");
        for (type_byte, service) in [
            (0x31u8, "distress"),
            (0x32, "flight-safety"),
            (0x33, "other-safety"),
            (0x34, "non-safety"),
        ] {
            assert_eq!(p(type_byte)["su_type"], "c-channel-assignment");
            assert_eq!(p(type_byte)["service"], service);
        }
        assert_eq!(p(0x40)["su_type"], "pr-channel-control-isu");
        assert_eq!(p(0x41)["su_type"], "t-channel-control-isu");
        assert_eq!(p(0x51)["su_type"], "t-channel-assignment");
        assert_eq!(p(0x61)["su_type"], "request-for-acknowledgement");
        assert_eq!(p(0x62)["su_type"], "acknowledge");
        assert_eq!(p(0x74)["lsdu_octets"], 3);
        assert_eq!(p(0x76)["lsdu_octets"], 4);
    }

    #[test]
    fn c_assignment_parses_frequencies() {
        let mut su10 = with_addressing(0x32, [0xAB, 0xCD, 0xEF], 0x44);
        su10[6] = 0x80 | (4000u16 >> 8) as u8;
        su10[7] = (4000u16 & 0xFF) as u8;
        su10[8] = (2000u16 >> 8) as u8;
        su10[9] = (2000u16 & 0xFF) as u8;
        let value = parse_p_su(&su_with_crc(su10)).expect("parses");
        assert_eq!(value["service"], "flight-safety");
        assert_eq!(value["aes_id"], "ABCDEF");
        assert_eq!(value["ges_id"], 0x44);
        assert_eq!(value["receive_mhz"], 1520.0);
        assert_eq!(value["transmit_mhz"], 1616.5);
        assert_eq!(value["receive_spotbeam"], true);
        assert_eq!(value["transmit_spotbeam"], false);
    }

    #[test]
    fn log_control_classifies_all_eight_types() {
        let expected = [
            (0x10u8, "log-on-request", "aes-to-ges"),
            (0x11, "log-on-confirm", "ges-to-aes"),
            (0x12, "log-off-request", "aes-to-ges"),
            (0x13, "log-on-reject", "ges-to-aes"),
            (0x14, "log-on-interrogation", "ges-to-aes"),
            (0x15, "log-on-log-off-acknowledge", "either"),
            (0x16, "log-on-prompt", "ges-to-aes"),
            (0x17, "data-channel-reassignment", "ges-to-aes"),
        ];
        for (type_byte, event, direction) in expected {
            let su = su_with_crc(with_addressing(type_byte, [0xAB, 0xCD, 0xEF], 0x2A));
            let value = parse_p_su(&su).expect("log-control parses");
            assert_eq!(value["su_type"], "log-control");
            assert_eq!(value["su_type_hex"], format!("0x{type_byte:02X}"));
            assert_eq!(value["event"], event);
            assert_eq!(value["direction"], direction);
            assert_eq!(value["aes_id"], "ABCDEF");
            assert_eq!(value["ges_id"], 0x2A);
        }
    }

    #[test]
    fn call_announcement_parses_channel_pair() {
        let mut su10 = with_addressing(0x21, [0xAB, 0xCD, 0xEF], 0x44);
        su10[6] = 0x80 | (4000u16 >> 8) as u8;
        su10[7] = (4000u16 & 0xFF) as u8;
        su10[8] = (2000u16 >> 8) as u8;
        su10[9] = (2000u16 & 0xFF) as u8;
        let value = parse_p_su(&su_with_crc(su10)).expect("parses");
        assert_eq!(value["su_type"], "call-announcement");
        assert_eq!(value["receive_mhz"], 4000.0 * 0.0025 + 1510.0);
        assert_eq!(value["transmit_mhz"], 2000.0 * 0.0025 + 1611.5);
        assert!(value.get("service").is_none());
    }

    #[test]
    fn t_channel_assignment_named_with_addressing() {
        let su = su_with_crc(with_addressing(0x51, [0x12, 0x34, 0x56], 0x07));
        let value = parse_p_su(&su).expect("parses");
        assert_eq!(value["su_type"], "t-channel-assignment");
        assert_eq!(value["aes_id"], "123456");
        assert_eq!(value["ges_id"], 0x07);
    }

    #[test]
    fn satellite_id_decodes_jaero_layout() {
        let mut su10 = vec![0u8; 10];
        su10[0] = 0x0C;
        su10[2] = 0x29;
        su10[3] = 0x40;
        su10[5] = 200;
        su10[6] = 0x01;
        su10[7] = 0x23;
        su10[8] = 0x84;
        su10[9] = 0x56;
        let value = parse_p_su(&su_with_crc(su10)).expect("parses");
        assert_eq!(value["seq"], 10);
        assert_eq!(value["satellite_id"], 20);
        assert_eq!(value["longitude_deg"], 60.0);
        assert_eq!(value["longitude_dir"], "W");
        assert_eq!(value["psmc1_mhz"], f64::from(0x0123) * 0.0025 + 1510.0);
        assert_eq!(value["psmc1_spotbeam"], false);
        assert_eq!(value["psmc2_mhz"], f64::from(0x0456) * 0.0025 + 1510.0);
        assert_eq!(value["psmc2_spotbeam"], true);
        let mut su10 = vec![0u8; 10];
        su10[0] = 0x0C;
        su10[2] = 40;
        su10[3] = 0x50;
        su10[5] = 100;
        su10[6] = 0x02;
        let value = parse_p_su(&su_with_crc(su10)).expect("parses");
        assert_eq!(value["satellite_id"], 5);
        assert_eq!(value["longitude_deg"], 150.0);
        assert_eq!(value["longitude_dir"], "E");
        assert!(value.get("psmc2_mhz").is_none());
    }

    #[test]
    fn smc_channels_decodes_jaero_layout() {
        let base = |channel: u32| f64::from(channel) * 0.0025 + 1510.0;
        let mut su10 = vec![0u8; 10];
        su10[0] = 0x05;
        su10[2] = 7 << 2;
        su10[3] = 0x2A;
        su10[4] = 0x01;
        su10[6] = 0x02;
        su10[8] = 0x03;
        let value = parse_p_su(&su_with_crc(su10)).expect("parses");
        assert_eq!(value["seq"], 7);
        assert_eq!(value["lsu"], 0);
        assert_eq!(value["ges_id"], 0x2A);
        assert_eq!(value["channels"][0]["name"], "psmc_rx");
        assert_eq!(value["channels"][0]["mhz"], base(0x0100));
        assert_eq!(value["channels"][1]["name"], "rsmc0_tx");
        assert_eq!(value["channels"][1]["mhz"], base(0x0200) + 101.5);
        assert_eq!(value["channels"][2]["mhz"], base(0x0300) + 101.5);
        let mut su10 = vec![0u8; 10];
        su10[0] = 0x05;
        su10[2] = (3 << 2) | 2;
        su10[4] = 0x04;
        let value = parse_p_su(&su_with_crc(su10)).expect("parses");
        assert_eq!(value["channels"][0]["name"], "rsmc2_tx");
        assert_eq!(value["channels"][0]["mhz"], base(0x0400) + 101.5);
        assert_eq!(value["channels"][2]["name"], "rsmc4_tx");
    }

    #[test]
    fn pr_control_isu_decodes_jaero_layout() {
        let mut su10 = vec![0u8; 10];
        su10[0] = 0x40;
        su10[4] = 0x2A;
        su10[7] = 0x10;
        su10[8] = 0x01;
        su10[9] = 0x23;
        let value = parse_p_su(&su_with_crc(su10)).expect("parses");
        assert_eq!(value["ges_id"], 0x2A);
        assert_eq!(value["bit_rate"], 1200);
        assert_eq!(value["pd_mhz"], f64::from(0x0123) * 0.0025 + 1510.0);
        assert_eq!(value["spotbeam"], false);
        let mut su10 = vec![0u8; 10];
        su10[0] = 0x40;
        su10[7] = 0x80;
        assert!(
            parse_p_su(&su_with_crc(su10))
                .expect("parses")
                .get("bit_rate")
                .is_none()
        );
        let table: Vec<Option<u32>> = (0..=10).map(control_isu_bitrate).collect();
        assert_eq!(
            table,
            [
                Some(600),
                Some(1200),
                Some(2400),
                Some(4800),
                Some(6000),
                Some(5250),
                Some(10500),
                Some(8400),
                None,
                Some(21000),
                None
            ]
        );
    }

    #[test]
    fn isu_chain_reassembles() {
        let payload: Vec<u8> = (0..27).map(|index| index as u8 + 0x40).collect();
        let units = build_isu_chain(0xA1B2C3, 0x44, 2, 5, &payload);
        assert_eq!(units.len(), 5);
        let mut reassembler = Reassembler::default();
        let mut out = None;
        for unit in &units {
            assert!(su_crc_ok(unit));
            out = reassembler.push(unit);
        }
        let user = out.expect("reassembly completes");
        assert_eq!(user.aes_id, "A1B2C3");
        assert_eq!(user.ges_id, 0x44);
        assert_eq!(user.data, payload);
    }

    #[test]
    fn acars_user_data_parses() {
        let mut data = vec![0xFF, 0xFF];
        data.extend(acars_block::build(
            '2',
            "VT-ANB",
            None,
            "B6",
            '4',
            Some("M11A"),
            Some("AI0142"),
            "/BOMASAI.ADS.VT-ANB072501A070A988CA73248F0E5DC10200000F5EE1ABC000102B885E0A19F5",
            false,
        ));
        let block = parse_acars(&data).expect("ACARS parses");
        assert!(block.crc_ok);
        assert_eq!(block.core.label, "B6");
    }

    #[test]
    fn bad_crc_su_rejected() {
        let mut su = fill_su();
        su[3] ^= 1;
        assert!(!su_crc_ok(&su));
        assert!(su_crc_ok(&fill_su()));
        assert!(su_crc_ok(&[0u8; 12]));
    }
}
