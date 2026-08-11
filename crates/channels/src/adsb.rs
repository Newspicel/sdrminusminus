//! ADS-B / Mode S decoder (PLAN §13 P2): 1090 MHz PPM at 1 Mbit/s, preamble correlation
//! and the Mode S CRC-24. A bit is two half-chips of 0.5 µs and a 1 is energy in the first of
//! them, so the whole decoder is a comparison between two windows.
//!
//! **It runs at the device's own rate** (`native_rate_max_hz`), which is the one thing this
//! decoder cannot compromise on: at 2 Msps a 0.5 µs pulse *is a single sample*, so any rate
//! conversion splits it across two, and both halves of every comparison come out the same.
//! Measured, not assumed — through the production DDC and through an unfiltered interpolation,
//! a 2.048 Msps signal resampled to 2.000 decodes nothing at all. So the decoder meets the radio
//! at its rate instead: a half-chip is 1.024 samples on an RTL-SDR, 1.2 on a 2.4 Msps one, and
//! the window boundaries are rounded per chip rather than stepped by a constant.
//!
//! It also meets the radio at its **phase**: the scan aligns to whole samples, but a
//! transmitter's bit clock owes the receiver's sample grid nothing, so every candidate is
//! sliced against a few sub-sample phase tables and the CRC picks the one that was right
//! (see [`Timing`]). dump1090 hard-codes the same two ideas for 2.4 Msps; this is the
//! any-rate form of them.
//!
//! Which downlink formats are accepted is a question about *proof of identity*, not about
//! parsing. DF17/18 extended squitters carry the address in the clear under a bare parity, so
//! a clean frame proves itself outright. DF4/5/20/21 roll-call replies carry no address at
//! all — it is only keyed onto the parity — so every 24-bit value reads as a valid frame and
//! decoding one off-air unconditionally would mean inventing aircraft out of noise.
//!
//! So a roll-call reply is decoded only when the address recovered from its parity belongs to
//! an aircraft a self-proving frame put on the air in the last [`ROLL_CALL_MAX_AGE_S`]
//! seconds. That is what makes Mode S worth having here: an aircraft with a transponder but no
//! ADS-B is heard through its all-call replies (DF11) and answers interrogations with altitude
//! (DF4/20) and squawk (DF5/21), none of which an extended-squitter-only decoder ever sees.

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{bits_be, mode_s_fix_single_bit, mode_s_overlay};
use sdrmm_wire::{
    AdsbMessage, AdsbParams, ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

/// The lowest device rate that carries the signal: two samples per bit, one per half-chip.
/// Below it a half-chip can hold no sample at all and the modulation is simply not there.
pub(crate) const INPUT_RATE_HZ: f64 = 2_000_000.0;
/// The highest. Every sample above this buys nothing a slicer can use, and the scan costs a
/// magnitude per sample on the DSP thread — the Pi 4 is the budget floor (PLAN §1), and the
/// rates a receiver actually offers for 1090 (2.048, 2.4, 2.56, 2.88, 3.2 Msps) all fit under it.
pub(crate) const MAX_INPUT_RATE_HZ: f64 = 4_000_000.0;

/// Half-chip: 0.5 µs, the resolution the whole waveform is defined on.
const CHIP_S: f64 = 0.5e-6;
/// 8 µs preamble with 0.5 µs pulses at 0.0, 1.0, 3.5 and 4.5 µs (ICAO Annex 10 Vol IV
/// §3.1.2.3.1) — half-chips 0, 2, 7 and 9 of sixteen.
const PREAMBLE_CHIPS: usize = 16;
const PREAMBLE_PULSES: [usize; 4] = [0, 2, 7, 9];
/// Preamble gaps a whole chip or more from every pulse, which must therefore be quiet. The
/// gaps *adjacent* to a pulse (1, 3, 6, 8, 10) are deliberately absent: at ~1 sample per chip
/// a band-limited pulse legitimately leaves up to half its energy in the neighbouring window,
/// so those are held to be weaker than their pulse, never to a level.
const PREAMBLE_FAR_GAPS: [usize; 7] = [4, 5, 11, 12, 13, 14, 15];
const SHORT_BYTES: usize = 7;
const LONG_BYTES: usize = 14;
/// Half-chips in a long frame: the preamble plus two per bit.
const LONG_FRAME_CHIPS: usize = PREAMBLE_CHIPS + LONG_BYTES * 8 * 2;
/// Largest ratio tolerated between the strongest and weakest preamble pulse. A real preamble
/// is four equal pulses; one noise spike plus three background samples is not one.
const PULSE_SPREAD: f32 = 4.0;

/// Sub-sample phases tried per candidate, `k / PHASE_TABLES` of a sample apart. The scan only
/// tries whole-sample alignments, so the tables cover the fraction in between; eight bounds
/// the residual mismatch to a sixteenth of a sample. Four was measured to be not enough — at
/// an eighth of a sample some bit patterns' first-versus-second margins invert and whole
/// frames vanish. Phase 0 keeps the decoder's original grid, so on a grid-aligned signal
/// nothing changes but the extra first-comparison rejects per sample.
const PHASE_TABLES: usize = 8;

/// A chip is at most 2.05 samples wide (4 Msps ceiling), so it overlaps at most three.
const CHIP_TAPS: usize = 3;

/// Where each half-chip of a long frame falls, as per-sample overlap weights, for one assumed
/// sub-sample phase.
///
/// Computed per chip and not stepped by a constant: at 2.048 Msps a half-chip is 1.024 samples,
/// so a fixed stride would drift a whole sample by the end of a 120 µs frame and slice the last
/// bits against the wrong halves.
///
/// Two things here were measured to be non-negotiable, and both come from the same field
/// failure — off-grid frames at 2.048 Msps decoded 0–6% while every test stayed green, because
/// the tests' generator shared the decoder's grid:
///
/// - **One phase table is not enough.** A transmitter's bit clock owes the receiver's sample
///   grid nothing, and at a non-integer samples-per-chip the leftover fraction shifts *within*
///   the frame (`frac(j × per_chip)` cycles with `j`), so whatever single phase is assumed,
///   some chip's energy lands in the neighbouring window — one flipped PPM bit and the CRC
///   drops the frame. Every table gets its chance and the CRC arbitrates.
/// - **Energy, not a peak.** A band-limited 0.5 µs pulse arrives with roughly a sample of rise
///   time, so at ~1 sample per chip its energy straddles two samples and a single-sample peak
///   cannot tell which chip owned it. The fractional-overlap sum can: with the right table
///   ~three quarters of the pulse lands in its own chip and an eighth leaks to each neighbour.
///   dump1090's 2.4 Msps demodulator hard-codes exactly this weighting, one rate at a time.
struct Timing {
    /// Per half-chip (`LONG_FRAME_CHIPS + 1` entries; the last is the consume boundary):
    /// index of its first sample in the frame window and the overlap of each touched sample.
    chips: Vec<(usize, [f32; CHIP_TAPS])>,
    /// Samples a full long frame spans at this phase — the window the scan slices.
    span: usize,
}

impl Timing {
    fn tables(input_rate: f64) -> Vec<Self> {
        (0..PHASE_TABLES)
            .map(|k| Self::new(input_rate, k as f64 / PHASE_TABLES as f64))
            .collect()
    }

    fn new(input_rate: f64, phase: f64) -> Self {
        let per_chip = input_rate * CHIP_S;
        let chips = (0..=LONG_FRAME_CHIPS)
            .map(|j| {
                let from = j as f64 * per_chip + phase;
                let to = from + per_chip;
                let start = from.floor() as usize;
                let mut weights = [0.0f32; CHIP_TAPS];
                for (i, w) in weights.iter_mut().enumerate() {
                    let k = (start + i) as f64;
                    *w = (to.min(k + 1.0) - from.max(k)).max(0.0) as f32;
                }
                (start, weights)
            })
            .collect();
        Self {
            chips,
            span: (LONG_FRAME_CHIPS as f64 * per_chip + phase).ceil() as usize,
        }
    }

    /// First sample of half-chip `chip` — the consume boundary once a frame is accepted.
    fn start(&self, chip: usize) -> usize {
        self.chips.get(chip).map_or(0, |&(start, _)| start)
    }

    fn frame_samples(&self) -> usize {
        self.span
    }

    /// Overlap-weighted magnitude of half-chip `chip` in a window starting at a frame's
    /// first sample.
    fn energy(&self, window: &[f32], chip: usize) -> f32 {
        let Some(&(start, weights)) = self.chips.get(chip) else {
            return 0.0;
        };
        weights
            .iter()
            .enumerate()
            .map(|(i, &w)| w * window.get(start + i).copied().unwrap_or(0.0))
            .sum()
    }
}

/// Bit offsets into a long frame (DO-260B §2.2.3): DF(5) CA(3) AA(24) ME(56) PI(24).
const ICAO_OFFSET_BITS: usize = 8;
const ME_OFFSET_BITS: usize = 32;

/// The 3 bits after DF are the capability of an all-call reply (ICAO Annex 10 Vol IV
/// §3.1.2.5.2.2.1) and the flight status of a surveillance reply (§3.1.2.6.5.1) — the same
/// three bits, two unrelated meanings, hence two names for the one offset.
const CAPABILITY_OFFSET_BITS: usize = 5;
const FLIGHT_STATUS_OFFSET_BITS: usize = 5;
/// A surveillance reply's header is DF(5) FS(3) DR(5) UM(6), and the 13-bit AC (DF4/20) or ID
/// (DF5/21) field follows it. The Comm-B message field of a DF20/21 starts where an extended
/// squitter's ME field does — both follow 32 header bits.
const REPLY_FIELD_OFFSET_BITS: usize = 19;
const MB_OFFSET_BITS: usize = ME_OFFSET_BITS;

/// BDS 2,0 — "aircraft identification" — is the one Comm-B register worth sniffing for here:
/// it holds the same 8-character callsign an extended squitter sends, for aircraft that never
/// send one (DO-181E §2.2.19.1.12).
const BDS_IDENTIFICATION: u64 = 0x20;

/// Legal PI values in an all-call reply: bits 1–17 are zero and only the 3-bit code label and
/// the 4-bit interrogator code remain (ICAO Annex 10 Vol IV §3.1.2.3.2.1.4).
const ALL_CALL_PI_MAX: u32 = 0x7F;

/// How long a self-proving frame vouches for its address. An aircraft in range sends all-call
/// replies at every antenna sweep and extended squitters twice a second, so a minute is many
/// missed frames — while an address that has gone quiet for one stops admitting replies that
/// nothing else can attribute.
const ROLL_CALL_MAX_AGE_S: f64 = 60.0;

/// 6-bit identification charset (DO-260B §2.2.3.2.5.2): index 0 and the reserved ranges are
/// `#`, 32 is a space.
const IDENT_CHARSET: &[u8; 64] =
    b"#ABCDEFGHIJKLMNOPQRSTUVWXYZ##### ###############0123456789######";

/// CPR zone height: 360° for airborne frames, 90° for surface ones (DO-260B §2.2.3.2.6.4).
const AIRBORNE_ZONE_DEG: f64 = 360.0;
const SURFACE_ZONE_DEG: f64 = 90.0;
const CPR_SCALE: f64 = 131_072.0;

/// Aircraft held for CPR even/odd pairing. A busy urban receiver hears 30–50 airframes at
/// once; past that the least recently heard entry is evicted, so an airshow (or a noisy
/// antenna inventing addresses) cannot grow this without bound.
const CPR_CACHE_LEN: usize = 64;
/// An even/odd pair may only be solved globally while both frames are fresh: DO-260B
/// §2.2.3.2.6.5 allows 10 s, beyond which the aircraft has flown out of its own zone. Held in
/// seconds because the sample clock is the device's now, not a constant.
const CPR_PAIR_MAX_AGE_S: f64 = 10.0;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "adsb".to_owned(),
    name: "ADS-B (1090ES)".to_owned(),
    bandwidth_hz: INPUT_RATE_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    decoder_kind: Some("adsb".to_owned()),
    native_rate_max_hz: Some(MAX_INPUT_RATE_HZ),
    ..ChannelDescriptor::default()
});

