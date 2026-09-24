use sdrmm_dsp::crc16_x25;
use serde::Serialize;

const FLAG: u8 = 0x7E;
const MIN_FRAME_OCTETS: usize = 4 + 4 + 1 + 2;
const MAX_FRAME_OCTETS: usize = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressType {
    Aircraft,
    GroundIcao,
    GroundDelegated,
    AllStations,
    Reserved,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AvlcAddress {
    pub kind: AddressType,
    pub addr: String,
    pub status_bit: bool,
}

fn parse_address(octets: &[u8]) -> AvlcAddress {
    let mut da = [0u8; 28];
    let groups = [(0usize, 22u32, 6u32), (1, 15, 7), (2, 8, 7), (3, 1, 7)];
    for &(oct, base, count) in &groups {
        let b = octets[oct];
        for k in 0..count {
            da[(base + k) as usize] = (b >> (7 - k)) & 1;
        }
    }
    let status_bit = (octets[0] >> 1) & 1 == 1;
    let type_bits = (da[27] << 2) | (da[26] << 1) | da[25];
    let kind = match type_bits {
        0b001 => AddressType::Aircraft,
        0b100 => AddressType::GroundIcao,
        0b101 => AddressType::GroundDelegated,
        0b111 => AddressType::AllStations,
        _ => AddressType::Reserved,
    };
    let mut addr: u32 = 0;
    for k in (1..=24).rev() {
        addr = (addr << 1) | da[k] as u32;
    }
    AvlcAddress {
        kind,
        addr: format!("{addr:06X}"),
        status_bit,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Control {
    Info {
        ns: u8,
        nr: u8,
        poll: bool,
    },
    Supervisory {
        kind: &'static str,
        nr: u8,
        poll: bool,
    },
    Unnumbered {
        kind: &'static str,
        poll: bool,
    },
}

fn parse_control(c: u8) -> Control {
    if c & 1 == 0 {
        Control::Info {
            ns: (c >> 1) & 7,
            poll: (c >> 4) & 1 == 1,
            nr: (c >> 5) & 7,
        }
    } else if c & 3 == 1 {
        let kind = match (c >> 2) & 3 {
            0 => "RR",
            1 => "RNR",
            2 => "REJ",
            _ => "SREJ",
        };
        Control::Supervisory {
            kind,
            poll: (c >> 4) & 1 == 1,
            nr: (c >> 5) & 7,
        }
    } else {
        let m = c & 0xEF;
        let kind = match m {
            0x03 => "UI",
            0x0F => "DM",
            0x43 => "DISC",
            0x63 => "UA",
            0x6F => "SABME",
            0x87 => "FRMR",
            0xAF => "XID",
            0xE3 => "TEST",
            _ => "U?",
        };
        Control::Unnumbered {
            kind,
            poll: (c >> 4) & 1 == 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "payload", rename_all = "snake_case")]
pub enum Payload {
    Acars,
    Atn { ipi: u8 },
    Xid,
    Empty,
    Other { first: u8 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct AvlcFrame {
    pub dst: AvlcAddress,
    pub src: AvlcAddress,
    pub control: Control,
    pub payload: Payload,
    pub info: Vec<u8>,
    pub raw: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FrmrInfo {
    pub rejected_control: u8,
    pub rejected: Control,
    pub vs: u8,
    pub vr: u8,
    pub rejected_was_response: bool,
    pub w_invalid_control: bool,
    pub x_info_not_allowed: bool,
    pub y_info_too_long: bool,
    pub z_invalid_nr: bool,
}

pub fn parse_frmr(info: &[u8]) -> Option<FrmrInfo> {
    if info.len() != 3 {
        return None;
    }
    let rejected_control = info[0];
    let vs = (info[1] >> 1) & 0x07;
    let rejected_was_response = (info[1] >> 4) & 1 == 1;
    let vr = (info[1] >> 5) & 0x07;
    let w_invalid_control = info[2] & 1 == 1;
    let x_info_not_allowed = (info[2] >> 1) & 1 == 1;
    let y_info_too_long = (info[2] >> 2) & 1 == 1;
    let z_invalid_nr = (info[2] >> 3) & 1 == 1;
    Some(FrmrInfo {
        rejected_control,
        rejected: parse_control(rejected_control),
        vs,
        vr,
        rejected_was_response,
        w_invalid_control,
        x_info_not_allowed,
        y_info_too_long,
        z_invalid_nr,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct XidParam {
    pub group: u8,
    pub id: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'static str>,
    pub value_hex: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_int: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freq_mhz: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub freq_support: Vec<FreqSupportEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FreqSupportEntry {
    pub gs_addr: String,
    pub freq_mhz: f64,
}

pub const XID_GID_PUBLIC: u8 = 0x80;
pub const XID_GID_PRIVATE: u8 = 0xF0;

fn vdl_param_name(id: u8) -> Option<&'static str> {
    Some(match id {
        0x00 => "parameter-set-id",
        0x01 => "connection-management",
        0x02 => "signal-quality",
        0x03 => "xid-sequencing",
        0x04 => "avlc-specific-options",
        0x05 => "expedited-sn-connection",
        0x06 => "lcr-cause",
        0x40 => "autotune-frequency",
        0x41 => "replacement-ground-stations",
        0x42 => "timer-t4",
        0x43 => "mac-persistence",
        0x44 => "counter-m1",
        0x45 => "timer-tm2",
        0x46 => "timer-tg5",
        0x47 => "timer-t3min",
        0x48 => "ground-station-address-filter",
        0x49 => "broadcast-connection",
        0x81 => "modulation-support",
        0x82 => "alternate-ground-stations",
        0x83 => "destination-airport",
        0x84 => "aircraft-location",
        0xC0 => "frequency-support-list",
        0xC1 => "airport-coverage",
        0xC3 => "nearest-airport-id",
        0xC4 => "atn-router-nets",
        0xC5 => "system-mask",
        0xC6 => "timer-tg3",
        0xC7 => "timer-tg4",
        0xC8 => "ground-station-location",
        _ => return None,
    })
}

fn pub_param_name(id: u8) -> Option<&'static str> {
    Some(match id {
        0x01 => "parameter-set-id",
        0x02 => "procedure-classes",
        0x03 => "hdlc-options",
        0x05 => "n1-downlink",
        0x06 => "n1-uplink",
        0x07 => "k-downlink",
        0x08 => "k-uplink",
        0x09 => "timer-t1-downlink",
        0x0A => "counter-n2",
        0x0B => "timer-t2",
        _ => return None,
    })
}

fn decode_vdl2_freq(buf: &[u8]) -> Option<(f64, u8)> {
    if buf.len() < 2 {
        return None;
    }
    let modulations = buf[0] >> 4;
    let raw = (u16::from_be_bytes([buf[0], buf[1]]) & 0x0FFF) as u32;
    let mut freq_khz = (raw + 10_000) * 10;
    if !freq_khz.is_multiple_of(25) {
        freq_khz += 25 - freq_khz % 25;
    }
    Some((freq_khz as f64 / 1000.0, modulations))
}

pub fn parse_xid(info: &[u8]) -> Option<Vec<XidParam>> {
    if info.is_empty() {
        return None;
    }
    let mut params = Vec::new();
    let mut pos = 1;
    while pos + 3 <= info.len() {
        let group = info[pos];
        let glen = u16::from_be_bytes([info[pos + 1], info[pos + 2]]) as usize;
        pos += 3;
        let end = (pos + glen).min(info.len());
        while pos + 2 <= end {
            let id = info[pos];
            let plen = info[pos + 1] as usize;
            pos += 2;
            if pos + plen > end {
                return None;
            }
            let value = &info[pos..pos + plen];
            pos += plen;
            let name = match group {
                XID_GID_PRIVATE => vdl_param_name(id),
                XID_GID_PUBLIC => pub_param_name(id),
                _ => None,
            };
            let printable = value.len() >= 2 && value.iter().all(|&b| (0x20..0x7F).contains(&b));
            let is_addr_list = group == XID_GID_PRIVATE
                && matches!(id, 0x41 | 0x48 | 0x82 | 0xC5)
                && !value.is_empty()
                && value.len().is_multiple_of(4);
            let text = if is_addr_list {
                Some(
                    value
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|c| parse_address(c).addr)
                        .collect::<Vec<_>>()
                        .join(","),
                )
            } else {
                printable.then(|| String::from_utf8_lossy(value).into_owned())
            };
            let freq_mhz = if group == XID_GID_PRIVATE && id == 0x40 {
                decode_vdl2_freq(value).map(|(mhz, _)| mhz)
            } else {
                None
            };
            let freq_support = if group == XID_GID_PRIVATE
                && id == 0xC0
                && !value.is_empty()
                && value.len().is_multiple_of(6)
            {
                value
                    .as_chunks::<6>()
                    .0
                    .iter()
                    .filter_map(|c| {
                        let (mhz, _) = decode_vdl2_freq(&c[0..2])?;
                        Some(FreqSupportEntry {
                            gs_addr: parse_address(&c[2..6]).addr,
                            freq_mhz: mhz,
                        })
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let value_int = match name {
                Some(n)
                    if (n.starts_with("timer-") || n.starts_with("counter-"))
                        && (1..=4).contains(&value.len()) =>
                {
                    let mut v: u32 = 0;
                    for &b in value {
                        v = (v << 8) | b as u32;
                    }
                    Some(v)
                }
                _ => None,
            };
            params.push(XidParam {
                group,
                id,
                name,
                value_hex: value.iter().map(|b| format!("{b:02x}")).collect(),
                text,
                value_int,
                freq_mhz,
                freq_support,
            });
        }
        pos = end;
    }
    if params.is_empty() {
        None
    } else {
        Some(params)
    }
}

pub fn scan(bits: &[u8]) -> Vec<AvlcFrame> {
    let mut frames = Vec::new();
    let mut shift: u8 = 0;
    let mut collecting = false;
    let mut ones = 0u32;
    let mut buf: Vec<u8> = Vec::new();

    let close = |buf: &[u8], frames: &mut Vec<AvlcFrame>| {
        if buf.len() < MIN_FRAME_OCTETS * 8 || !buf.len().is_multiple_of(8) {
            return;
        }
        let octets: Vec<u8> = buf
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| c.iter().enumerate().fold(0u8, |b, (i, &v)| b | (v << i)))
            .collect();
        if octets.len() > MAX_FRAME_OCTETS {
            return;
        }
        let n = octets.len();
        let fcs = crc16_x25(&octets[..n - 2]);
        let le = u16::from_le_bytes([octets[n - 2], octets[n - 1]]);
        if fcs != le {
            return;
        }
        let dst = parse_address(&octets[0..4]);
        let src = parse_address(&octets[4..8]);
        let control = parse_control(octets[8]);
        let info = octets[9..n - 2].to_vec();
        let payload = match (&control, info.first()) {
            (Control::Unnumbered { kind: "XID", .. }, _) => Payload::Xid,
            (_, Some(0xFF)) => Payload::Acars,
            (_, Some(&ipi @ (0x81..=0x83))) => Payload::Atn { ipi },
            (_, Some(&first)) => Payload::Other { first },
            (_, None) => Payload::Empty,
        };
        frames.push(AvlcFrame {
            dst,
            src,
            control,
            payload,
            info,
            raw: octets,
        });
    };

    for &bit in bits {
        shift = (shift >> 1) | (bit << 7);
        if !collecting {
            if shift == FLAG {
                collecting = true;
                buf.clear();
                ones = 0;
            }
            continue;
        }
        if bit == 1 {
            ones += 1;
            if ones > 6 {
                collecting = false;
                continue;
            }
            buf.push(1);
        } else if ones == 5 {
            ones = 0;
        } else if ones == 6 {
            let len = buf.len().saturating_sub(7);
            close(&buf[..len], &mut frames);
            buf.clear();
            ones = 0;
        } else {
            buf.push(0);
            ones = 0;
            if buf.len() > MAX_FRAME_OCTETS * 8 {
                collecting = false;
            }
        }
    }
    frames
}

#[cfg(test)]
pub fn build(frames: &[Vec<u8>]) -> Vec<u8> {
    let flag = [0u8, 1, 1, 1, 1, 1, 1, 0];
    let mut bits: Vec<u8> = Vec::new();
    bits.extend(flag);
    for frame in frames {
        let mut octets = frame.clone();
        let fcs = crc16_x25(&octets);
        octets.extend(fcs.to_le_bytes());
        let mut ones = 0;
        for &o in &octets {
            for i in 0..8 {
                let b = (o >> i) & 1;
                bits.push(b);
                if b == 1 {
                    ones += 1;
                    if ones == 5 {
                        bits.push(0);
                        ones = 0;
                    }
                } else {
                    ones = 0;
                }
            }
        }
        bits.extend(flag);
    }
    bits
}

#[cfg(test)]
pub fn encode_address(kind: AddressType, specific: u32, status_bit: bool, last: bool) -> [u8; 4] {
    let type_bits: u8 = match kind {
        AddressType::Aircraft => 0b001,
        AddressType::GroundIcao => 0b100,
        AddressType::GroundDelegated => 0b101,
        AddressType::AllStations => 0b111,
        AddressType::Reserved => 0b000,
    };
    let mut da = [0u8; 28];
    for (k, bit) in da.iter_mut().enumerate().skip(1).take(24) {
        *bit = ((specific >> (k - 1)) & 1) as u8;
    }
    da[25] = type_bits & 1;
    da[26] = (type_bits >> 1) & 1;
    da[27] = (type_bits >> 2) & 1;
    let mut out = [0u8; 4];
    let groups = [(0usize, 22u32, 6u32), (1, 15, 7), (2, 8, 7), (3, 1, 7)];
    for &(oct, base, count) in &groups {
        let mut b = 0u8;
        for k in 0..count {
            b |= da[(base + k) as usize] << (7 - k);
        }
        out[oct] = b;
    }
    if status_bit {
        out[0] |= 0b10;
    }
    if last {
        out[3] |= 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_frame() -> Vec<u8> {
        let mut f = Vec::new();
        f.extend(encode_address(
            AddressType::Aircraft,
            0xA6F123,
            false,
            false,
        ));
        f.extend(encode_address(
            AddressType::GroundIcao,
            0x2C0A55,
            true,
            true,
        ));
        f.push(0x03);
        f.push(0xFF);
        f.extend(b"PAYLOAD");
        f
    }

    #[test]
    fn roundtrip_frame() {
        let bits = build(&[test_frame()]);
        let frames = scan(&bits);
        assert_eq!(frames.len(), 1);
        let f = &frames[0];
        assert_eq!(f.dst.kind, AddressType::Aircraft);
        assert_eq!(f.dst.addr, "A6F123");
        assert!(!f.dst.status_bit);
        assert_eq!(f.src.kind, AddressType::GroundIcao);
        assert_eq!(f.src.addr, "2C0A55");
        assert!(f.src.status_bit);
        assert_eq!(
            f.control,
            Control::Unnumbered {
                kind: "UI",
                poll: false
            }
        );
        assert_eq!(f.payload, Payload::Acars);
        assert_eq!(&f.info[1..], b"PAYLOAD");
    }

    #[test]
    fn two_frames_one_stream() {
        let mut f2 = test_frame();
        f2[8] = 0x01;
        let bits = build(&[test_frame(), f2]);
        let frames = scan(&bits);
        assert_eq!(frames.len(), 2);
        assert!(matches!(
            frames[1].control,
            Control::Supervisory { kind: "RR", .. }
        ));
    }

    #[test]
    fn bad_fcs_rejected() {
        let mut bits = build(&[test_frame()]);
        bits[8 * 12 + 2] ^= 1;
        assert!(scan(&bits).is_empty());
    }

    #[test]
    fn sabme_u_command_recognized() {
        assert_eq!(
            parse_control(0x6F),
            Control::Unnumbered {
                kind: "SABME",
                poll: false
            }
        );
        assert_eq!(
            parse_control(0x7F),
            Control::Unnumbered {
                kind: "SABME",
                poll: true
            }
        );
    }

    #[test]
    fn byte_swapped_fcs_now_rejected() {
        let mut octets = test_frame();
        let fcs = crc16_x25(&octets);
        let [lo, hi] = fcs.to_le_bytes();
        if lo == hi {
            return;
        }
        octets.push(hi);
        octets.push(lo);
        let flag = [0u8, 1, 1, 1, 1, 1, 1, 0];
        let mut bits: Vec<u8> = Vec::new();
        bits.extend(flag);
        let mut ones = 0;
        for &o in &octets {
            for i in 0..8 {
                let b = (o >> i) & 1;
                bits.push(b);
                if b == 1 {
                    ones += 1;
                    if ones == 5 {
                        bits.push(0);
                        ones = 0;
                    }
                } else {
                    ones = 0;
                }
            }
        }
        bits.extend(flag);
        assert!(
            scan(&bits).is_empty(),
            "byte-swapped FCS must not be accepted"
        );
    }
}

#[cfg(test)]
mod body_tests {
    use super::*;

    #[test]
    fn off_air_s_frame_parses_as_rr() {
        let octets = [0x14u8, 0x22, 0xcc, 0x54, 0xb2, 0x0c, 0x42, 0xb5, 0xa1];
        let bits = build(&[octets.to_vec()]);
        let frames = scan(&bits);
        assert_eq!(frames.len(), 1);
        let f = &frames[0];
        assert_eq!(
            f.control,
            Control::Supervisory {
                kind: "RR",
                nr: 5,
                poll: false
            }
        );
        assert_eq!(f.payload, Payload::Empty);
        assert!(f.info.is_empty());
    }

    #[test]
    fn xid_parameters_decode_with_names_and_text() {
        let info = [
            0x82, 0xF0, 0x00, 0x09, 0x00, 0x01, b'V', 0x83, 0x04, b'K', b'S', b'M', b'F',
        ];
        let params = parse_xid(&info).expect("params");
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, Some("parameter-set-id"));
        assert_eq!(params[1].name, Some("destination-airport"));
        assert_eq!(params[1].text.as_deref(), Some("KSMF"));
        assert_eq!(params[1].value_hex, "4b534d46");
    }

    #[test]
    fn malformed_xid_returns_none() {
        let info = [0x82, 0xF0, 0x00, 0x04, 0x42, 0x40];
        assert!(parse_xid(&info).is_none());
    }
}

#[cfg(test)]
mod frmr_tests {
    use super::*;

    #[test]
    fn frmr_info_field_expands() {
        let info = [0x64, 0xB8, 0x08];
        let frmr = parse_frmr(&info).expect("frmr decodes");
        assert_eq!(frmr.rejected_control, 0x64);
        assert_eq!(
            frmr.rejected,
            Control::Info {
                ns: 2,
                nr: 3,
                poll: false
            }
        );
        assert_eq!(frmr.vs, 4);
        assert_eq!(frmr.vr, 5);
        assert!(frmr.rejected_was_response);
        assert!(!frmr.w_invalid_control);
        assert!(!frmr.x_info_not_allowed);
        assert!(!frmr.y_info_too_long);
        assert!(frmr.z_invalid_nr);
    }

    #[test]
    fn frmr_w_and_y_flags() {
        let frmr = parse_frmr(&[0x6F, 0x00, 0x05]).expect("frmr decodes");
        assert_eq!(
            frmr.rejected,
            Control::Unnumbered {
                kind: "SABME",
                poll: false
            }
        );
        assert!(frmr.w_invalid_control);
        assert!(!frmr.x_info_not_allowed);
        assert!(frmr.y_info_too_long);
        assert!(!frmr.z_invalid_nr);
        assert_eq!(frmr.vs, 0);
        assert_eq!(frmr.vr, 0);
    }

    #[test]
    fn frmr_wrong_length_rejected() {
        assert!(parse_frmr(&[0x64, 0xB8]).is_none());
        assert!(parse_frmr(&[0x64, 0xB8, 0x08, 0x00]).is_none());
    }
}

#[cfg(test)]
mod xid_gs_tests {
    use super::*;

    #[test]
    fn ground_station_list_param_decodes_addresses() {
        let gs1 = encode_address(AddressType::GroundIcao, 0x2C0A55, false, false);
        let gs2 = encode_address(AddressType::GroundIcao, 0x2D4917, false, true);
        let mut info = vec![0x82, 0xF0, 0x00, (2 + 8) as u8, 0x41, 8];
        info.extend_from_slice(&gs1);
        info.extend_from_slice(&gs2);
        let params = parse_xid(&info).unwrap();
        assert_eq!(params[0].name, Some("replacement-ground-stations"));
        assert_eq!(params[0].text.as_deref(), Some("2C0A55,2D4917"));
    }

    #[test]
    fn gs_address_filter_and_system_mask_decode_addresses() {
        let gs = encode_address(AddressType::GroundIcao, 0x2C0A55, false, true);
        for id in [0x48u8, 0xC5] {
            let mut info = vec![0x82, 0xF0, 0x00, 6, id, 4];
            info.extend_from_slice(&gs);
            let params = parse_xid(&info).unwrap();
            assert_eq!(params[0].id, id);
            assert_eq!(params[0].text.as_deref(), Some("2C0A55"));
        }
    }
}

#[cfg(test)]
mod xid_vdl2_3_tests {
    use super::*;

    fn vdl_one(id: u8, value: &[u8]) -> Vec<u8> {
        let glen = 2 + value.len();
        let mut info = vec![
            0x82,
            0xF0,
            (glen >> 8) as u8,
            glen as u8,
            id,
            value.len() as u8,
        ];
        info.extend_from_slice(value);
        info
    }

    #[test]
    fn new_vdl_param_names() {
        let cases: &[(u8, &str)] = &[
            (0x46, "timer-tg5"),
            (0x47, "timer-t3min"),
            (0x48, "ground-station-address-filter"),
            (0x49, "broadcast-connection"),
            (0xC0, "frequency-support-list"),
            (0xC1, "airport-coverage"),
            (0xC3, "nearest-airport-id"),
            (0xC4, "atn-router-nets"),
            (0xC5, "system-mask"),
            (0xC6, "timer-tg3"),
            (0xC7, "timer-tg4"),
        ];
        for &(id, name) in cases {
            assert_eq!(vdl_param_name(id), Some(name), "id {id:#04x}");
        }
    }

    #[test]
    fn public_group_params_named() {
        let info = [0x82, 0x80, 0x00, 0x06, 0x09, 0x02, 0x00, 0x64, 0x0A, 0x00];
        let params = parse_xid(&info).unwrap();
        assert_eq!(params[0].group, XID_GID_PUBLIC);
        assert_eq!(params[0].name, Some("timer-t1-downlink"));
        assert_eq!(params[0].value_int, Some(100));
        assert_eq!(params[1].name, Some("counter-n2"));
    }

    #[test]
    fn autotune_frequency_decodes_to_mhz() {
        let params = parse_xid(&vdl_one(0x40, &[0x0E, 0x71])).unwrap();
        assert_eq!(params[0].name, Some("autotune-frequency"));
        assert_eq!(params[0].freq_mhz, Some(136.975));
    }

    #[test]
    fn frequency_support_list_decodes_entries() {
        let gs = encode_address(AddressType::GroundIcao, 0x2C0A55, false, true);
        let mut value = vec![0x0E, 0x71];
        value.extend_from_slice(&gs);
        let params = parse_xid(&vdl_one(0xC0, &value)).unwrap();
        assert_eq!(params[0].name, Some("frequency-support-list"));
        assert_eq!(params[0].freq_support.len(), 1);
        assert_eq!(params[0].freq_support[0].gs_addr, "2C0A55");
        assert_eq!(params[0].freq_support[0].freq_mhz, 136.975);
    }

    #[test]
    fn timers_decode_to_int() {
        let params = parse_xid(&vdl_one(0x46, &[0x01, 0x2C])).unwrap();
        assert_eq!(params[0].name, Some("timer-tg5"));
        assert_eq!(params[0].value_int, Some(300));
        let params = parse_xid(&vdl_one(0x44, &[0x05])).unwrap();
        assert_eq!(params[0].name, Some("counter-m1"));
        assert_eq!(params[0].value_int, Some(5));
    }

    #[test]
    fn freq_decode_known_channels() {
        assert_eq!(decode_vdl2_freq(&[0x0E, 0x70]).unwrap().0, 136.975);
        assert_eq!(decode_vdl2_freq(&[0x0E, 0x66]).unwrap().0, 136.875);
        assert_eq!(decode_vdl2_freq(&[0x0E, 0x57]).unwrap().0, 136.725);
        assert_eq!(decode_vdl2_freq(&[0x0E, 0x4F]).unwrap().0, 136.650);
        assert_eq!(decode_vdl2_freq(&[0xCE, 0x70]).unwrap(), (136.975, 0xC));
    }
}
