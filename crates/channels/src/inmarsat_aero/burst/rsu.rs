use sdrmm_dsp::crc16_x25;
use serde_json::{Value, json};

use crate::inmarsat_aero::su::AeroUserData;

pub(in crate::inmarsat_aero) const R_SU_LEN: usize = 19;
const R_PAYLOAD: usize = 11;
const USER_DATA_FLAG: u8 = 0x08;
const PENDING_MAX_AGE: u32 = 10;

pub(super) fn r_su_crc_ok(su: &[u8]) -> bool {
    su.len() == R_SU_LEN && crc16_x25(&su[..17]) == u16::from_le_bytes([su[17], su[18]])
}

fn seq_indicator(value: u8) -> Option<(u8, u8)> {
    match value {
        1 => Some((1, 1)),
        2 => Some((1, 2)),
        3 => Some((2, 2)),
        4 => Some((1, 3)),
        5 => Some((2, 3)),
        6 => Some((3, 3)),
        _ => None,
    }
}

#[cfg(test)]
fn seq_indicator_for(index: u8, total: u8) -> u8 {
    match (index, total) {
        (1, 1) => 1,
        (1, 2) => 2,
        (2, 2) => 3,
        (1, 3) => 4,
        (2, 3) => 5,
        (3, 3) => 6,
        _ => 0,
    }
}

pub(super) fn parse_r_su(su: &[u8]) -> Option<Value> {
    if su.len() < R_SU_LEN || su[1] & USER_DATA_FLAG != 0 {
        return None;
    }
    let (su_type, kind) = match su[2] {
        0x20 => ("r-access-request", "general-telephone"),
        0x23 => ("r-access-request", "abbreviated-telephone"),
        0x22 => ("r-access-request", "data"),
        0x61 => ("r-request-for-acknowledgement", ""),
        0x62 => ("r-acknowledgement", ""),
        0x12 => ("r-log-on-off-control", ""),
        0x30 => ("r-call-progress", ""),
        0x15 => ("r-log-on-off-acknowledgement", ""),
        0x17 => ("r-log-control-ready-for-reassignment", ""),
        0x60 => ("r-telephony-acknowledge", ""),
        _ => return None,
    };
    let mut value = json!({
        "su_type": su_type,
        "su_type_hex": format!("0x{:02X}", su[2]),
    });
    if !kind.is_empty() {
        value["request_kind"] = json!(kind);
    }
    Some(value)
}

struct PendingRIsu {
    aes_id: u32,
    ges_id: u8,
    qno: u8,
    refno: u8,
    parts: Vec<Option<Vec<u8>>>,
    age: u32,
}

#[derive(Default)]
pub(super) struct RIsuReassembler {
    pending: Vec<PendingRIsu>,
}

impl RIsuReassembler {
    pub(super) fn push(&mut self, su: &[u8]) -> Option<AeroUserData> {
        if su[1] & USER_DATA_FLAG == 0 {
            return None;
        }
        let (index, total) = seq_indicator(su[0] >> 4)?;
        let su_type = su[0] & 0x0F;
        if su_type == 15 || su_type == 0 {
            return None;
        }
        let qno = su[1] >> 4;
        let refno = su[1] & 0x07;
        let aes_id = u32::from_be_bytes([0, su[2], su[3], su[4]]);
        let ges_id = su[5];
        let take = if index == total {
            usize::from(su_type).min(R_PAYLOAD)
        } else {
            R_PAYLOAD
        };
        for pending in &mut self.pending {
            pending.age += 1;
        }
        self.pending.retain(|pending| pending.age < PENDING_MAX_AGE);
        let position = self
            .pending
            .iter()
            .position(|pending| {
                pending.aes_id == aes_id
                    && pending.ges_id == ges_id
                    && pending.qno == qno
                    && pending.refno == refno
            })
            .unwrap_or_else(|| {
                self.pending.push(PendingRIsu {
                    aes_id,
                    ges_id,
                    qno,
                    refno,
                    parts: vec![None; usize::from(total)],
                    age: 0,
                });
                self.pending.len() - 1
            });
        let pending = &mut self.pending[position];
        *pending.parts.get_mut(usize::from(index - 1))? = Some(su[6..6 + take].to_vec());
        pending.age = 0;
        if !pending.parts.iter().all(Option::is_some) {
            return None;
        }
        let done = self.pending.swap_remove(position);
        Some(AeroUserData {
            aes_id: format!("{:06X}", done.aes_id),
            ges_id: done.ges_id,
            qno: done.qno,
            refno: done.refno,
            data: done.parts.into_iter().flatten().flatten().collect(),
        })
    }
}