#[derive(Clone, Copy)]
struct CprFix {
    lat: u32,
    lon: u32,
    /// Absolute stream position of the frame, in samples — the DSP plane's only clock.
    at: u64,
}

struct Aircraft {
    icao: u32,
    even: Option<CprFix>,
    odd: Option<CprFix>,
    /// Stream position of the most recent accepted frame — the cache's eviction key.
    last: u64,
    /// Stream position of the most recent frame that *proved* this address, which is a
    /// stronger thing than having heard it: only a frame carrying the address in the clear
    /// may vouch for a reply that carries it nowhere but on the parity.
    proven: Option<u64>,
}

impl Aircraft {
    fn new(icao: u32, at: u64) -> Self {
        Self {
            icao,
            even: None,
            odd: None,
            last: at,
            proven: None,
        }
    }
}

pub struct AdsbChannel {
    crc_fix: bool,
    reference: Option<(f64, f64)>,
    /// Half-chip boundary tables at this radio's rate, one per assumed sub-sample phase —
    /// the decoder runs at whatever the device gives it (see [`Timing`]).
    timings: Vec<Timing>,
    /// The longest frame any table spans: the scan bound and the block-boundary carry.
    frame_span: usize,
    /// How far apart two CPR frames may be and still solve globally, in samples at this rate.
    cpr_pair_max_age: u64,
    /// How long a proved address vouches for a roll-call reply, in samples at this rate.
    roll_call_max_age: u64,
    /// Sample magnitudes: the tail of the previous block followed by the current one.
    mag: Vec<f32>,
    /// Absolute stream index of `mag[0]`.
    stream_pos: u64,
    cpr: Vec<Aircraft>,
}

