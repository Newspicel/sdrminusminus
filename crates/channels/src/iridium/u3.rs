use serde_json::{Value, json};

use super::rs::{bytes_of_bits, checksum_16, first_39, rs6_correct, rs8_correct, symbols6};
use crate::datalink::hex;

pub fn parse_u3(payload_bits: &[u8]) -> Value {
    if payload_bits.len() < 312 {
        return json!({ "u3_type": "IU3" });
    }
    let p = &payload_bits[..312];
    let (bytes, _) = bytes_of_bits(p);
    if let Some(rs8m) = first_39(&bytes).and_then(|sent| rs8_correct(&sent)) {
        let cs_ok = checksum_16(&rs8m) == 0;
        let data: &[u8] = if cs_ok {
            let end = rs8m[..28]
                .iter()
                .rposition(|&x| x != 0)
                .map_or(0, |i| i + 1);
            &rs8m[..end]
        } else {
            &rs8m[..]
        };
        return json!({
            "u3_type": "I38",
            "cs_ok": cs_ok,
            "odd_byte": rs8m[28],
            "data_hex": hex(data),
        });
    }
    let mut cw6 = symbols6(p);
    if let Some(fixed) = rs6_correct(&mut cw6) {
        let rs6m = &cw6[..42];
        let subformat = rs6m[0];
        let v: Vec<u8> = rs6m[1..]
            .iter()
            .flat_map(|&s| (0..6).rev().map(move |k| (s >> k) & 1))
            .collect();
        let mut out = json!({
            "u3_type": "I36",
            "subformat": subformat,
            "rs_corrected": fixed,
        });
        let group = |bits: &[u8], n: usize| -> Vec<u64> {
            bits.chunks(n)
                .filter(|c| c.len() == n)
                .map(|c| c.iter().fold(0u64, |a, &b| (a << 1) | b as u64))
                .collect()
        };
        match subformat {
            6 => {
                let mut nums = group(&v[2..], 24);
                while nums.last() == Some(&0) {
                    nums.pop();
                }
                out["numbers"] = json!(nums);
            }
            32 | 34 => {
                let body = &v[2..v.len().saturating_sub(4)];
                let mut nums = group(body, 24);
                let tail = group(&v[v.len().saturating_sub(4)..], 4);
                if let Some(&t) = tail.first()
                    && t != 0
                {
                    nums.push(t);
                }
                while nums.last() == Some(&0x7ffff) {
                    nums.pop();
                }
                out["numbers"] = json!(nums);
            }
            _ => {
                out["data_hex"] = json!(hex(rs6m));
            }
        }
        return out;
    }

    json!({ "u3_type": "IU3" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_payload_is_iu3() {
        assert_eq!(parse_u3(&[0u8; 100])["u3_type"], "IU3");
    }
}