#[cfg(test)]
pub(in crate::inmarsat_aero) fn build_r_sus(
    aes_id: u32,
    ges_id: u8,
    qno: u8,
    refno: u8,
    data: &[u8],
) -> Vec<Vec<u8>> {
    let total = data.len().div_ceil(R_PAYLOAD).clamp(1, 3) as u8;
    let aes = aes_id.to_be_bytes();
    (1..=total)
        .map(|index| {
            let offset = usize::from(index - 1) * R_PAYLOAD;
            let chunk = &data[offset..data.len().min(offset + R_PAYLOAD)];
            let su_type = if index == total {
                chunk.len() as u8
            } else {
                R_PAYLOAD as u8
            };
            let mut su = vec![
                (seq_indicator_for(index, total) << 4) | (su_type & 0x0F),
                (qno << 4) | USER_DATA_FLAG | (refno & 0x07),
                aes[1],
                aes[2],
                aes[3],
                ges_id,
            ];
            su.extend_from_slice(chunk);
            su.resize(17, 0);
            su.extend(crc16_x25(&su).to_le_bytes());
            su
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control(type_byte: u8, flags: u8) -> Vec<u8> {
        let mut su = vec![0u8; R_SU_LEN];
        su[1] = flags;
        su[2] = type_byte;
        let crc = crc16_x25(&su[..17]);
        su[17..].copy_from_slice(&crc.to_le_bytes());
        su
    }

    #[test]
    fn r_control_set_classifies_aerotype_r() {
        let expected = [
            (0x20u8, "r-access-request", Some("general-telephone")),
            (0x23, "r-access-request", Some("abbreviated-telephone")),
            (0x22, "r-access-request", Some("data")),
            (0x61, "r-request-for-acknowledgement", None),
            (0x62, "r-acknowledgement", None),
            (0x12, "r-log-on-off-control", None),
            (0x30, "r-call-progress", None),
            (0x15, "r-log-on-off-acknowledgement", None),
            (0x17, "r-log-control-ready-for-reassignment", None),
            (0x60, "r-telephony-acknowledge", None),
        ];
        for (type_byte, su_type, kind) in expected {
            let su = control(type_byte, 0);
            assert!(r_su_crc_ok(&su));
            let value = parse_r_su(&su).expect("classifies");
            assert_eq!(value["su_type"], su_type);
            assert_eq!(value["su_type_hex"], format!("0x{type_byte:02X}"));
            match kind {
                Some(kind) => assert_eq!(value["request_kind"], kind),
                None => assert!(value.get("request_kind").is_none()),
            }
        }
        assert!(parse_r_su(&control(0x20, USER_DATA_FLAG)).is_none());
        assert!(parse_r_su(&control(0xAA, 0)).is_none());
        assert!(parse_r_su(&[0u8; 12]).is_none());
    }

    #[test]
    fn seq_indicator_matches_jaero_switch() {
        for (indicator, index, total) in [
            (1u8, 1u8, 1u8),
            (2, 1, 2),
            (3, 2, 2),
            (4, 1, 3),
            (5, 2, 3),
            (6, 3, 3),
        ] {
            assert_eq!(seq_indicator(indicator), Some((index, total)));
            assert_eq!(seq_indicator_for(index, total), indicator);
        }
        assert!(seq_indicator(0).is_none());
        assert!((7..=15).all(|value| seq_indicator(value).is_none()));
    }

    #[test]
    fn r_units_reassemble() {
        let payload: Vec<u8> = (0..25).collect();
        let mut reassembler = RIsuReassembler::default();
        let done: Vec<AeroUserData> = build_r_sus(0x123456, 7, 2, 3, &payload)
            .iter()
            .filter_map(|su| reassembler.push(su))
            .collect();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].data, payload);
    }
}
