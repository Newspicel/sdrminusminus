use std::sync::LazyLock;

use sdrmm_dsp::{DVB_PRIMITIVE, ReedSolomon};

pub const RS6_N: usize = 52;
const RS6_NPAR: usize = 10;
const RS6_FCR: u32 = 54;
const GF64_ORDER: u32 = 63;
const GF64_PRIMITIVE: u16 = 0x43;

const RS8_DATA: usize = 31;
const RS8_SENT: usize = 39;
const RS8_OFFSET: usize = 208;
const RS8_ERASURES: [usize; 8] = [247, 248, 249, 250, 251, 252, 253, 254];

static RS8: LazyLock<ReedSolomon> = LazyLock::new(|| ReedSolomon::new(DVB_PRIMITIVE, 0, 16));

struct Gf64 {
    exp: [u8; 126],
    log: [u8; 64],
}

const GF64: Gf64 = Gf64::new();

impl Gf64 {
    const fn new() -> Self {
        let mut exp = [0u8; 126];
        let mut log = [0u8; 64];
        let mut x: u16 = 1;
        let mut i = 0;
        while i < 63 {
            exp[i] = x as u8;
            log[x as usize] = i as u8;
            x <<= 1;
            if x & 0x40 != 0 {
                x ^= GF64_PRIMITIVE;
            }
            i += 1;
        }
        while i < 126 {
            exp[i] = exp[i - 63];
            i += 1;
        }
        Self { exp, log }
    }

    fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 {
            return 0;
        }
        self.exp[usize::from(self.log[usize::from(a)]) + usize::from(self.log[usize::from(b)])]
    }

    fn div(&self, a: u8, b: u8) -> u8 {
        if a == 0 {
            return 0;
        }
        let d = (i32::from(self.log[usize::from(a)]) - i32::from(self.log[usize::from(b)]))
            .rem_euclid(GF64_ORDER as i32);
        self.exp[d as usize]
    }

    fn pow(&self, base_log: u32, e: u32) -> u8 {
        self.exp[((base_log * e) % GF64_ORDER) as usize]
    }

    fn eval_at(&self, poly: &[u8], x_log: u32) -> u8 {
        poly.iter()
            .enumerate()
            .filter(|(_, c)| **c != 0)
            .fold(0u8, |acc, (k, &c)| {
                acc ^ self.mul(c, self.pow(x_log, k as u32))
            })
    }
}

fn rs6_syndromes(cw: &[u8; RS6_N]) -> [u8; RS6_NPAR] {
    let gf = &GF64;
    let mut syndromes = [0u8; RS6_NPAR];
    for (j, s) in syndromes.iter_mut().enumerate() {
        let root_log = (RS6_FCR + j as u32) % GF64_ORDER;
        *s = cw
            .iter()
            .enumerate()
            .filter(|(_, c)| **c != 0)
            .fold(0u8, |acc, (i, &c)| {
                acc ^ gf.mul(c, gf.pow(root_log, (RS6_N - 1 - i) as u32))
            });
    }
    syndromes
}

fn rs6_locator(syndromes: &[u8; RS6_NPAR]) -> Option<[u8; RS6_NPAR + 1]> {
    let gf = &GF64;
    let mut lambda = [0u8; RS6_NPAR + 1];
    let mut prev = [0u8; RS6_NPAR + 1];
    lambda[0] = 1;
    prev[0] = 1;
    let (mut l, mut m, mut b) = (0usize, 1usize, 1u8);
    for r in 0..RS6_NPAR {
        let delta = (1..=l).fold(syndromes[r], |acc, i| {
            acc ^ gf.mul(lambda[i], syndromes[r - i])
        });
        if delta == 0 {
            m += 1;
            continue;
        }
        let before = lambda;
        let scale = gf.div(delta, b);
        for i in 0..=RS6_NPAR - m {
            lambda[i + m] ^= gf.mul(scale, prev[i]);
        }
        if 2 * l <= r {
            prev = before;
            l = r + 1 - l;
            b = delta;
            m = 1;
        } else {
            m += 1;
        }
    }
    (l <= RS6_NPAR / 2).then_some(lambda)
}