fn params(settings: &ChannelSettings) -> Result<&AdsbParams, ChannelError> {
    match &settings.params {
        ChannelParams::Adsb(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "adsb channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_params(p: &AdsbParams) -> Result<(), ChannelError> {
    if let Some(lat) = p.ref_lat
        && !(lat.is_finite() && (-90.0..=90.0).contains(&lat))
    {
        return Err(ChannelError::InvalidSettings(format!(
            "adsb ref_lat must be within ±90°, got {lat}"
        )));
    }
    if let Some(lon) = p.ref_lon
        && !(lon.is_finite() && (-180.0..=180.0).contains(&lon))
    {
        return Err(ChannelError::InvalidSettings(format!(
            "adsb ref_lon must be within ±180°, got {lon}"
        )));
    }
    if p.ref_lat.is_some() != p.ref_lon.is_some() {
        return Err(ChannelError::InvalidSettings(
            "adsb reference position needs both ref_lat and ref_lon".to_owned(),
        ));
    }
    Ok(())
}

/// Occupied RF band relative to the channel offset, in Hz.
pub(crate) fn occupied_band() -> (f64, f64) {
    let half = INPUT_RATE_HZ / 2.0;
    (-half, half)
}

/// ADS-B keeps the DDC's full output band: the pulses are 0.5 µs, so every extra filter
/// stage costs rise time the bit slicer needs. The DDC's own anti-alias response is the
/// channel selectivity here.
pub(crate) fn channel_filter() -> ChannelFilter {
    ChannelFilter::Passthrough
}

/// Preamble correlation over one 16-sample window. The accept threshold is derived from the
/// pulses themselves — receive levels differ by tens of dB between an overhead aircraft and
/// one at the horizon, so no fixed level can gate this.
fn preamble_ok(timing: &Timing, window: &[f32]) -> bool {
    let chip = |index: usize| timing.energy(window, index);
    // Cheapest discriminator first: noise fails one of these early most of the time, which is
    // what keeps the per-sample cost near the magnitude computation itself. The gaps at 1 and
    // 8 sit *between* two pulses and collect band-limited tails from both sides, so each is
    // judged against its two pulses jointly — chip-by-chip the margin can shrink to nothing
    // at the worst sub-sample phases, while the pair keeps a clear one. The outer gaps see
    // one tail at most and a plain ordering holds.
    if !(chip(0) + chip(2) > 2.0 * chip(1)
        && chip(2) > chip(3)
        && chip(7) > chip(6)
        && chip(7) + chip(9) > 2.0 * chip(8)
        && chip(9) > chip(10))
    {
        return false;
    }
    let pulses = PREAMBLE_PULSES.map(chip);
    let far_gaps = PREAMBLE_FAR_GAPS.map(chip);
    let mean = pulses.iter().sum::<f32>() * 0.25;
    if mean <= 0.0 {
        return false;
    }
    let weakest = pulses.iter().copied().fold(f32::INFINITY, f32::min);
    let strongest = pulses.iter().copied().fold(0.0, f32::max);
    if strongest > weakest * PULSE_SPREAD {
        return false;
    }
    let threshold = mean * 0.5;
    weakest > threshold && far_gaps.iter().all(|&g| g < threshold)
}

/// PPM slicing: a 1 is energy in the first half of the bit, a 0 in the second. `window` starts
/// at the frame's first sample, so the body's half-chips are numbered from the preamble's end.
fn slice_bits(timing: &Timing, window: &[f32], frame: &mut [u8; LONG_BYTES]) {
    for (index, byte) in frame.iter_mut().enumerate() {
        let mut value = 0u8;
        for bit in 0..8 {
            let chip = PREAMBLE_CHIPS + (index * 8 + bit) * 2;
            let first = timing.energy(window, chip);
            let second = timing.energy(window, chip + 1);
            value = value << 1 | u8::from(first > second);
        }
        *byte = value;
    }
}

fn hex_upper(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        for nibble in [b >> 4, b & 0x0F] {
            if let Some(c) = char::from_digit(u32::from(nibble), 16) {
                out.push(c.to_ascii_uppercase());
            }
        }
    }
    out
}

/// Gray code to binary, for the Gillham altitude fields (any width up to 16 bits).
fn gray_decode(gray: u32) -> u32 {
    let mut b = gray;
    b ^= b >> 8;
    b ^= b >> 4;
    b ^= b >> 2;
    b ^= b >> 1;
    b
}

/// 12-bit AC field of an airborne position frame (DO-260B §2.2.3.2.3.4.3). Bit 4 of the
/// field is Q: set for the 25 ft encoding, clear for the Gillham-coded 100 ft one used above
/// 50 175 ft. `None` when the transmitter reports no altitude.
fn barometric_altitude(ac12: u32) -> Option<i32> {
    if ac12 == 0 {
        return None;
    }
    if ac12 & 0x10 != 0 {
        let n = ((ac12 & 0x0FE0) >> 1) | (ac12 & 0x000F);
        return Some(i32::try_from(n).ok()? * 25 - 1000);
    }
    gillham_altitude(ac12)
}

/// Mode C altitude from the 12-bit AC field with Q clear. Field order is
/// C1 A1 C2 A2 C4 A4 B1 D1 B2 D2 B4 D4 — the 13-bit interrogation field without its M bit.
fn gillham_altitude(ac12: u32) -> Option<i32> {
    let bit = |index: u32| (ac12 >> (11 - index)) & 1;
    let (c1, a1, c2, a2, c4, a4) = (bit(0), bit(1), bit(2), bit(3), bit(4), bit(5));
    let (b1, b2, d2, b4, d4) = (bit(6), bit(8), bit(9), bit(10), bit(11));
    // D1 (bit 7 here) is the Q bit and is zero on this path, so the 500 ft Gray code starts
    // at D2.
    let five_hundreds =
        gray_decode(d2 << 7 | d4 << 6 | a1 << 5 | a2 << 4 | a4 << 3 | b1 << 2 | b2 << 1 | b4);
    let mut hundreds = gray_decode(c1 << 2 | c2 << 1 | c4);
    // The C bits count 1..5 with 5 and 7 exchanged, and run backwards inside odd 500 ft bands.
    if hundreds & 5 == 5 {
        hundreds ^= 2;
    }
    if !(1..=5).contains(&hundreds) {
        return None;
    }
    if five_hundreds & 1 == 1 {
        hundreds = 6 - hundreds;
    }
    let steps = i32::try_from(five_hundreds * 5 + hundreds).ok()? - 13;
    (steps >= -12).then_some(steps * 100)
}

/// 13-bit AC field of a DF4/DF20 altitude reply (ICAO Annex 10 Vol IV §3.1.2.6.5.4). It is the
/// extended squitter's 12-bit field with an M bit inserted after A4: M clear means feet, and
/// the Q bit below it then selects the 25 ft or Gillham encoding exactly as it does there.
fn surveillance_altitude(ac13: u32) -> Option<i32> {
    // All zero is "no altitude information", and the metric encoding M marks is not defined by
    // the standard — reporting either as an altitude would be inventing one.
    if ac13 == 0 || ac13 & 0x0040 != 0 {
        return None;
    }
    barometric_altitude((ac13 & 0x1F80) >> 1 | (ac13 & 0x003F))
}

/// 13-bit ID field of a DF5/DF21 identity reply as the four octal digits a controller reads
/// (ICAO Annex 10 Vol IV §3.1.2.6.7.1). Field order is C1 A1 C2 A2 C4 A4 X B1 D1 B2 D2 B4 D4,
/// and each digit's bits are named for their weight, so the layout is an interleave rather
/// than four consecutive triples.
fn squawk(id13: u32) -> String {
    let bit = |index: u32| (id13 >> (12 - index)) & 1;
    let digit = |four: u32, two: u32, one: u32| bit(four) << 2 | bit(two) << 1 | bit(one);
    let (a, b, c, d) = (
        digit(5, 3, 1),
        digit(11, 9, 7),
        digit(4, 2, 0),
        digit(12, 10, 8),
    );
    format!("{a}{b}{c}{d}")
}

/// Flight status of a surveillance reply, as the airborne/on-ground answer it contains
/// (ICAO Annex 10 Vol IV §3.1.2.6.5.1). Codes 4 and 5 report the SPI ident pulse and say
/// nothing about the air/ground state, so they leave it unknown rather than guessing.
fn flight_status_on_ground(fs: u64) -> Option<bool> {
    (fs <= 3).then_some(fs == 1 || fs == 3)
}

/// Capability field of an all-call reply (ICAO Annex 10 Vol IV §3.1.2.5.2.2.1): 4 and 5 are a
/// level-2+ transponder declaring itself on the ground and airborne; 6 means it can be either
/// and 0–3 say nothing.
fn capability_on_ground(ca: u64) -> Option<bool> {
    match ca {
        4 => Some(true),
        5 => Some(false),
        _ => None,
    }
}

/// The 8 six-bit characters starting at `offset_bits`, trailing pad removed. Both the extended
/// squitter's identification field and the Comm-B identification register spell a callsign this
/// way, at different offsets in different frames.
fn callsign(frame: &[u8], offset_bits: usize) -> Option<String> {
    let mut text = String::with_capacity(8);
    for i in 0..8 {
        let code = bits_be(frame, offset_bits + i * 6, 6) as usize;
        text.push(char::from(*IDENT_CHARSET.get(code)?));
    }
    let trimmed = text.trim_end();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Callsign from a DF20/DF21 Comm-B reply, when its MB field reads as BDS 2,0.
///
/// A Comm-B register says nowhere which register it is, so this is a guess, checked twice: the
/// leading octet must be the register's own code, and all 8 characters must be defined ones.
/// A `#` means some other register was read as this one and the callsign would be an artefact.
fn comm_b_callsign(frame: &[u8]) -> Option<String> {
    if bits_be(frame, MB_OFFSET_BITS, 8) != BDS_IDENTIFICATION {
        return None;
    }
    let text = callsign(frame, MB_OFFSET_BITS + 8)?;
    (!text.contains('#')).then_some(text)
}

/// Airborne velocity, TC 19 subtypes 1 and 2 (DO-260B §2.2.3.2.6.1). Subtypes 3/4 report
/// airspeed and heading instead of a ground vector and are deliberately left undecoded.
fn velocity(frame: &[u8], msg: &mut AdsbMessage) {
    let subtype = bits_be(frame, ME_OFFSET_BITS + 5, 3);
    if subtype == 1 || subtype == 2 {
        // Subtype 2 is the supersonic scale: the same fields in 4 kt steps.
        let scale = if subtype == 2 { 4.0 } else { 1.0 };
        let east_west = bits_be(frame, ME_OFFSET_BITS + 14, 10);
        let north_south = bits_be(frame, ME_OFFSET_BITS + 25, 10);
        // Zero means "no velocity information"; the encoded value is the speed plus one.
        if east_west != 0 && north_south != 0 {
            let sign = |bit: usize| {
                if bits_be(frame, ME_OFFSET_BITS + bit, 1) == 1 {
                    -1.0
                } else {
                    1.0
                }
            };
            let vx = sign(13) * (east_west - 1) as f64 * scale;
            let vy = sign(24) * (north_south - 1) as f64 * scale;
            msg.ground_speed_kt = Some(vx.hypot(vy));
            msg.track_deg = Some(vx.atan2(vy).to_degrees().rem_euclid(360.0));
        }
    }
    let rate = bits_be(frame, ME_OFFSET_BITS + 37, 9);
    if rate != 0 {
        let fpm = (rate as i32 - 1) * 64;
        msg.vertical_rate_fpm = Some(if bits_be(frame, ME_OFFSET_BITS + 36, 1) == 1 {
            -fpm
        } else {
            fpm
        });
    }
}

/// Positive-remainder modulo. CPR is built on it: `%` would return a negative remainder for
/// southern latitudes and western longitudes and put the aircraft a zone away.
fn modulo(a: f64, b: f64) -> f64 {
    a - b * (a / b).floor()
}

/// Longitude zones as a function of latitude (DO-260B Appendix A table A-2): the table lists
/// the northern limit of each zone count, from 59 down to 2.
const NL_BOUNDARIES: [f64; 58] = [
    10.470_471_30,
    14.828_174_37,
    18.186_263_57,
    21.029_394_93,
    23.545_044_87,
    25.829_247_07,
    27.938_987_10,
    29.911_356_86,
    31.772_097_08,
    33.539_934_36,
    35.228_995_98,
    36.850_251_08,
    38.412_418_92,
    39.922_566_84,
    41.386_518_32,
    42.809_140_12,
    44.194_549_51,
    45.546_267_23,
    46.867_332_52,
    48.160_391_28,
    49.427_764_39,
    50.671_501_66,
    51.893_424_69,
    53.095_161_53,
    54.278_174_72,
    55.443_784_44,
    56.593_187_56,
    57.727_473_54,
    58.847_637_76,
    59.954_592_77,
    61.049_177_74,
    62.132_166_59,
    63.204_274_79,
    64.266_165_23,
    65.318_453_10,
    66.361_710_08,
    67.396_467_74,
    68.423_220_22,
    69.442_426_31,
    70.454_510_75,
    71.459_864_73,
    72.458_845_45,
    73.451_774_42,
    74.438_934_16,
    75.420_562_57,
    76.396_843_91,
    77.367_894_61,
    78.333_740_83,
    79.294_282_25,
    80.249_232_13,
    81.198_013_49,
    82.139_569_81,
    83.071_994_45,
    83.991_735_63,
    84.891_661_91,
    85.755_416_21,
    86.535_369_98,
    87.000_000_00,
];

fn cpr_nl(lat: f64) -> i32 {
    let lat = lat.abs();
    NL_BOUNDARIES
        .iter()
        .position(|&limit| lat < limit)
        .map_or(1, |zone| 59 - zone as i32)
}

/// Global CPR: an even/odd pair fixes the position outright (DO-260B §2.2.3.2.6.5). Airborne
/// frames only — surface zones are a quarter as tall, which leaves a four-way ambiguity that
/// only a receiver reference resolves.
fn cpr_global(even: &CprFix, odd: &CprFix, latest_odd: bool) -> Option<(f64, f64)> {
    let lat_even = f64::from(even.lat) / CPR_SCALE;
    let lat_odd = f64::from(odd.lat) / CPR_SCALE;
    let lon_even = f64::from(even.lon) / CPR_SCALE;
    let lon_odd = f64::from(odd.lon) / CPR_SCALE;

    let zone = (59.0 * lat_even - 60.0 * lat_odd + 0.5).floor();
    let mut rlat_even = (AIRBORNE_ZONE_DEG / 60.0) * (modulo(zone, 60.0) + lat_even);
    let mut rlat_odd = (AIRBORNE_ZONE_DEG / 59.0) * (modulo(zone, 59.0) + lat_odd);
    // Latitudes come out in [0, 360); the southern hemisphere is the upper quarter.
    if rlat_even >= 270.0 {
        rlat_even -= 360.0;
    }
    if rlat_odd >= 270.0 {
        rlat_odd -= 360.0;
    }
    if !(-90.0..=90.0).contains(&rlat_even) || !(-90.0..=90.0).contains(&rlat_odd) {
        return None;
    }
    let nl = cpr_nl(rlat_even);
    // Straddling a zone boundary makes the pair inconsistent: wait for the next frame.
    if nl != cpr_nl(rlat_odd) {
        return None;
    }

    let (lat, lon_cpr, i) = if latest_odd {
        (rlat_odd, lon_odd, 1)
    } else {
        (rlat_even, lon_even, 0)
    };
    let ni = f64::from((nl - i).max(1));
    let m = (lon_even * f64::from(nl - 1) - lon_odd * f64::from(nl) + 0.5).floor();
    let dlon = AIRBORNE_ZONE_DEG / ni;
    let mut lon = dlon * (modulo(m, ni) + lon_cpr);
    if lon >= 180.0 {
        lon -= 360.0;
    }
    Some((lat, lon))
}

/// Local CPR: one frame plus a reference position within half a zone (±3° airborne,
/// ±0.75° on the surface) of the aircraft.
fn cpr_local(fix: &CprFix, odd: bool, ref_lat: f64, ref_lon: f64, zone: f64) -> Option<(f64, f64)> {
    let i = i32::from(odd);
    let lat_cpr = f64::from(fix.lat) / CPR_SCALE;
    let lon_cpr = f64::from(fix.lon) / CPR_SCALE;

    let dlat = zone / f64::from(60 - i);
    let j = (ref_lat / dlat).floor() + (modulo(ref_lat, dlat) / dlat - lat_cpr + 0.5).floor();
    let lat = dlat * (j + lat_cpr);
    if !(-90.0..=90.0).contains(&lat) {
        return None;
    }

    let ni = f64::from((cpr_nl(lat) - i).max(1));
    let dlon = zone / ni;
    let m = (ref_lon / dlon).floor() + (modulo(ref_lon, dlon) / dlon - lon_cpr + 0.5).floor();
    let mut lon = dlon * (m + lon_cpr);
    if lon >= 180.0 {
        lon -= 360.0;
    }
    Some((lat, lon))
}

/// The AA field: the aircraft address as DF11/17/18 transmit it, in the clear.
fn clear_address(frame: &[u8]) -> u32 {
    bits_be(frame, ICAO_OFFSET_BITS, 24) as u32
}

/// A DF4/5/20/21 reply to a ground interrogation. The 13-bit field after the header is an
/// altitude in the altitude formats and an identity code in the identity ones; the Comm-B
/// formats carry 56 further bits whose register the frame does not name.
fn surveillance_reply(frame: &[u8], df: u8, msg: &mut AdsbMessage) {
    msg.on_ground = flight_status_on_ground(bits_be(frame, FLIGHT_STATUS_OFFSET_BITS, 3));
    let field = bits_be(frame, REPLY_FIELD_OFFSET_BITS, 13) as u32;
    match df {
        4 | 20 => msg.altitude_ft = surveillance_altitude(field),
        _ => msg.squawk = Some(squawk(field)),
    }
    if matches!(df, 20 | 21) {
        msg.callsign = comm_b_callsign(frame);
    }
}

impl AdsbChannel {
    /// Cache slot for `icao`, evicting the least recently heard aircraft when full.
    fn slot(&mut self, icao: u32, at: u64) -> Option<&mut Aircraft> {
        if let Some(index) = self.cpr.iter().position(|a| a.icao == icao) {
            return self.cpr.get_mut(index);
        }
        if self.cpr.len() < CPR_CACHE_LEN {
            self.cpr.push(Aircraft::new(icao, at));
            return self.cpr.last_mut();
        }
        let oldest = self
            .cpr
            .iter()
            .enumerate()
            .min_by_key(|(_, a)| a.last)
            .map(|(index, _)| index)?;
        let entry = self.cpr.get_mut(oldest)?;
        *entry = Aircraft::new(icao, at);
        Some(entry)
    }

    /// Record an airborne CPR frame and solve the position when it completes a fresh pair.
    fn pair(&mut self, icao: u32, fix: CprFix, odd: bool) -> Option<(f64, f64)> {
        let entry = self.slot(icao, fix.at)?;
        entry.last = fix.at;
        if odd {
            entry.odd = Some(fix);
        } else {
            entry.even = Some(fix);
        }
        let (Some(even), Some(odd_fix)) = (entry.even, entry.odd) else {
            return None;
        };
        (even.at.abs_diff(odd_fix.at) <= self.cpr_pair_max_age)
            .then(|| cpr_global(&even, &odd_fix, odd))
            .flatten()
    }

    fn fill_position(
        &mut self,
        frame: &[u8],
        icao: u32,
        at: u64,
        zone: f64,
        msg: &mut AdsbMessage,
    ) {
        let odd = bits_be(frame, ME_OFFSET_BITS + 21, 1) == 1;
        let fix = CprFix {
            lat: bits_be(frame, ME_OFFSET_BITS + 22, 17) as u32,
            lon: bits_be(frame, ME_OFFSET_BITS + 39, 17) as u32,
            at,
        };
        let global = (zone == AIRBORNE_ZONE_DEG)
            .then(|| self.pair(icao, fix, odd))
            .flatten();
        let solved = global.or_else(|| {
            self.reference
                .and_then(|(lat, lon)| cpr_local(&fix, odd, lat, lon, zone))
        });
        if let Some((lat, lon)) = solved {
            msg.lat = Some(lat);
            msg.lon = Some(lon);
        }
    }

    /// Note that `icao` was heard, and — for a frame carrying it in the clear — that it was
    /// proved. Only the latter admits roll-call replies (see [`AdsbChannel::vouched`]).
    fn observe(&mut self, icao: u32, at: u64, proved: bool) {
        let Some(entry) = self.slot(icao, at) else {
            return;
        };
        entry.last = at;
        if proved {
            entry.proven = Some(at);
        }
    }

    /// Whether a frame recent enough to vouch for a roll-call reply has proved `icao`.
    ///
    /// A reply refreshes `last` — it is evidence the aircraft is still there, and the cache
    /// should not evict it — but never `proven`: proof of identity does not decay into
    /// hearsay, or one lucky parity match would keep a bogus address alive forever.
    fn vouched(&self, icao: u32, at: u64) -> bool {
        self.cpr.iter().any(|a| {
            a.icao == icao
                && a.proven
                    .is_some_and(|t| at.abs_diff(t) <= self.roll_call_max_age)
        })
    }

    fn message(&mut self, frame: &[u8], df: u8, icao: u32, at: u64) -> AdsbMessage {
        let mut msg = AdsbMessage {
            icao: format!("{icao:06X}"),
            df,
            raw: hex_upper(frame),
            ..AdsbMessage::default()
        };
        match df {
            17 | 18 => self.extended_squitter(frame, icao, at, &mut msg),
            11 => {
                msg.on_ground = capability_on_ground(bits_be(frame, CAPABILITY_OFFSET_BITS, 3));
            }
            _ => surveillance_reply(frame, df, &mut msg),
        }
        msg
    }

    /// The ME field of an extended squitter: what the aircraft chose to broadcast about
    /// itself, selected by the 5-bit type code.
    fn extended_squitter(&mut self, frame: &[u8], icao: u32, at: u64, msg: &mut AdsbMessage) {
        let type_code = bits_be(frame, ME_OFFSET_BITS, 5) as u8;
        msg.type_code = Some(type_code);
        let altitude = || bits_be(frame, ME_OFFSET_BITS + 8, 12) as u32;
        match type_code {
            1..=4 => msg.callsign = callsign(frame, ME_OFFSET_BITS + 8),
            5..=8 => {
                msg.on_ground = Some(true);
                self.fill_position(frame, icao, at, SURFACE_ZONE_DEG, msg);
            }
            9..=18 => {
                msg.on_ground = Some(false);
                msg.altitude_ft = barometric_altitude(altitude());
                self.fill_position(frame, icao, at, AIRBORNE_ZONE_DEG, msg);
            }
            19 => velocity(frame, msg),
            // The type code selects the altitude *source* (GNSS height above the ellipsoid
            // rather than barometric), not its encoding: the AC12 field is the same Q-bit /
            // Gillham code in feet. Reading it as metres is the mode-s.org interpretation
            // that dump1090, readsb, java-adsb and rs1090 all contradict — and 12 bits of
            // metres tops out at 13 435 ft, which cannot express the altitude of the
            // high-integrity GNSS traffic that emits these very type codes.
            20..=22 => {
                msg.on_ground = Some(false);
                msg.altitude_ft = barometric_altitude(altitude());
                self.fill_position(frame, icao, at, AIRBORNE_ZONE_DEG, msg);
            }
            _ => {}
        }
    }

    /// Who sent this frame, and whether the frame itself is proof of it.
    ///
    /// `None` rejects the frame. Every accept here is a claim that an aircraft exists, so each
    /// downlink format is admitted on the strength of its own parity and nothing weaker:
    ///
    /// - DF17/18 transmit their parity bare, so a clean frame is a 1-in-16-million coincidence
    ///   away from certain, and the address sits in the clear beside it.
    /// - DF11 keys the interrogator identifier onto the parity, leaving 17 bits bare. The
    ///   address is still in the clear, and the residual 1-in-131-072 is what buys the aircraft
    ///   that never send an extended squitter at all.
    /// - DF4/5/20/21 key the *address* onto the parity, so the frame proves nothing by itself:
    ///   every value reads as valid, and the recovered address has to have been proved
    ///   already by one of the formats above.
    fn attribute(&self, frame: &mut [u8], df: u8, at: u64) -> Option<(u32, bool)> {
        match df {
            17 | 18 => {
                if mode_s_overlay(frame)? != 0 {
                    // A flipped bit inside DF picks the wrong frame length, so the syndrome
                    // cannot close over the right byte count: such frames are dropped, never
                    // mis-repaired. Repair is for bare parity only — on an overlaid one it
                    // would "fix" a real frame into a different aircraft's.
                    if !self.crc_fix || mode_s_fix_single_bit(frame).is_none() {
                        return None;
                    }
                }
                Some((clear_address(frame), true))
            }
            11 => (mode_s_overlay(frame)? <= ALL_CALL_PI_MAX).then(|| (clear_address(frame), true)),
            4 | 5 | 20 | 21 => {
                let icao = mode_s_overlay(frame)?;
                self.vouched(icao, at).then_some((icao, false))
            }
            _ => None,
        }
    }

    /// Try to decode a frame starting at `at` in [`Self::mag`], returning the samples it
    /// consumed. Every phase table gets its chance and the first accepted frame wins; `None`
    /// means "not a frame here at any phase" and the scan advances one sample.
    fn try_frame(&mut self, at: usize, out: &mut ChannelOutputs) -> Option<usize> {
        let stamp = self.stream_pos + at as u64;
        let mut hit = None;
        for timing in &self.timings {
            let Some(window) = self.mag.get(at..at + timing.frame_samples()) else {
                continue;
            };
            if !preamble_ok(timing, window) {
                continue;
            }
            let mut frame = [0u8; LONG_BYTES];
            slice_bits(timing, window, &mut frame);

            let df = frame.first().map_or(0, |&b| b >> 3);
            let len = if df >= 16 { LONG_BYTES } else { SHORT_BYTES };
            let Some(bytes) = frame.get_mut(..len) else {
                continue;
            };
            let Some((icao, proved)) = self.attribute(bytes, df, stamp) else {
                continue;
            };
            hit = Some((
                frame,
                len,
                df,
                icao,
                proved,
                timing.start(PREAMBLE_CHIPS + len * 8 * 2),
            ));
            break;
        }
        let (frame, len, df, icao, proved, consumed) = hit?;
        self.observe(icao, stamp, proved);
        let message = self.message(frame.get(..len)?, df, icao, stamp);
        out.events.push(DecoderEvent::Adsb(message));
        Some(consumed)
    }
}

impl ChannelRx for AdsbChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        check_params(p)?;
        let timings = Timing::tables(ctx.input_rate);
        let frame_span = timings.iter().map(Timing::frame_samples).max().unwrap_or(0);
        Ok(Self {
            crc_fix: p.crc_fix,
            reference: p.ref_lat.zip(p.ref_lon),
            timings,
            frame_span,
            cpr_pair_max_age: (CPR_PAIR_MAX_AGE_S * ctx.input_rate) as u64,
            roll_call_max_age: (ROLL_CALL_MAX_AGE_S * ctx.input_rate) as u64,
            mag: Vec::new(),
            stream_pos: 0,
            cpr: Vec::with_capacity(CPR_CACHE_LEN),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        check_params(p)?;
        self.crc_fix = p.crc_fix;
        self.reference = p.ref_lat.zip(p.ref_lon);
        Ok(())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        // Steady-state cost per input sample: one magnitude (two multiplies, an add and a
        // square root — `norm()` would call `hypot`, an order of magnitude slower for
        // overflow safety this signal cannot need) plus the four-comparison preamble reject.
        self.mag
            .extend(iq.iter().map(|s| s.re.mul_add(s.re, s.im * s.im).sqrt()));

        let frame_span = self.frame_span;
        let mut at = 0;
        while at + frame_span <= self.mag.len() {
            at += self.try_frame(at, out).unwrap_or(1);
        }

        // Keep everything a frame could still start in. Only offsets with a full frame (at
        // the longest phase table) behind them are scanned, and those are exactly the ones
        // dropped here, so results never depend on where the host cut the block — and no
        // frame is emitted twice.
        //
        // The window is the *long* frame's for every candidate, so a short reply (DF4/5/11)
        // waits for a long frame's worth of samples — 232 µs — before the scan reaches it.
        // On a live stream that is latency and nothing else; a capture that ends inside those
        // 232 µs loses its last short frame. Decoding one earlier means scanning positions
        // that a later block would have to be stopped from re-scanning, which is a watermark
        // this decoder does not carry.
        let keep = self.mag.len().saturating_sub(frame_span - 1);
        self.mag.drain(..keep);
        self.stream_pos += keep as u64;
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::{PI, TAU};

    use sdrmm_wire::NfmParams;

    use super::*;
    use crate::{
        testgen::{
            add_noise,
            adsb::{
                all_call_reply, altitude_reply, comm_b_altitude_reply, comm_b_identity_reply,
                identity_reply, mb_identification, me_airborne_position, me_airborne_position_gnss,
                me_identification, me_surface_position, me_velocity, position_me_raw, squitter,
                transmission, transmission_at_phase,
            },
        },
        testutil::settings,
    };

    /// Berlin: comfortably away from every NL zone boundary, so the decoder's table and the
    /// generator's closed-form NL cannot disagree for reasons unrelated to the test.
    const LAT: f64 = 52.257_2;
    const LON: f64 = 13.409_1;
    const LEVEL: f32 = 0.5;
    const GAP_US: f64 = 30.0;

    fn adsb_params(p: AdsbParams) -> ChannelSettings {
        settings(ChannelParams::Adsb(p))
    }

    fn channel(p: AdsbParams) -> AdsbChannel {
        AdsbChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            adsb_params(p),
        )
        .unwrap()
    }

    fn feed(chan: &mut AdsbChannel, iq: &[Complex<f32>], blocks: &[usize]) -> Vec<AdsbMessage> {
        let mut out = ChannelOutputs::default();
        let mut messages = Vec::new();
        let mut pos = 0;
        for len in blocks.iter().cycle() {
            if pos >= iq.len() {
                break;
            }
            let end = (pos + len).min(iq.len());
            out.reset();
            chan.process(&iq[pos..end], &mut out);
            assert!(out.audio_pcm.is_empty(), "adsb must not produce audio");
            for event in &out.events {
                let DecoderEvent::Adsb(m) = event else {
                    panic!("adsb channel emitted {}", event.kind())
                };
                messages.push(m.clone());
            }
            pos = end;
        }
        messages
    }

    fn decode(p: AdsbParams, frames: &[Vec<u8>]) -> Vec<AdsbMessage> {
        let iq = transmission(frames, GAP_US, LEVEL, INPUT_RATE_HZ);
        feed(&mut channel(p), &iq, &[4_096])
    }

    /// Frame body as published in the Mode S literature; parity is appended here.
    fn published(hex: &str) -> Vec<u8> {
        let mut frame: Vec<u8> = hex
            .as_bytes()
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        sdrmm_dsp::mode_s_append_parity(&mut frame);
        frame
    }

    fn only(messages: Vec<AdsbMessage>) -> AdsbMessage {
        assert_eq!(messages.len(), 1, "{messages:?}");
        messages.into_iter().next().unwrap()
    }

    /// The rule PLAN §18 wrote — "ADS-B needs the device at exactly 2 Msps" — cost the commonest
    /// ADS-B receiver there is: no RTL-SDR can produce 2.000 Msps, and its nearest rate is 2.048.
    /// The decoder runs at the radio's rate now, so these are the rates a real one offers — and
    /// the phases: a frame off the sample grid is what the air always sends, and the alignment
    /// this test's generator would otherwise share with the decoder's own windows.
    #[test]
    fn decodes_at_every_rate_and_phase_a_receiver_actually_offers() {
        for rate in [
            INPUT_RATE_HZ,
            2_048_000.0,
            2_400_000.0,
            2_560_000.0,
            MAX_INPUT_RATE_HZ,
        ] {
            for phase in [0.0f64, 0.21, 0.43, 0.5, 0.68, 0.9] {
                // Exactly 2 Msps near a half-sample offset is a blind spot of the rate, not
                // of the decoder: with one sample per half-chip, every band-limited sample
                // integrates half a pulse and half a gap and reads the same level whatever
                // the bits — there is nothing left to decode, which is dump1090's known 2.0
                // weakness too. The radios the rate range exists for (2.048 up) put a
                // different fraction of each chip on each sample, so they have no such phase.
                if rate == INPUT_RATE_HZ && (phase - 0.5).abs() < 0.2 {
                    continue;
                }
                let frames = [
                    squitter(0x3C_6444, me_identification("DLH123")),
                    squitter(0x3C_6444, me_airborne_position(38_000, LAT, LON, false)),
                ];
                let iq = transmission_at_phase(&frames, GAP_US, LEVEL, rate, phase);
                let mut chan = AdsbChannel::new(
                    ChannelCtx { input_rate: rate },
                    adsb_params(AdsbParams::default()),
                )
                .expect("channel");
                let messages = feed(&mut chan, &iq, &[4_096]);
                let calls: Vec<_> = messages.iter().filter_map(|m| m.callsign.clone()).collect();
                assert_eq!(
                    calls,
                    vec!["DLH123"],
                    "at {rate} Hz phase {phase}: {messages:?}"
                );
                assert!(
                    messages.iter().any(|m| m.altitude_ft == Some(38_000)),
                    "at {rate} Hz phase {phase}: {messages:?}"
                );
            }
        }
    }

    /// Off the sample grid *and* with noise under it is what an RTL-SDR at 2.048 Msps
    /// actually hands this decoder — the condition the field reported as an empty map while
    /// the grid-aligned tests stayed green.
    #[test]
    fn noisy_off_grid_frames_decode_at_the_rtl_rate() {
        for (index, phase) in [0.05, 0.19, 0.33, 0.47, 0.61, 0.75, 0.89]
            .into_iter()
            .enumerate()
        {
            let frames = [squitter(0x3C_6444, me_identification("DLH123"))];
            let mut iq = transmission_at_phase(&frames, GAP_US, LEVEL, 2_048_000.0, phase);
            add_noise(&mut iq, 0xADB0 + index as u32, 0.01);
            let mut chan = AdsbChannel::new(
                ChannelCtx {
                    input_rate: 2_048_000.0,
                },
                adsb_params(AdsbParams::default()),
            )
            .expect("channel");
            let msg = only(feed(&mut chan, &iq, &[4_096]));
            assert_eq!(msg.callsign.as_deref(), Some("DLH123"), "phase {phase}");
        }
    }

    /// The range is the contract, and a rate outside it is refused rather than decoded badly.
    #[test]
    fn refuses_a_rate_the_slicer_cannot_work_at() {
        for rate in [1_000_000.0, INPUT_RATE_HZ - 1.0, MAX_INPUT_RATE_HZ + 1.0] {
            assert!(
                AdsbChannel::new(
                    ChannelCtx { input_rate: rate },
                    adsb_params(AdsbParams::default())
                )
                .is_err(),
                "{rate} Hz must be refused"
            );
        }
    }

    #[test]
    fn identification_round_trips_through_the_air() {
        let frame = squitter(0x3C_6444, me_identification("DLH123"));
        let msg = only(decode(AdsbParams::default(), std::slice::from_ref(&frame)));
        assert_eq!(msg.df, 17);
        assert_eq!(msg.icao, "3C6444");
        assert_eq!(msg.type_code, Some(4));
        assert_eq!(msg.callsign.as_deref(), Some("DLH123"));
        assert_eq!(msg.raw, hex_upper(&frame));
        assert_eq!(msg.altitude_ft, None);
        assert_eq!(msg.lat, None);
    }

    /// The identification squitter every Mode S text quotes: ICAO 4840D6, callsign KLM1023.
    #[test]
    fn published_identification_frame_decodes() {
        let msg = only(decode(
            AdsbParams::default(),
            &[published("8D4840D6202CC371C32CE0")],
        ));
        assert_eq!(msg.icao, "4840D6");
        assert_eq!(msg.callsign.as_deref(), Some("KLM1023"));
    }

    /// The published even/odd pair for ICAO 40621D at 38 000 ft. A global solution is
    /// reported at the position of the *later* frame, and the literature quotes the pair
    /// with the even frame last: 52.25720 N, 3.91937 E. The aircraft moved ~1 km between the
    /// two transmissions, so the odd-last solution is a different (and equally correct) point.
    #[test]
    fn published_position_pair_solves_globally() {
        let even = published("8D40621D58C382D690C8AC");
        let odd = published("8D40621D58C386435CC412");

        let msgs = decode(AdsbParams::default(), &[odd.clone(), even.clone()]);
        assert_eq!(msgs.len(), 2);
        let [first, last] = &msgs[..] else {
            unreachable!()
        };
        assert_eq!(first.icao, "40621D");
        assert_eq!(first.type_code, Some(11));
        assert_eq!(first.altitude_ft, Some(38_000));
        // A single frame with no reference position cannot be placed.
        assert_eq!(first.lat, None);
        let (lat, lon) = (last.lat.unwrap(), last.lon.unwrap());
        assert!((lat - 52.257_202).abs() < 1e-4, "lat {lat}");
        assert!((lon - 3.919_37).abs() < 1e-4, "lon {lon}");

        // Same pair the other way round: the odd frame's own position, ~1 km further east.
        let msgs = decode(AdsbParams::default(), &[even, odd]);
        let solved = msgs.last().unwrap();
        assert!((solved.lat.unwrap() - 52.265_78).abs() < 1e-4, "{solved:?}");
        assert!((solved.lon.unwrap() - 3.938_91).abs() < 1e-3, "{solved:?}");
    }

    #[test]
    fn airborne_position_pair_solves_to_the_encoded_point() {
        let icao = 0x3C_6444;
        let msgs = decode(
            AdsbParams::default(),
            &[
                squitter(icao, me_airborne_position(36_000, LAT, LON, false)),
                squitter(icao, me_airborne_position(36_000, LAT, LON, true)),
            ],
        );
        assert_eq!(msgs.len(), 2);
        let solved = msgs.last().unwrap();
        assert_eq!(solved.altitude_ft, Some(36_000));
        assert_eq!(solved.on_ground, Some(false));
        let (lat, lon) = (solved.lat.unwrap(), solved.lon.unwrap());
        assert!((lat - LAT).abs() < 0.01, "lat {lat}");
        assert!((lon - LON).abs() < 0.01, "lon {lon}");
    }

    #[test]
    fn a_single_frame_is_placed_against_a_reference_position() {
        for odd in [false, true] {
            let msg = only(decode(
                AdsbParams {
                    ref_lat: Some(LAT - 0.4),
                    ref_lon: Some(LON + 0.6),
                    ..AdsbParams::default()
                },
                &[squitter(
                    0x3C_6444,
                    me_airborne_position(9_000, LAT, LON, odd),
                )],
            ));
            let (lat, lon) = (msg.lat.unwrap(), msg.lon.unwrap());
            assert!((lat - LAT).abs() < 0.01, "odd {odd}: lat {lat}");
            assert!((lon - LON).abs() < 0.01, "odd {odd}: lon {lon}");
        }
    }

    /// Southern/western coordinates are where a `%`-based CPR gets the sign wrong.
    #[test]
    fn positions_south_and_west_of_the_meridian_solve() {
        let icao = 0xE8_0000;
        let (lat, lon) = (-33.868_2, -70.652_7);
        let msgs = decode(
            AdsbParams::default(),
            &[
                squitter(icao, me_airborne_position(12_000, lat, lon, false)),
                squitter(icao, me_airborne_position(12_000, lat, lon, true)),
            ],
        );
        let solved = msgs.last().unwrap();
        assert!((solved.lat.unwrap() - lat).abs() < 0.01, "{solved:?}");
        assert!((solved.lon.unwrap() - lon).abs() < 0.01, "{solved:?}");
    }

    #[test]
    fn surface_position_needs_a_reference_and_reports_on_ground() {
        let frame = squitter(0x3C_6444, me_surface_position(LAT, LON, false));
        let without = only(decode(AdsbParams::default(), std::slice::from_ref(&frame)));
        assert_eq!(without.on_ground, Some(true));
        assert_eq!(without.altitude_ft, None);
        assert_eq!(without.lat, None);

        let with = only(decode(
            AdsbParams {
                ref_lat: Some(LAT + 0.1),
                ref_lon: Some(LON - 0.1),
                ..AdsbParams::default()
            },
            &[frame],
        ));
        assert!((with.lat.unwrap() - LAT).abs() < 0.01, "{with:?}");
        assert!((with.lon.unwrap() - LON).abs() < 0.01, "{with:?}");
    }

    #[test]
    fn velocity_round_trips_speed_track_and_climb() {
        for (speed, track, climb) in [
            (420.0, 250.0, -1_408),
            (180.0, 5.0, 2_048),
            (500.0, 91.0, 0),
            (300.0, 359.5, -64),
        ] {
            let msg = only(decode(
                AdsbParams::default(),
                &[squitter(0x3C_6444, me_velocity(speed, track, climb))],
            ));
            assert_eq!(msg.type_code, Some(19));
            let got_speed = msg.ground_speed_kt.unwrap();
            let got_track = msg.track_deg.unwrap();
            assert!((got_speed - speed).abs() < 1.5, "speed {got_speed}");
            let error = (got_track - track + 540.0).rem_euclid(360.0) - 180.0;
            assert!(error.abs() < 0.5, "track {got_track}");
            assert_eq!(msg.vertical_rate_fpm, Some(climb));
        }
    }

    /// The Q bit switches the altitude encoding at 50 175 ft: 25 ft steps below, Gillham
    /// coded 100 ft steps above.
    #[test]
    fn altitude_decodes_on_both_sides_of_the_q_bit_boundary() {
        for altitude in [
            -1_000, 0, 725, 36_000, 50_175, 50_200, 51_000, 62_000, 80_000,
        ] {
            let msg = only(decode(
                AdsbParams::default(),
                &[squitter(
                    0x3C_6444,
                    me_airborne_position(altitude, LAT, LON, false),
                )],
            ));
            assert_eq!(msg.altitude_ft, Some(altitude), "altitude {altitude}");
        }
    }

    #[test]
    fn gnss_altitude_frames_use_the_same_ac12_encoding_as_barometric() {
        // The altitudes a GNSS-equipped airliner actually reports; a metre reading would
        // saturate its 12-bit field long before FL380.
        for alt_ft in [3_000, 38_000, 50_175] {
            let msg = only(decode(
                AdsbParams::default(),
                &[squitter(
                    0x3C_6444,
                    me_airborne_position_gnss(alt_ft, LAT, LON, false),
                )],
            ));
            assert_eq!(msg.type_code, Some(21));
            assert_eq!(msg.altitude_ft, Some(alt_ft), "{alt_ft} ft");
        }
    }

    /// AC12 == 0 is "altitude not available" for every airborne position frame, GNSS or not.
    #[test]
    fn absent_altitude_is_none_on_both_altitude_paths() {
        for tc in [11u64, 21] {
            let msg = only(decode(
                AdsbParams::default(),
                &[squitter(0x3C_6444, position_me_raw(tc, 0, LAT, LON, false))],
            ));
            assert_eq!(msg.altitude_ft, None, "tc {tc}");
        }
    }

    fn flip(frame: &mut [u8], bit: usize) {
        frame[bit / 8] ^= 0x80 >> (bit % 8);
    }

    #[test]
    fn crc_fix_repairs_a_single_bit_error() {
        let clean = squitter(0x3C_6444, me_identification("DLH123"));
        let mut damaged = clean.clone();
        flip(&mut damaged, 63);

        let msg = only(decode(AdsbParams::default(), &[damaged.clone()]));
        assert_eq!(msg.callsign.as_deref(), Some("DLH123"));
        assert_eq!(msg.raw, hex_upper(&clean));

        assert!(
            decode(
                AdsbParams {
                    crc_fix: false,
                    ..AdsbParams::default()
                },
                &[damaged]
            )
            .is_empty(),
            "crc_fix off must not repair anything"
        );
    }

    #[test]
    fn a_two_bit_error_is_dropped() {
        let mut damaged = squitter(0x3C_6444, me_identification("DLH123"));
        flip(&mut damaged, 40);
        flip(&mut damaged, 77);
        assert!(decode(AdsbParams::default(), &[damaged]).is_empty());
    }

    #[test]
    fn noise_alone_produces_no_frames() {
        let mut iq = vec![Complex::new(0.0f32, 0.0); 4_000_000];
        add_noise(&mut iq, 0x5EED_1234, 1.0);
        assert!(feed(&mut channel(AdsbParams::default()), &iq, &[65_536]).is_empty());
    }

    /// The accept threshold is derived from the pulses, so a frame 34 dB weaker than the one
    /// every other test uses must decode just as well — with noise under it.
    #[test]
    fn a_weak_frame_buried_in_noise_still_decodes() {
        let frame = squitter(0x3C_6444, me_identification("DLH123"));
        let mut iq = transmission(&[frame], GAP_US, 0.01, INPUT_RATE_HZ);
        add_noise(&mut iq, 0x00C0_FFEE, 0.001);
        let msg = only(feed(&mut channel(AdsbParams::default()), &iq, &[1_024]));
        assert_eq!(msg.callsign.as_deref(), Some("DLH123"));
    }

    #[test]
    fn ragged_blocks_decode_exactly_like_one_block() {
        let icao = 0x3C_6444;
        let frames = [
            squitter(icao, me_identification("DLH123")),
            squitter(icao, me_airborne_position(36_000, LAT, LON, false)),
            squitter(icao, me_velocity(420.0, 250.0, -1_408)),
            squitter(icao, me_airborne_position(36_000, LAT, LON, true)),
        ];
        let iq = transmission(&frames, GAP_US, LEVEL, INPUT_RATE_HZ);

        let whole = feed(&mut channel(AdsbParams::default()), &iq, &[iq.len()]);
        assert_eq!(whole.len(), 4);
        let ragged = feed(
            &mut channel(AdsbParams::default()),
            &iq,
            &[997, 1, 4_096, 65, 239, 7, 1_024],
        );
        assert_eq!(whole, ragged);
    }

    #[test]
    fn a_frame_split_across_two_calls_is_still_decoded() {
        let iq = transmission(
            &[squitter(0x3C_6444, me_identification("DLH123"))],
            GAP_US,
            LEVEL,
            INPUT_RATE_HZ,
        );
        // Cut inside the frame: the preamble is in the first block, most bits in the second.
        let cut = (GAP_US * 2.0) as usize + 40;
        let mut chan = channel(AdsbParams::default());
        let mut out = ChannelOutputs::default();
        chan.process(&iq[..cut], &mut out);
        assert!(out.events.is_empty());
        chan.process(&iq[cut..], &mut out);
        assert_eq!(out.events.len(), 1);
    }

    #[test]
    fn the_cpr_cache_is_bounded() {
        let frames: Vec<Vec<u8>> = (0..CPR_CACHE_LEN as u32 * 3)
            .map(|n| squitter(0x40_0000 + n, me_airborne_position(30_000, LAT, LON, false)))
            .collect();
        let mut chan = channel(AdsbParams::default());
        let iq = transmission(&frames, GAP_US, LEVEL, INPUT_RATE_HZ);
        assert_eq!(feed(&mut chan, &iq, &[8_192]).len(), frames.len());
        assert_eq!(chan.cpr.len(), CPR_CACHE_LEN);
    }

    /// An even/odd pair that arrived minutes apart describes two different places; the
    /// aircraft has flown between them, so the pair must not be solved.
    #[test]
    fn a_stale_even_odd_pair_is_not_paired() {
        let mut chan = channel(AdsbParams::default());
        // The window is ten seconds of *this radio's* samples, so the test asks the channel
        // rather than a constant — at 2.4 Msps the same ten seconds is a different number.
        let stale = chan.cpr_pair_max_age;
        let even = published("8D40621D58C382D690C8AC");
        let odd = published("8D40621D58C386435CC412");
        let icao = 0x40_621D;
        assert!(chan.message(&even, 17, icao, 0).lat.is_none());
        assert!(chan.message(&odd, 17, icao, stale + 1).lat.is_none());
        // Fresh again once a new even frame arrives.
        assert!(chan.message(&even, 17, icao, stale + 2).lat.is_some());
        assert_eq!(chan.cpr.first().map(|a| a.icao), Some(0x40_621D));
    }

    // ── Mode S beyond the extended squitter ────────────────────────────────────────────────

    /// A proving frame, so the roll-call replies under test have an address to be attributed
    /// to. Everything after it in the same transmission is inside the vouching window.
    fn proof(icao: u32) -> Vec<u8> {
        squitter(icao, me_identification("DLH123"))
    }

    /// A short frame is only scanned once a long frame's worth of samples sits behind it (see
    /// [`AdsbChannel::process`]), so a transmission ending on one leaves that much room —
    /// otherwise "decoded nothing" would mean "the scan never got there".
    fn decode_replies(p: AdsbParams, frames: &[Vec<u8>]) -> Vec<AdsbMessage> {
        let iq = transmission(frames, 300.0, LEVEL, INPUT_RATE_HZ);
        feed(&mut channel(p), &iq, &[4_096])
    }

    /// An all-call reply is self-proving — the address rides in the clear — so it needs no
    /// help, and it is the only thing a Mode S aircraft that never squitters volunteers.
    #[test]
    fn an_all_call_reply_reports_the_aircraft_and_its_air_ground_state() {
        for (capability, on_ground) in [(4, Some(true)), (5, Some(false)), (6, None), (0, None)] {
            let msg = only(decode_replies(
                AdsbParams::default(),
                &[all_call_reply(0x3C_6444, capability, 0)],
            ));
            assert_eq!(msg.df, 11);
            assert_eq!(msg.icao, "3C6444");
            assert_eq!(msg.on_ground, on_ground, "capability {capability}");
            // DF11 has no ME field, so claiming a type code would be reading the address as one.
            assert_eq!(msg.type_code, None);
        }
    }

    /// The interrogator identifier is keyed onto an all-call reply's parity, so the bits it
    /// occupies are not evidence — but everything above them still is.
    #[test]
    fn an_all_call_reply_survives_its_interrogator_identifier() {
        for interrogator in [0, 1, 15, ALL_CALL_PI_MAX] {
            let msg = only(decode_replies(
                AdsbParams::default(),
                &[all_call_reply(0x40_621D, 5, interrogator)],
            ));
            assert_eq!(msg.icao, "40621D", "interrogator {interrogator}");
        }
        // Past the field the standard defines, the frame is indistinguishable from noise that
        // happened to slice into 56 bits.
        assert!(
            decode_replies(
                AdsbParams::default(),
                &[all_call_reply(0x40_621D, 5, ALL_CALL_PI_MAX + 1)]
            )
            .is_empty()
        );
    }

    /// The rule the whole roll-call path rests on. A DF4/5/20/21 reply carries its address
    /// nowhere but on the parity, so *every* one of them checks out against *some* address —
    /// decoding one unvouched would put an aircraft on the map that was never on the air.
    #[test]
    fn a_roll_call_reply_is_dropped_until_something_proves_the_address() {
        let icao = 0x3C_6444;
        let reply = altitude_reply(icao, 36_000, 0);

        assert!(
            decode_replies(AdsbParams::default(), std::slice::from_ref(&reply)).is_empty(),
            "an unvouched reply must not become an aircraft"
        );

        let msgs = decode_replies(AdsbParams::default(), &[proof(icao), reply]);
        assert_eq!(msgs.len(), 2, "{msgs:?}");
        assert_eq!(msgs[1].df, 4);
        assert_eq!(msgs[1].icao, "3C6444");
        assert_eq!(msgs[1].altitude_ft, Some(36_000));
    }

    /// Proof expires: an address last seen in the clear a minute ago no longer vouches for a
    /// reply that nothing else can attribute.
    #[test]
    fn proof_of_an_address_goes_stale() {
        let mut chan = channel(AdsbParams::default());
        let window = chan.roll_call_max_age;
        chan.observe(0x3C_6444, 0, true);
        assert!(chan.vouched(0x3C_6444, window));
        assert!(!chan.vouched(0x3C_6444, window + 1));

        // A reply refreshes the cache entry but not the proof — otherwise one lucky parity
        // match would keep an address alive on the strength of frames that prove nothing.
        chan.observe(0x3C_6444, window, false);
        assert!(!chan.vouched(0x3C_6444, window + 1));
    }

    /// Noise cannot become an aircraft, and once one *is* on the whitelist it must not become
    /// that aircraft's replies either: every 24-bit value is a legal roll-call address, so all
    /// that stands between noise and a fabricated altitude is the preamble gate and the chance
    /// of the recovered address being the one address that was proved.
    #[test]
    fn noise_does_not_become_replies_from_an_aircraft_already_known() {
        let mut chan = channel(AdsbParams::default());
        chan.observe(0x3C_6444, 0, true);
        let mut iq = vec![Complex::new(0.0f32, 0.0); 4_000_000];
        add_noise(&mut iq, 0x5EED_4321, 1.0);
        assert!(feed(&mut chan, &iq, &[65_536]).is_empty());
    }

    /// Squawk is four octal digits interleaved bit by bit across the field; check the whole
    /// code space rather than the three emergency codes everyone remembers.
    #[test]
    fn every_squawk_round_trips_through_an_identity_reply() {
        let icao = 0x3C_6444;
        let mut chan = channel(AdsbParams::default());
        chan.observe(icao, 0, true);
        for code in 0..4_096u32 {
            let text = format!(
                "{}{}{}{}",
                code >> 9 & 7,
                code >> 6 & 7,
                code >> 3 & 7,
                code & 7
            );
            let frame = identity_reply(icao, &text, 0);
            let msg = chan.message(&frame, 5, icao, 0);
            assert_eq!(msg.squawk.as_deref(), Some(text.as_str()));
            assert_eq!(
                msg.altitude_ft, None,
                "an identity reply carries no altitude"
            );
        }
    }

    /// The AC13 field is the squitter's AC12 with an M bit wedged into it, so the same Q-bit
    /// and Gillham paths have to come out of a different bit layout.
    #[test]
    fn a_surveillance_altitude_reply_decodes_on_both_sides_of_the_q_bit() {
        let icao = 0x3C_6444;
        let mut chan = channel(AdsbParams::default());
        chan.observe(icao, 0, true);
        for alt_ft in [-1_000, 0, 725, 36_000, 50_175, 50_200, 62_000] {
            let frame = altitude_reply(icao, alt_ft, 0);
            assert_eq!(
                chan.message(&frame, 4, icao, 0).altitude_ft,
                Some(alt_ft),
                "{alt_ft} ft"
            );
        }
        // An all-zero field is "no altitude information", not sea level.
        assert_eq!(surveillance_altitude(0), None);
        // M set selects metric units, whose encoding the standard leaves undefined.
        assert_eq!(surveillance_altitude(0x0040 | 0x0010 | 0x0020), None);
    }

    /// Flight status is the only air/ground answer a surveillance reply carries, and two of
    /// its codes report the ident pulse instead of answering at all.
    #[test]
    fn flight_status_answers_air_ground_only_when_it_says_so() {
        let icao = 0x3C_6444;
        let mut chan = channel(AdsbParams::default());
        chan.observe(icao, 0, true);
        for (fs, on_ground) in [
            (0, Some(false)),
            (1, Some(true)),
            (2, Some(false)),
            (3, Some(true)),
            (4, None),
            (5, None),
        ] {
            let frame = altitude_reply(icao, 5_000, fs);
            assert_eq!(
                chan.message(&frame, 4, icao, 0).on_ground,
                on_ground,
                "flight status {fs}"
            );
        }
    }

    /// Comm-B replies carry the surveillance field *and* 56 bits of register. BDS 2,0 is the
    /// one worth reading: it is where an aircraft with no extended squitter puts its callsign.
    #[test]
    fn comm_b_replies_carry_the_surveillance_field_and_a_bds20_callsign() {
        let icao = 0x40_621D;
        let msgs = decode(
            AdsbParams::default(),
            &[
                proof(icao),
                comm_b_altitude_reply(icao, 24_000, 0, mb_identification("KLM1023")),
                comm_b_identity_reply(icao, "7421", 1, mb_identification("KLM1023")),
            ],
        );
        assert_eq!(msgs.len(), 3, "{msgs:?}");
        let [_, altitude, identity] = &msgs[..] else {
            unreachable!()
        };
        assert_eq!(altitude.df, 20);
        assert_eq!(altitude.altitude_ft, Some(24_000));
        assert_eq!(altitude.callsign.as_deref(), Some("KLM1023"));
        assert_eq!(altitude.on_ground, Some(false));
        assert_eq!(identity.df, 21);
        assert_eq!(identity.squawk.as_deref(), Some("7421"));
        assert_eq!(identity.callsign.as_deref(), Some("KLM1023"));
        assert_eq!(identity.on_ground, Some(true));
    }

    /// A Comm-B register does not say which register it is, so reading one as BDS 2,0 is a
    /// guess — and a guess that would otherwise turn any register into a plausible callsign.
    #[test]
    fn a_register_that_is_not_bds20_yields_no_callsign() {
        let icao = 0x3C_6444;
        let mut chan = channel(AdsbParams::default());
        chan.observe(icao, 0, true);

        // BDS 4,0 (selected vertical intention): a different code entirely.
        let mut other = mb_identification("KLM1023");
        other[0] = 0x40;
        assert_eq!(
            chan.message(&other_reply(icao, other), 20, icao, 0)
                .callsign,
            None
        );

        // The right code over characters the charset leaves undefined — the `#` the decoder
        // must refuse rather than print.
        let mut undefined = [0u8; 7];
        undefined[0] = 0x20;
        undefined[1] = 0xFF;
        assert_eq!(
            chan.message(&other_reply(icao, undefined), 20, icao, 0)
                .callsign,
            None
        );
    }

    fn other_reply(icao: u32, mb: [u8; 7]) -> Vec<u8> {
        comm_b_altitude_reply(icao, 10_000, 0, mb)
    }

    /// The NL table is 58 hand-typed constants; check every one of them against the closed
    /// form it tabulates (DO-260B Appendix A), away from the boundaries themselves.
    #[test]
    fn the_nl_table_matches_its_closed_form() {
        let closed_form = |lat: f64| -> i32 {
            let nz = 15.0;
            let a = 1.0 - (1.0 - (PI / (2.0 * nz)).cos()) / (PI * lat / 180.0).cos().powi(2);
            (TAU / a.acos()).floor() as i32
        };
        let mut lat = 0.0;
        while lat < 86.5 {
            let near_boundary = NL_BOUNDARIES.iter().any(|b| (b - lat).abs() < 1e-4);
            if !near_boundary {
                assert_eq!(cpr_nl(lat), closed_form(lat), "lat {lat}");
                assert_eq!(cpr_nl(-lat), closed_form(lat), "lat -{lat}");
            }
            lat += 0.013;
        }
        assert_eq!(cpr_nl(88.0), 1);
        assert_eq!(cpr_nl(-89.9), 1);
    }

    #[test]
    fn mismatched_params_variant_is_rejected() {
        let mut chan = channel(AdsbParams::default());
        let err = chan.apply(settings(ChannelParams::Nfm(NfmParams::default())));
        assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
        let built = AdsbChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(ChannelParams::Nfm(NfmParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }

    #[test]
    fn wrong_input_rate_is_rejected() {
        let built = AdsbChannel::new(
            ChannelCtx {
                input_rate: 48_000.0,
            },
            adsb_params(AdsbParams::default()),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }

    #[test]
    fn an_incomplete_or_out_of_range_reference_is_rejected() {
        for bad in [
            AdsbParams {
                ref_lat: Some(91.0),
                ref_lon: Some(0.0),
                ..AdsbParams::default()
            },
            AdsbParams {
                ref_lat: Some(0.0),
                ref_lon: Some(181.0),
                ..AdsbParams::default()
            },
            AdsbParams {
                ref_lat: Some(0.0),
                ref_lon: None,
                ..AdsbParams::default()
            },
            AdsbParams {
                ref_lat: Some(f64::NAN),
                ref_lon: Some(0.0),
                ..AdsbParams::default()
            },
        ] {
            let built = AdsbChannel::new(
                ChannelCtx {
                    input_rate: INPUT_RATE_HZ,
                },
                adsb_params(bad.clone()),
            );
            assert!(
                matches!(built, Err(ChannelError::InvalidSettings(_))),
                "{bad:?} must be rejected"
            );
        }
    }
}
