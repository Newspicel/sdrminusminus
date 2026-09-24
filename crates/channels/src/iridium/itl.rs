use serde_json::{Value, json};

use super::itl_tables::{PRS_HDR, PRS_LIST, PRS_PLANES};

const INV_DQPSK: [u8; 4] = [0, 3, 1, 2];
const SYMBOLS: usize = 384;

type Prs = (u128, u128);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItlFrame {
    pub version: u8,
    pub plane: Option<u8>,
    pub sat: Option<String>,
    pub msg_type: Option<String>,
    pub msg: [Option<u8>; 4],
    pub types: [Option<u8>; 4],
}

impl ItlFrame {
    pub fn to_json(&self) -> Value {
        json!({
            "type": "time-location",
            "version": self.version,
            "plane": self.plane,
            "sat": self.sat,
            "msg_type": self.msg_type,
            "msg": self.msg,
            "msg_types": self.types,
        })
    }
}

fn pack(bits: &[u8]) -> Prs {
    let n = bits.len();
    let (mut hi, mut lo) = (0u128, 0u128);
    for (i, _) in bits.iter().enumerate().filter(|(_, bit)| **bit != 0) {
        let pos = n - 1 - i;
        if pos >= 128 {
            hi |= 1u128 << (pos - 128);
        } else {
            lo |= 1u128 << pos;
        }
    }
    (hi, lo)
}

fn hamming(a: Prs, b: Prs) -> u32 {
    (a.0 ^ b.0).count_ones() + (a.1 ^ b.1).count_ones()
}

fn nearest(value: Prs, table: &[Prs], max_dist: u32) -> Option<usize> {
    let mut best = (u32::MAX, 0usize);
    for (i, &entry) in table.iter().enumerate() {
        let distance = hamming(value, entry);
        if distance < best.0 {
            best = (distance, i);
        }
    }
    (best.0 <= max_dist).then_some(best.1)
}

fn map_sat(num: u8, version: u8) -> Option<(String, String)> {
    let n = i32::from(num);
    let none = || "---".to_owned();
    match version {
        2 => Some(match n {
            77 => (none(), "M08".into()),
            0..66 => (format!("S{:02}", n % 11 + 1), format!("M{:02}", n / 11 + 1)),
            82..=84 => (format!("R{:02}", (n - 82) % 3 + 1), "N01".into()),
            85..=95 => (format!("S{:02}", n - 84), "N02".into()),
            96..=107 => (
                format!("R{:02}", (n - 96) % 3 + 1),
                format!("N{:02}", (n - 96) / 3 + 3),
            ),
            108 => (none(), "SSS".into()),
            111 => (none(), "N08".into()),
            _ => (none(), format!("{n:03}")),
        }),
        1 if n < 88 => Some((format!("S{:02}", n % 11 + 1), format!("M{:02}", n / 11 + 1))),
        _ => None,
    }
}

fn split_channels(payload: &[u8]) -> ([u8; SYMBOLS], [u8; SYMBOLS]) {
    let mut i_channel = [0u8; SYMBOLS];
    let mut q_channel = [0u8; SYMBOLS];
    let mut phase = 0u8;
    for (k, pair) in payload.chunks_exact(2).take(SYMBOLS).enumerate() {
        let mapped = (pair[1] << 1) | pair[0];
        phase = (phase + INV_DQPSK[usize::from(mapped)]) % 4;
        let (i, q) = match phase {
            0 => (0, 0),
            1 => (1, 0),
            2 => (1, 1),
            _ => (0, 1),
        };
        i_channel[k] = i;
        q_channel[k] = q;
    }
    (i_channel, q_channel)
}

pub fn decode_itl(payload: &[u8]) -> Option<ItlFrame> {
    if payload.len() < 2 * SYMBOLS {
        return None;
    }
    let (i_channel, q_channel) = split_channels(payload);
    let version = nearest(pack(&i_channel[..128]), &PRS_HDR, 40)? as u8;
    if version == 0 {
        return None;
    }
    let plane = nearest(pack(&i_channel[128..]), &PRS_PLANES, 64).map(|i| (i + 1) as u8);
    let mut msg = [None; 4];
    let mut types = [None; 4];
    for (k, code) in q_channel.chunks_exact(96).enumerate() {
        if let Some(j) = nearest(pack(code), &PRS_LIST, 24) {
            msg[k] = Some((j % 128) as u8);
            types[k] = Some((j / 128) as u8);
        }
    }
    let (sat, msg_type) = msg[0]
        .and_then(|m| map_sat(m, version))
        .map_or((None, None), |(s, t)| (Some(s), Some(t)));
    Some(ItlFrame {
        version,
        plane,
        sat,
        msg_type,
        msg,
        types,
    })
}
