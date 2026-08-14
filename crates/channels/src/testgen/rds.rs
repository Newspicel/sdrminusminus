//! RDS reference modulator (): the group sequence a station transmits, the shaped
//! 57 kHz DBPSK subcarrier it rides on, and the FM composite / transmission around it.
//!
//! Physical layer per EN 50067 (IEC 62106) §1.2 — 1187.5 bit/s differentially encoded biphase
//! data, shaped by the 100 % cosine roll-off the standard prescribes for the transmitter, on a
//! subcarrier locked to the third harmonic of the 19 kHz pilot. Group formats per §3.

use std::f64::consts::{PI, TAU};

use num_complex::Complex;
use sdrmm_dsp::{RdsOffset, rds_encode_block};

use super::fm_modulate;

/// Bit rate: the subcarrier divided by 48 (EN 50067 §1.2.2).
pub const BIT_RATE: f64 = 1_187.5;
/// Stereo pilot. The subcarrier is its third harmonic, in phase with it — the standard also
/// allows quadrature, which a receiver has to resolve on its own either way.
const PILOT_HZ: f64 = 19_000.0;
const SUBCARRIER_HZ: f64 = 3.0 * PILOT_HZ;
/// Peak deviation of a fully modulated broadcast carrier; [`composite`] returns ±1 for it.
const DEVIATION_HZ: f64 = 75_000.0;
/// Deviation shares of a typical multiplex: audio 45 %, pilot 9 %, RDS 4 % (EN 50067 §1.1
/// recommends ±2 kHz for the subcarrier, i.e. 4 % of the ±75 kHz peak once both sidebands
/// add).
const AUDIO_LEVEL: f64 = 0.45;
const PILOT_LEVEL: f64 = 0.09;
const RDS_LEVEL: f64 = 0.04;
/// Shaping-pulse truncation, in bit periods either side of its centre. The response decays as
/// 1/t², so five periods leaves the tails ~60 dB down.
const SHAPING_SPAN: f64 = 5.0;

const BLOCK_BITS: u32 = 26;
const GROUP_BITS: f64 = 104.0;
const PS_LEN: usize = 8;
const RT_LEN: usize = 64;
/// Characters a 2A group carries.
const RT_SEGMENT: usize = 4;
/// Ends a RadioText message shorter than 64 characters (EN 50067 §3.1.5.3).
const RT_TERMINATOR: u8 = 0x0D;
/// AF code 224+n announces n alternative frequencies; 1..=204 are 87.5 MHz + 100 kHz·code and
/// 205 is the "filler" that pads an odd list (EN 50067 §3.2.1.6.1).
const AF_COUNT_BASE: u8 = 224;
const AF_FILLER: u8 = 205;
const AF_BASE_HZ: f64 = 87_500_000.0;
const AF_STEP_HZ: f64 = 100_000.0;
const AF_MAX: usize = 25;

/// The station identity a generated transmission carries.
#[derive(Clone, Debug)]
pub struct Station {
    pub pi: u16,
    pub ps: String,
    pub radiotext: String,
    pub pty: u8,
    pub tp: bool,
    pub ta: bool,
    pub music: bool,
    pub alt_freqs_hz: Vec<f64>,
}

/// The 104-bit group sequence a station transmits, as `4·count` blocks with their offset
/// words already added — block A, B, C (or C′) and D of each group in transmission order.
#[must_use]
pub fn groups(station: &Station, count: usize) -> Vec<u32> {
    let cycle = cycle(station);
    let mut blocks = Vec::with_capacity(count * 4);
    for index in 0..count {
        let Some(group) = cycle.get(index % cycle.len().max(1)) else {
            break;
        };
        let c_offset = if group.version_b {
            RdsOffset::CPrime
        } else {
            RdsOffset::C
        };
        blocks.push(rds_encode_block(station.pi, RdsOffset::A));
        blocks.push(rds_encode_block(group.b, RdsOffset::B));
        blocks.push(rds_encode_block(group.c, c_offset));
        blocks.push(rds_encode_block(group.d, RdsOffset::D));
    }
    blocks
}

/// A complete FM composite at `rate`: 19 kHz pilot, the RDS subcarrier, and an optional mono
/// audio tone, summed at broadcast deviation shares and returned as a real signal in ±1.
#[must_use]
pub fn composite(
    station: &Station,
    seconds: f64,
    audio_tone_hz: Option<f64>,
    rate: f64,
) -> Vec<f32> {
    let len = (seconds.max(0.0) * rate) as usize;
    let data = subcarrier(station, len, rate);
    (0..len)
        .map(|n| {
            let t = n as f64 / rate;
            let audio = audio_tone_hz.map_or(0.0, |f| AUDIO_LEVEL * (TAU * f * t).cos());
            let pilot = PILOT_LEVEL * (TAU * PILOT_HZ * t).cos();
            let level = f64::from(data.get(n).copied().unwrap_or(0.0));
            let rds = RDS_LEVEL * level * (TAU * SUBCARRIER_HZ * t).cos();
            (audio + pilot + rds) as f32
        })
        .collect()
}

