//! The CPM parameter space ( §3.1, §3.3): everything that distinguishes one CPM/FSK
//! entry from another is *data* held here — the symbol→level table, the modulation index, the
//! frequency pulse, the oversampling. A `match` on a standard anywhere downstream of this
//! struct is a defect; the five phase-3 probes (POCSAG, DMR, D-STAR, AIS, APRS) differ only in
//! the values they put in these fields.

use crate::soft::SoftBit;

/// The symbol→frequency-level table: `levels[i]` is the frequency multiple symbol index `i`
/// transmits, in the alphabet's own units (conventionally the odd integers, ±1 for M = 2,
/// ±1/±3 for M = 4, …). The *order* is the standard's bit-to-symbol assignment and is supplied
/// by the caller — DMR maps dibit 01 to +3, 00 to +1, 10 to −1, 11 to −3
/// (ETSI TS 102 361-1 §4.2.2), which is neither the natural nor the Gray order, so no built-in
/// ordering can be assumed; [`Mapping::natural`] and [`Mapping::gray`] are offered as plain
/// constructors for entries that do use them.
///
/// M must be a power of two: each symbol carries exactly `log2(M)` bits, and the per-bit
/// demapper ([`Mapping::soft_bits`]) reads bit `k` of the symbol *index* — which is what makes
/// the bit-to-level assignment data instead of code.
#[derive(Clone, Debug, PartialEq)]
pub struct Mapping {
    levels: Vec<f32>,
    /// Levels in ascending order, for the nearest-level slicer.
    sorted: Vec<(f32, u8)>,
    max_level: f32,
    min_spacing: f32,
}

impl Mapping {
    /// A caller-supplied table. See the type docs for the conventions.
    ///
    /// # Panics
    /// If M is not a power of two of at least 2, or any level is non-finite, or two levels
    /// coincide (a slicer cannot tell them apart, so the table cannot mean anything).
    #[must_use]
    pub fn new(levels: Vec<f32>) -> Self {
        let m = levels.len();
        assert!(
            m >= 2 && m.is_power_of_two() && m <= 256,
            "M must be a power of two in 2..=256, got {m}"
        );
        assert!(
            levels.iter().all(|l| l.is_finite()),
            "levels must be finite"
        );
        let mut sorted: Vec<(f32, u8)> = levels
            .iter()
            .enumerate()
            .map(|(i, &l)| (l, i as u8))
            .collect();
        sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
        let min_spacing = sorted
            .windows(2)
            .map(|w| w[1].0 - w[0].0)
            .fold(f32::INFINITY, f32::min);
        assert!(min_spacing > 0.0, "levels must be distinct");
        let max_level = sorted.iter().map(|&(l, _)| l.abs()).fold(0.0f32, f32::max);
        assert!(max_level > 0.0, "at least one level must be nonzero");
        Self {
            levels,
            sorted,
            max_level,
            min_spacing,
        }
    }

    /// Natural binary order: index `i` transmits level `2i − (M−1)` — ascending odd integers,
    /// so index 0 is the most negative level and index M−1 the most positive.
    #[must_use]
    pub fn natural(m: usize) -> Self {
        Self::new(
            (0..m)
                .map(|i| (2 * i as i32 - (m as i32 - 1)) as f32)
                .collect(),
        )
    }

    /// Binary-reflected Gray order over the ascending odd-integer levels: adjacent levels
    /// differ in one bit of their symbol index, so a one-level slicing error costs one bit.
    #[must_use]
    pub fn gray(m: usize) -> Self {
        let mut levels = vec![0.0f32; m];
        for (rank, level) in (0..m)
            .map(|i| (2 * i as i32 - (m as i32 - 1)) as f32)
            .enumerate()
        {
            levels[rank ^ (rank >> 1)] = level;
        }
        Self::new(levels)
    }

    #[must_use]
    pub fn m(&self) -> usize {
        self.levels.len()
    }

    #[must_use]
    pub fn bits_per_symbol(&self) -> u32 {
        self.levels.len().trailing_zeros()
    }

    #[must_use]
    pub fn levels(&self) -> &[f32] {
        &self.levels
    }

    /// The level symbol index `index` transmits; indices are taken modulo M, mirroring
    /// `fsk4::level`'s masking, so a sliced index is always valid to look up.
    #[must_use]
    pub fn level(&self, index: u8) -> f32 {
        self.levels[index as usize & (self.levels.len() - 1)]
    }