pub fn rs6_correct(cw: &mut [u8; RS6_N]) -> Option<u32> {
    let gf = &GF64;
    let syndromes = rs6_syndromes(cw);
    if syndromes.iter().all(|&s| s == 0) {
        return Some(0);
    }
    let lambda = rs6_locator(&syndromes)?;
    let degree = (0..=RS6_NPAR).rev().find(|&i| lambda[i] != 0).unwrap_or(0);
    let mut omega = [0u8; RS6_NPAR];
    for (j, o) in omega.iter_mut().enumerate() {
        *o = (0..=j.min(degree)).fold(0u8, |acc, k| acc ^ gf.mul(lambda[k], syndromes[j - k]));
    }
    let mut derivative = [0u8; RS6_NPAR + 1];
    for k in (1..=RS6_NPAR).step_by(2) {
        derivative[k - 1] = lambda[k];
    }
    let mut corrected = 0usize;
    for (i, symbol) in cw.iter_mut().enumerate() {
        let deg = (RS6_N - 1 - i) as u32;
        let x_inv_log = (GF64_ORDER - deg % GF64_ORDER) % GF64_ORDER;
        if gf.eval_at(&lambda, x_inv_log) != 0 {
            continue;
        }
        let slope = gf.eval_at(&derivative, x_inv_log);
        if slope == 0 {
            return None;
        }
        let adjust = (i64::from(deg) * (1 - i64::from(RS6_FCR))).rem_euclid(i64::from(GF64_ORDER));
        let magnitude = gf.div(gf.eval_at(&omega, x_inv_log), slope);
        *symbol ^= gf.mul(magnitude, gf.exp[adjust as usize]);
        corrected += 1;
    }
    if corrected != degree || rs6_syndromes(cw).iter().any(|&s| s != 0) {
        return None;
    }
    Some(corrected as u32)
}

pub fn rs8_correct(sent: &[u8; RS8_SENT]) -> Option<[u8; RS8_DATA]> {
    let mut cw = [0u8; 255];
    cw[RS8_OFFSET..RS8_OFFSET + RS8_SENT].copy_from_slice(sent);
    RS8.decode_with_erasures(&mut cw, &RS8_ERASURES)?;
    let mut out = [0u8; RS8_DATA];
    out.copy_from_slice(&cw[RS8_OFFSET..RS8_OFFSET + RS8_DATA]);
    Some(out)
}

pub fn iip_crc24(data: &[u8]) -> u32 {
    const REVERSED_POLY: u32 = 0xAD85DD;
    let mut crc: u32 = 0xFF_FFFF;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ REVERSED_POLY
            } else {
                crc >> 1
            };
        }
    }
    crc ^ 0x0C91B6
}

pub fn checksum_16(msg: &[u8; RS8_DATA]) -> u16 {
    let mut sum: u32 = msg[..28]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|w| u32::from(u16::from_le_bytes([w[0], w[1]])))
        .sum();
    sum += u32::from(msg[28]);
    sum += u32::from(u16::from_le_bytes([msg[29], msg[30]]));
    let folded = (sum & 0xFFFF) + (sum >> 16);
    (folded as u16) ^ 0xFFFF
}

pub fn bytes_of_bits(bits: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let straight: Vec<u8> = bits
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| c.iter().fold(0u8, |v, &b| (v << 1) | b))
        .collect();
    let reversed = straight.iter().map(|b| b.reverse_bits()).collect();
    (straight, reversed)
}

pub fn symbols6(bits: &[u8]) -> [u8; RS6_N] {
    let mut cw = [0u8; RS6_N];
    for (symbol, chunk) in cw.iter_mut().zip(bits.chunks(6)) {
        *symbol = chunk.iter().fold(0u8, |v, &b| (v << 1) | b);
    }
    cw
}

pub fn first_39(bytes: &[u8]) -> Option<[u8; RS8_SENT]> {
    bytes.get(..RS8_SENT)?.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc24_check_value() {
        assert_eq!(iip_crc24(b"123456789"), 0xbde882);
        assert_eq!(iip_crc24(b""), 0xf36e49);
    }
}