/// The same composite frequency-modulated onto a carrier as complex baseband IQ — what a WFM
/// channel receives off the air.
#[must_use]
pub fn transmission(
    station: &Station,
    seconds: f64,
    audio_tone_hz: Option<f64>,
    rate: f64,
) -> Vec<Complex<f32>> {
    fm_modulate(
        &composite(station, seconds, audio_tone_hz, rate),
        DEVIATION_HZ,
        rate,
    )
}

/// One group's three variable blocks; block A always carries the PI code.
struct Group {
    b: u16,
    c: u16,
    d: u16,
    version_b: bool,
}

/// The repeating group cycle: the four 0A groups carrying the PS name and the alternative
/// frequency list, then one 2A group per four RadioText characters.
fn cycle(station: &Station) -> Vec<Group> {
    let common = (u16::from(station.tp) << 10) | (u16::from(station.pty & 0x1F) << 5);
    let ps = ps_bytes(&station.ps);
    let af = af_pairs(&station.alt_freqs_hz);
    let mut groups = Vec::new();

    for segment in 0..PS_LEN / 2 {
        let flags = (u16::from(station.ta) << 4) | (u16::from(station.music) << 3);
        // Decoder-identification bit 2 stays 0: static PTY, mono — what this generator makes.
        let pair = af
            .get(segment % af.len().max(1))
            .copied()
            .unwrap_or([AF_FILLER; 2]);
        groups.push(Group {
            b: common | flags | segment as u16,
            c: u16::from(pair[0]) << 8 | u16::from(pair[1]),
            d: pair_at(&ps, 2 * segment),
            version_b: false,
        });
    }

    let rt = rt_bytes(&station.radiotext);
    for (segment, chunk) in rt.as_chunks::<RT_SEGMENT>().0.iter().enumerate() {
        // Text A/B flag (bit 4) stays 0: one message for the life of the transmission.
        groups.push(Group {
            b: (2 << 12) | common | segment as u16,
            c: pair_at(chunk, 0),
            d: pair_at(chunk, 2),
            version_b: false,
        });
    }
    groups
}

fn ps_bytes(name: &str) -> Vec<u8> {
    let mut bytes: Vec<u8> = name.bytes().take(PS_LEN).collect();
    bytes.resize(PS_LEN, b' ');
    bytes
}

fn rt_bytes(text: &str) -> Vec<u8> {
    let mut bytes: Vec<u8> = text.bytes().take(RT_LEN).collect();
    if bytes.len() < RT_LEN {
        bytes.push(RT_TERMINATOR);
    }
    // A 2A group carries four characters, so the message is padded out to a whole segment.
    while !bytes.len().is_multiple_of(RT_SEGMENT) {
        bytes.push(b' ');
    }
    bytes
}

fn pair_at(bytes: &[u8], index: usize) -> u16 {
    let at = |i: usize| u16::from(bytes.get(i).copied().unwrap_or(b' '));
    at(index) << 8 | at(index + 1)
}

fn af_code(hz: f64) -> Option<u8> {
    let code = ((hz - AF_BASE_HZ) / AF_STEP_HZ).round();
    (1.0..=204.0).contains(&code).then_some(code as u8)
}

/// The AF list as the byte pairs successive 0A groups carry: the count first, then the
/// frequencies, padded to a whole pair with the filler code.
fn af_pairs(freqs: &[f64]) -> Vec<[u8; 2]> {
    let codes: Vec<u8> = freqs
        .iter()
        .filter_map(|&f| af_code(f))
        .take(AF_MAX)
        .collect();
    let mut all = Vec::with_capacity(codes.len() + 2);
    all.push(AF_COUNT_BASE + codes.len() as u8);
    all.extend_from_slice(&codes);
    if !all.len().is_multiple_of(2) {
        all.push(AF_FILLER);
    }
    all.as_chunks::<2>().0.to_vec()
}

/// Impulse response of the transmitter data shaping, `H(f) = cos(πf/(4·fb))` over
/// `|f| ≤ 2·fb` (EN 50067 §1.2.4), at `t` seconds from its centre.
fn shaping(t: f64) -> f64 {
    let a = PI / (4.0 * BIT_RATE);
    let edge = 2.0 * BIT_RATE;
    // `H` has a removable singularity at t = ±a/2π, where its cosine is zero too; the limit
    // there is the band edge itself.
    if (t.abs() - a / TAU).abs() < 1e-9 {
        return edge;
    }
    2.0 * a * (TAU * edge * t).cos() / (a * a - 4.0 * PI * PI * t * t)
}

fn shaping_taps(rate: f64) -> Vec<f64> {
    let half = (SHAPING_SPAN * rate / BIT_RATE).round() as usize;
    (0..=2 * half)
        .map(|k| shaping((k as f64 - half as f64) / rate))
        .collect()
}

fn block_bits(blocks: &[u32], out: &mut Vec<bool>) {
    for &block in blocks {
        for k in (0..BLOCK_BITS).rev() {
            out.push(block >> k & 1 != 0);
        }
    }
}