    /// Largest |level| — the scale the demodulator's peak estimate normalises to.
    #[must_use]
    pub fn max_level(&self) -> f32 {
        self.max_level
    }

    /// Smallest gap between adjacent levels — the scale slicing margins and the known-symbol
    /// hook's plausibility bounds are measured against.
    #[must_use]
    pub fn min_spacing(&self) -> f32 {
        self.min_spacing
    }

    /// Nearest-level hard decision: the symbol index whose level is closest to `y`. For the
    /// DMR table this reproduces `fsk4::slice`'s thresholds at ±2 and 0 exactly.
    #[must_use]
    pub fn slice(&self, y: f32) -> u8 {
        // Binary search on the sorted levels, then compare the two bracketing neighbours —
        // O(log M) with no allocation, and total_cmp keeps a NaN input from panicking (it
        // sorts above every level and slices to the top one, which the demodulator's
        // finiteness guard makes unreachable anyway).
        let at = self.sorted.partition_point(|&(l, _)| l < y);
        match (self.sorted.get(at.wrapping_sub(1)), self.sorted.get(at)) {
            (Some(&(lo, i)), Some(&(hi, j))) => {
                if y - lo <= hi - y {
                    i
                } else {
                    j
                }
            }
            (Some(&(_, i)), None) | (None, Some(&(_, i))) => i,
            // Unreachable: construction guarantees at least two levels.
            (None, None) => 0,
        }
    }

    /// Per-bit soft decisions for a normalised soft symbol, appended MSB-first — the bit order
    /// `SymbolWindow::bits` reads dibits in. Max-log over the level table: for bit `k`, the
    /// squared distance to the nearest level whose index has bit `k` clear, minus the same for
    /// bit `k` set — positive means 1 (crate convention).
    ///
    /// Scale: half the level spacing of boundary distance reads as ±0.5 and a full spacing
    /// saturates at ±1.0 — the `fsk4::soft_bits` calibration kept (a clean inner symbol's
    /// outer-bit reads ±0.5 there too), so an over-deviated burst cannot out-vote the rest of
    /// a frame.
    pub fn soft_bits(&self, y: f32, out: &mut Vec<SoftBit>) {
        let scale = 0.5 / (self.min_spacing * self.min_spacing);
        for k in (0..self.bits_per_symbol()).rev() {
            let (mut d0, mut d1) = (f32::INFINITY, f32::INFINITY);
            for (i, &level) in self.levels.iter().enumerate() {
                let d = (y - level) * (y - level);
                if i >> k & 1 == 0 {
                    d0 = d0.min(d);
                } else {
                    d1 = d1.min(d);
                }
            }
            out.push(SoftBit(((d0 - d1) * scale).clamp(-1.0, 1.0)));
        }
    }
}

/// One CPM/CPFSK waveform, fully described by data (§3.3): the mapping table, the modulation
/// index in its one canonical form, the frequency pulse, and the oversampling. [`CpmMod`] and
/// [`CpmDemod`] are both constructed from this, which is what keeps a transmitter and its
/// receiver describing the *same* waveform.
///
/// **The canonical index is h.** A caller may know the waveform by h directly (GMSK's ½, MSK's
/// ½) or by its deviation set (DMR's ±1944 Hz at 4800 baud); [`CpmParams::from_deviation`]
/// converts at construction and nothing downstream ever sees a Hz figure again. With the
/// conventional odd-integer levels, the outermost deviation `d` of an M-level set gives
/// `h = 2·d / ((M−1)·baud)` — implemented as `2·d / (max_level·baud)`, the same number for the
/// conventional table and still meaningful for any other.
///
/// **The frequency pulse is unit-area** ([`pulse::Norm::Area`](crate::pulse::Norm)): its
/// integral fixes the phase pulse at q(∞) = ½ and the per-symbol phase step at π·h — asserted
/// here, because an Energy-normalised pulse would silently rescale h by the ratio of the two
/// norms and no downstream test would localise the decibels to this one argument.
///
/// [`CpmMod`]: super::CpmMod
/// [`CpmDemod`]: super::CpmDemod
#[derive(Clone, Debug)]
pub struct CpmParams {
    mapping: Mapping,
    h: f64,
    freq_pulse: Vec<f32>,
    sps: f64,
}

