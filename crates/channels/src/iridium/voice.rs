use serde_json::{Value, json};

use super::iip::parse_iip_frame;
use super::rs::{bytes_of_bits, first_39, iip_crc24, rs6_correct, rs8_correct, symbols6};
use crate::datalink::hex;

pub fn classify_voice(payload_bits: &[u8]) -> Option<Value> {
    if payload_bits.len() < 312 {
        return None;
    }
    let (straight, reversed) = bytes_of_bits(payload_bits);
    if iip_crc24(&reversed) == 0 {
        let mut frame = parse_iip_frame(&reversed);
        frame["voice_type"] = json!("VDA");
        return Some(frame);
    }
    let mut symbols = symbols6(&payload_bits[..312]);
    if let Some(fixed) = rs6_correct(&mut symbols) {
        let bits: String = symbols[..42].iter().map(|s| format!("{s:06b}")).collect();
        return Some(json!({
            "voice_type": "VO6",
            "data_bits": bits,
            "rs_corrected": fixed,
        }));
    }
    if let Some(data) = first_39(&straight).and_then(|sent| rs8_correct(&sent)) {
        return Some(json!({
            "voice_type": "VOD",
            "data_hex": hex(&data),
        }));
    }
    Some(zero_padded(&straight).unwrap_or_else(|| {
        json!({
            "voice_type": "VOC",
            "ambe_hex": hex(&straight),
        })
    }))
}

fn zero_padded(bytes: &[u8]) -> Option<Value> {
    let n = bytes.len();
    let padded = n >= 4 && bytes[n - 4..n - 1].iter().all(|&x| x == 0);
    let balanced = bytes.iter().fold(0u8, |a, &x| a.wrapping_add(x)) == 0;
    if !padded || !balanced {
        return None;
    }
    let end = bytes[..n - 1]
        .iter()
        .rposition(|&x| x != 0)
        .map_or(1, |i| i + 1);
    Some(json!({
        "voice_type": "VOZ",
        "data_hex": hex(&bytes[..end]),
    }))
}

#[cfg(test)]
mod tests {
    use super::super::encode::bits_of_hex;
    use super::super::rs::RS6_N;
    use super::*;

    const VDA: &str =
        "a5b25318a40cddb8b6c8347b6bc4de749b78fc4ef8d3988ee822296b923cb93a2c067d8c904549";
    const VO6: &str =
        "2076bfda8efaba67d77ca9bfaf99093f556b4fed45268aecffa20b8bc2079f93f5a9dbd45d79e1";
    const VOD: &str =
        "91c5b10becb5563bfc1e6f93427ecbc8fe2955e5cd8e46dc8ed4b7c2764d2ac749977139180ede";
    const VOD_ERR: &str =
        "91c5b10becef563bfc1e6f93427ecbc8fe2955e5cd8e46dc8ed4b7c2764d2ac749977139180ede";
    const VOD_MSG: &str = "91c5b10becb5563bfc1e6f93427ecbc8fe2955e5cd8e46dc8ed4b7c2764d2a";
    const VOZ: &str =
        "5a4d767706f85d8690024ad6bda3401be9c8cbccc935f6cd1f61226ae15338ae1a34a100000000";
    const VOC: &str =
        "004d33ba0d246ac04c81b1baf23e3bf9eef5f79f2b4934af87f5520b69b94b0d982e85bb55b672";

    #[test]
    fn ladder_classifies_each_stage() {
        for (hexstr, want) in [
            (VDA, "VDA"),
            (VO6, "VO6"),
            (VOD, "VOD"),
            (VOZ, "VOZ"),
            (VOC, "VOC"),
        ] {
            let v = classify_voice(&bits_of_hex(hexstr)).expect("voice");
            assert_eq!(v["voice_type"], want, "payload {hexstr}");
        }
    }

    #[test]
    fn vod_corrects_single_byte_error() {
        let v = classify_voice(&bits_of_hex(VOD_ERR)).expect("voice");
        assert_eq!(v["voice_type"], "VOD");
        assert_eq!(v["data_hex"], VOD_MSG);
    }

    #[test]
    fn vo6_recovers_oracle_message() {
        let msg6: [u8; 42] = [
            8, 7, 26, 63, 54, 40, 59, 58, 46, 38, 31, 23, 31, 10, 38, 63, 43, 57, 36, 9, 15, 53,
            21, 43, 19, 62, 53, 5, 9, 40, 43, 44, 63, 58, 8, 11, 34, 60, 8, 7, 39, 57,
        ];
        let bits = bits_of_hex(VO6);
        let mut cw6: [u8; RS6_N] = symbols6(&bits[..312]);
        assert_eq!(rs6_correct(&mut cw6), Some(0));
        assert_eq!(&cw6[..42], &msg6);
        let mut damaged = cw6;
        damaged[7] ^= 0x15;
        assert_eq!(rs6_correct(&mut damaged), Some(1));
        assert_eq!(&damaged[..42], &msg6);
    }

    #[test]
    fn short_payload_rejected() {
        assert!(classify_voice(&[0u8; 200]).is_none());
    }
}
