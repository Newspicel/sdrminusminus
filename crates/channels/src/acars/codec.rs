use flate2::{Decompress, FlushDecompress, Status};

const BASE64_PAD: u8 = b'=';
const INFLATE_CHUNK: usize = 4096;

fn base64_sextet(c: u8) -> Option<u32> {
    let v = match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    };
    Some(u32::from(v))
}

pub fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    let (quads, []) = input.as_chunks::<4>() else {
        return None;
    };
    let mut out = Vec::with_capacity(quads.len() * 3);
    let last = quads.len().checked_sub(1);
    for (index, quad) in quads.iter().enumerate() {
        let pads = quad.iter().rev().take_while(|&&c| c == BASE64_PAD).count();
        if pads > 2 || (pads > 0 && Some(index) != last) {
            return None;
        }
        let mut word = 0u32;
        for &c in &quad[..4 - pads] {
            word = (word << 6) | base64_sextet(c)?;
        }
        word <<= 6 * pads;
        let bytes = word.to_be_bytes();
        let kept = 3 - pads;
        let dropped_bits = word & (0x00FF_FFFF >> (8 * kept));
        if dropped_bits != 0 {
            return None;
        }
        out.extend_from_slice(&bytes[1..=kept]);
    }
    Some(out)
}

pub fn inflate_raw(input: &[u8]) -> Option<Vec<u8>> {
    inflate(input, false)
}

pub fn inflate_zlib(input: &[u8]) -> Option<Vec<u8>> {
    inflate(input, true)
}

fn inflate(input: &[u8], zlib: bool) -> Option<Vec<u8>> {
    let mut state = Decompress::new(zlib);
    let mut out = Vec::with_capacity(input.len().saturating_mul(2).max(INFLATE_CHUNK));
    loop {
        let consumed = usize::try_from(state.total_in()).ok()?;
        let before = (state.total_in(), state.total_out());
        if out.len() == out.capacity() {
            out.reserve(INFLATE_CHUNK.max(out.len()));
        }
        let status = state
            .decompress_vec(input.get(consumed..)?, &mut out, FlushDecompress::None)
            .ok()?;
        match status {
            Status::StreamEnd => return Some(out),
            Status::Ok | Status::BufError => {
                let stalled = before == (state.total_in(), state.total_out());
                if stalled && out.len() < out.capacity() {
                    return None;
                }
            }
        }
    }
}

#[cfg(test)]
pub mod testing {
    use std::io::Write;

    use flate2::{
        Compression,
        write::{DeflateEncoder, ZlibEncoder},
    };

    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn deflate_raw(data: &[u8]) -> Vec<u8> {
        let mut enc = DeflateEncoder::new(Vec::new(), Compression::new(6));
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    pub fn zlib(data: &[u8]) -> Vec<u8> {
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::new(6));
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    pub fn base64_encode(data: &[u8]) -> String {
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let mut word = [0u8; 3];
            word[..chunk.len()].copy_from_slice(chunk);
            let bits = u32::from_be_bytes([0, word[0], word[1], word[2]]);
            for i in 0..4 {
                if i <= chunk.len() {
                    let sextet = (bits >> (18 - 6 * i)) & 0x3F;
                    out.push(char::from(ALPHABET[sextet as usize]));
                } else {
                    out.push('=');
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{
        testing::{base64_encode, deflate_raw as deflate, zlib},
        *,
    };

    #[test]
    fn base64_roundtrips_every_length() {
        let data: Vec<u8> = (0..=255u8).collect();
        for len in 0..40 {
            let encoded = base64_encode(&data[..len]);
            assert_eq!(base64_decode(encoded.as_bytes()).unwrap(), &data[..len]);
        }
    }

    #[test]
    fn base64_decodes_every_padding_form() {
        assert_eq!(base64_decode(b"TWFu").unwrap(), b"Man");
        assert_eq!(base64_decode(b"TWE=").unwrap(), b"Ma");
        assert_eq!(base64_decode(b"TQ==").unwrap(), b"M");
        assert_eq!(base64_decode(b"").unwrap(), b"");
        assert_eq!(base64_decode(b"+/+/").unwrap(), [0xFB, 0xFF, 0xBF]);
    }

    #[test]
    fn base64_rejects_non_canonical_input() {
        for bad in [
            &b"TWE"[..],
            b"TQ=",
            b"T===",
            b"TQ==TWFu",
            b"TR==",
            b"TWF=",
            b"TW!u",
            b"TW=u",
        ] {
            assert!(base64_decode(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn inflate_roundtrips_raw_and_zlib() {
        let data: Vec<u8> = (0..20_000u32).map(|i| (i * 7 % 251) as u8).collect();
        assert_eq!(inflate_raw(&deflate(&data)).unwrap(), data);
        assert_eq!(inflate_zlib(&zlib(&data)).unwrap(), data);
        assert_eq!(inflate_raw(&deflate(b"")).unwrap(), b"");
    }

    #[test]
    fn inflate_rejects_truncated_and_corrupt_streams() {
        let data = b"the quick brown fox jumps over the lazy dog".repeat(40);
        let packed = zlib(&data);
        assert!(inflate_zlib(&packed[..packed.len() / 2]).is_none());
        assert!(inflate_zlib(b"not zlib").is_none());
        assert!(inflate_raw(&[]).is_none());
    }
}