impl CpmParams {
    /// From the modulation index directly.
    ///
    /// # Panics
    /// If `h` is not positive and finite, `sps` is below 2 (the timing recovery's floor), the
    /// outer deviation would exceed Nyquist (`h·max_level ≥ sps`), or `freq_pulse` is empty or
    /// not unit-area.
    #[must_use]
    pub fn from_h(mapping: Mapping, h: f64, freq_pulse: Vec<f32>, sps: f64) -> Self {
        assert!(
            h.is_finite() && h > 0.0,
            "modulation index must be positive"
        );
        assert!(
            sps.is_finite() && sps >= 2.0,
            "need at least two samples per symbol"
        );
        // h·L·baud/2 is the outer level's frequency; past rate/2 it aliases and no
        // discriminator can read it back.
        assert!(
            h * f64::from(mapping.max_level()) < sps,
            "outer deviation exceeds Nyquist: h·max_level = {} at {sps} samples/symbol",
            h * f64::from(mapping.max_level())
        );
        assert!(!freq_pulse.is_empty(), "frequency pulse must have taps");
        assert!(
            freq_pulse.iter().all(|t| t.is_finite()),
            "frequency pulse taps must be finite"
        );
        let area: f64 = freq_pulse.iter().map(|&t| f64::from(t)).sum();
        assert!(
            (area - 1.0).abs() < 1e-3,
            "frequency pulse must be unit-area (pulse::Norm::Area), got Σ = {area}"
        );
        Self {
            mapping,
            h,
            freq_pulse,
            sps,
        }
    }

    /// From a deviation set: `deviation_hz` is the outermost level's frequency offset (DMR's
    /// ±1944 Hz is the ±3 level, AIS's ±2400 Hz the ±1). Conversion:
    /// `h = 2·deviation_hz / (max_level·baud)` — for the conventional odd-integer table this
    /// is the textbook `2d/((M−1)·baud)`.
    ///
    /// # Panics
    /// As [`Self::from_h`], plus if `deviation_hz` or `baud` is not positive and finite.
    #[must_use]
    pub fn from_deviation(
        mapping: Mapping,
        deviation_hz: f64,
        baud: f64,
        freq_pulse: Vec<f32>,
        sps: f64,
    ) -> Self {
        assert!(
            deviation_hz.is_finite() && deviation_hz > 0.0,
            "deviation must be positive"
        );
        assert!(baud.is_finite() && baud > 0.0, "baud must be positive");
        let h = 2.0 * deviation_hz / (f64::from(mapping.max_level()) * baud);
        Self::from_h(mapping, h, freq_pulse, sps)
    }

    #[must_use]
    pub fn mapping(&self) -> &Mapping {
        &self.mapping
    }

    /// The canonical modulation index: a symbol at level L advances the carrier phase by
    /// π·h·L over its pulse.
    #[must_use]
    pub fn h(&self) -> f64 {
        self.h
    }

    #[must_use]
    pub fn freq_pulse(&self) -> &[f32] {
        &self.freq_pulse
    }