/// Differential encoding (EN 50067 §1.2.3): the line level flips on a 1. This is what makes
/// the receiver immune to the subcarrier's 180° phase ambiguity.
fn differential(data: &[bool]) -> Vec<bool> {
    let mut last = false;
    data.iter()
        .map(|&bit| {
            last ^= bit;
            last
        })
        .collect()
}

/// The shaped biphase data waveform for `len` samples at `rate`, normalised to ±1.
fn subcarrier(station: &Station, len: usize, rate: f64) -> Vec<f32> {
    let count = (len as f64 * BIT_RATE / (rate * GROUP_BITS)).ceil() as usize + 1;
    let mut raw = Vec::new();
    block_bits(&groups(station, count), &mut raw);
    let line = differential(&raw);

    let taps = shaping_taps(rate);
    let half = taps.len() / 2;
    let half_symbol = 0.5 * rate / BIT_RATE;
    let mut wave = vec![0.0f64; len + taps.len()];
    'outer: for (i, &level) in line.iter().enumerate() {
        let amplitude = if level { 1.0 } else { -1.0 };
        for phase in 0..2 {
            // Biphase: a bit is its own level followed by the inverse, which empties the
            // spectrum at the subcarrier itself and at DC.
            let weight = if phase == 0 { amplitude } else { -amplitude };
            let centre = ((2 * i + phase) as f64 * half_symbol).round() as usize;
            if centre >= wave.len() {
                break 'outer;
            }
            for (slot, &tap) in wave[centre..].iter_mut().zip(&taps) {
                *slot += weight * tap;
            }
        }
    }

    // A pulse written at `centre` is centred `half` samples later, so output sample n is at
    // wave[n + half].
    let peak = wave
        .iter()
        .fold(0.0f64, |m, v| m.max(v.abs()))
        .max(f64::MIN_POSITIVE);
    wave[half..half + len]
        .iter()
        .map(|v| (v / peak) as f32)
        .collect()
}

#[cfg(test)]
mod tests {
    use sdrmm_dsp::rds_check_block;

    use super::*;

    fn station() -> Station {
        Station {
            pi: 0xD3C2,
            ps: "SDR--FM".to_owned(),
            radiotext: "reference modulator".to_owned(),
            pty: 10,
            tp: true,
            ta: false,
            music: true,
            alt_freqs_hz: vec![89_800_000.0, 95_100_000.0],
        }
    }

    #[test]
    fn every_block_checks_against_its_own_offset_word() {
        let blocks = groups(&station(), 12);
        assert_eq!(blocks.len(), 48);
        let offsets = [RdsOffset::A, RdsOffset::B, RdsOffset::C, RdsOffset::D];
        for (i, &block) in blocks.iter().enumerate() {
            assert!(block >> BLOCK_BITS == 0, "block {i} overflows 26 bits");
            let offset = offsets[i % 4];
            assert!(
                rds_check_block(block, offset).is_some(),
                "block {i} fails its {offset:?} check"
            );
        }
    }

    #[test]
    fn block_a_is_the_programme_identification_of_every_group() {
        let s = station();
        for group in groups(&s, 9).as_chunks::<4>().0 {
            assert_eq!(rds_check_block(group[0], RdsOffset::A), Some(s.pi));
        }
    }

    #[test]
    fn alternative_frequencies_are_announced_with_their_count() {
        let s = station();
        let first_c = groups(&s, 1)[2];
        let c = rds_check_block(first_c, RdsOffset::C).unwrap();
        assert_eq!((c >> 8) as u8, AF_COUNT_BASE + 2);
        assert_eq!(af_code(89_800_000.0), Some(23));
        assert_eq!(c as u8, 23);
    }

    #[test]
    fn composite_stays_inside_full_scale_and_carries_the_pilot() {
        let mpx = composite(&station(), 0.05, Some(1_000.0), 240_000.0);
        assert_eq!(mpx.len(), 12_000);
        for (n, &s) in mpx.iter().enumerate() {
            assert!((-1.0..=1.0).contains(&s), "sample {n} out of range: {s}");
        }
        // Correlate against the pilot: it must be there at its nominal share, phase 0.
        let corr: f64 = mpx
            .iter()
            .enumerate()
            .map(|(n, &s)| f64::from(s) * (TAU * PILOT_HZ * n as f64 / 240_000.0).cos())
            .sum::<f64>()
            / mpx.len() as f64;
        assert!(
            (corr - PILOT_LEVEL / 2.0).abs() < 0.005,
            "pilot level {corr}"
        );
    }

    #[test]
    fn transmission_is_a_unit_magnitude_carrier() {
        let iq = transmission(&station(), 0.01, None, 240_000.0);
        assert_eq!(iq.len(), 2_400);
        for (n, s) in iq.iter().enumerate() {
            assert!(
                (s.norm() - 1.0).abs() < 1e-3,
                "sample {n} magnitude {}",
                s.norm()
            );
        }
    }
}