    #[must_use]
    pub fn sps(&self) -> f64 {
        self.sps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pulse::{self, Norm};

    fn dmr_mapping() -> Mapping {
        // ETSI TS 102 361-1 §4.2.2: dibit 00 → +1, 01 → +3, 10 → −1, 11 → −3.
        Mapping::new(vec![1.0, 3.0, -1.0, -3.0])
    }

    #[test]
    fn the_dmr_table_reproduces_fsk4s_slicer() {
        let map = dmr_mapping();
        for (y, want) in [
            (2.5, 0b01),
            (3.5, 0b01),
            (1.9, 0b00),
            (0.1, 0b00),
            (-0.1, 0b10),
            (-1.9, 0b10),
            (-2.1, 0b11),
            (-9.0, 0b11),
        ] {
            assert_eq!(map.slice(y), want, "slice({y})");
        }
    }

    #[test]
    fn soft_bits_match_the_fsk4_calibration_on_the_dmr_table() {
        let map = dmr_mapping();
        let mut out = Vec::new();
        // Clean +3: sign bit fully 0 (negative-saturated), outer bit half-confident 1 —
        // exactly fsk4::soft_bits' numbers for the same symbol.
        map.soft_bits(3.0, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, -1.0);
        assert!((out[1].0 - 0.5).abs() < 1e-6);
        // Clean +1: both bits half-confident, sign 0 and inner 0.
        out.clear();
        map.soft_bits(1.0, &mut out);
        assert!((out[0].0 + 0.5).abs() < 1e-6);
        assert!((out[1].0 + 0.5).abs() < 1e-6);
        // A boundary symbol is an erasure on the bit the boundary decides.
        out.clear();
        map.soft_bits(2.0, &mut out);
        assert_eq!(out[1].0, 0.0);
        // Clean −3: sign bit fully 1.
        out.clear();
        map.soft_bits(-3.0, &mut out);
        assert_eq!(out[0].0, 1.0);
        assert!((out[1].0 - 0.5).abs() < 1e-6);
    }

    #[test]
    fn an_inverted_two_level_table_demaps_through_the_table_not_the_sign() {
        // Bell-202 through a discriminator: bit 1 (mark, 1200 Hz) sits BELOW the 1700 Hz
        // centre, so its level is −1 — the demap must follow the table, not assume
        // positive level means 1.
        let map = Mapping::new(vec![1.0, -1.0]);
        let mut out = Vec::new();
        map.soft_bits(-1.0, &mut out);
        assert!(out[0].0 > 0.0, "mark at level −1 must vote for bit 1");
        assert_eq!(map.slice(-1.0), 1);
        assert_eq!(map.slice(1.0), 0);
    }

    #[test]
    fn natural_and_gray_orders_lay_out_the_odd_integers() {
        let nat = Mapping::natural(4);
        assert_eq!(nat.levels(), &[-3.0, -1.0, 1.0, 3.0]);
        let gray = Mapping::gray(4);
        // Ascending levels visit indices 0, 1, 3, 2 — one bit flip per step.
        assert_eq!(gray.levels(), &[-3.0, -1.0, 3.0, 1.0]);
        let mut prev: Option<u8> = None;
        for &(_, i) in &gray.sorted {
            if let Some(p) = prev {
                assert_eq!((i ^ p).count_ones(), 1, "gray step {p} -> {i}");
            }
            prev = Some(i);
        }
        assert_eq!(Mapping::natural(2).levels(), &[-1.0, 1.0]);
    }

    #[test]
    fn eight_level_slicing_and_scale() {
        let map = Mapping::natural(8);
        assert_eq!(map.m(), 8);
        assert_eq!(map.bits_per_symbol(), 3);
        assert_eq!(map.max_level(), 7.0);
        assert_eq!(map.min_spacing(), 2.0);
        assert_eq!(map.slice(6.7), 7);
        assert_eq!(map.slice(-4.2), 1);
        let mut out = Vec::new();
        map.soft_bits(7.0, &mut out);
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|b| b.0 > 0.0), "clean +7 is index 0b111");
    }

    #[test]
    fn deviation_conversion_reproduces_the_documented_formula() {
        let p = CpmParams::from_deviation(
            dmr_mapping(),
            1_944.0,
            4_800.0,
            pulse::root_raised_cosine(10.0, 0.2, 8, Norm::Area),
            10.0,
        );
        // h = 2·1944/((4−1)·4800) = 0.27.
        assert!((p.h() - 0.27).abs() < 1e-12, "h = {}", p.h());
        // GMSK-style: AIS ±2400 Hz at 9600 baud, two levels → h = ½.
        let ais = CpmParams::from_deviation(
            Mapping::natural(2),
            2_400.0,
            9_600.0,
            pulse::gaussian_freq(5.0, 0.4, 3, Norm::Area),
            5.0,
        );
        assert!((ais.h() - 0.5).abs() < 1e-12);
    }

    #[test]
    #[should_panic(expected = "unit-area")]
    fn an_energy_normalised_pulse_is_rejected() {
        let _ = CpmParams::from_h(
            Mapping::natural(2),
            0.5,
            pulse::rect(8.0, Norm::Energy),
            8.0,
        );
    }

    #[test]
    #[should_panic(expected = "power of two")]
    fn a_three_level_table_is_rejected() {
        let _ = Mapping::new(vec![-1.0, 0.0, 1.0]);
    }

    #[test]
    #[should_panic(expected = "distinct")]
    fn coincident_levels_are_rejected() {
        let _ = Mapping::new(vec![1.0, 1.0]);
    }

    #[test]
    #[should_panic(expected = "Nyquist")]
    fn a_deviation_past_nyquist_is_rejected() {
        // h·max_level = 12 at 8 samples/symbol: the outer tone would alias.
        let _ = CpmParams::from_h(Mapping::natural(4), 4.0, pulse::rect(8.0, Norm::Area), 8.0);
    }
}
