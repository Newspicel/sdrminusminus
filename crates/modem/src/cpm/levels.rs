//! The known-symbol hook ( §3.4), CPM form: data-aided level and centre estimation
//! anchored to the known sequences every burst standard embeds — "positions i..j carry known
//! sequence S", generalising `sdrmm_dsp::fsk4::SyncLevels` from packed DMR dibits to any M and
//! any pattern length.
//!
//! The centre and peak loops in [`CpmDemod`](super::CpmDemod) learn from whatever the channel
//! carries, and most of what a burst carries is data: a payload whose symbol mean is not zero
//! drags the centre, and a keying-edge transient scales the peak. A known sequence is the one
//! stretch of a burst whose transmitted levels are known exactly, so it says what the levels
//! are with no data dependence at all — which is what a TDMA receiver should measure them from
//! (MMDVM does, in DMRDMORX.cpp/DMRSlotRX.cpp). Each detection fits
//! `measured = gain·transmitted + centre` by least squares; the correction applied is the
//! average over the last [`ANCHORS`] fits.
//!
//! The plausibility gates keep their measured judgments from `fsk4`, restated in the units
//! that made them work: the gain range is scale-free and carries over unchanged, and the
//! centre and misfit bounds — 1.0 in DMR's ±1/±3 units, where adjacent levels sit 2.0 apart —
//! are half the mapping's minimum level spacing, which reproduces the committed constants
//! exactly on the DMR table and means the same thing on any other.

use super::params::{CpmParams, Mapping};

/// Detections the estimate averages over — MMDVM's figure (DMRDMORX.cpp keeps four). One
/// fit carries whatever its own burst put on it: the centre the data mean dragged, the
/// transient of the keying edge. Four bursts' worth averages that away while still following
/// a fading transmitter.
const ANCHORS: usize = 4;

/// Fitted gains a detection is believed at. The symbols arrive already normalised by
/// [`CpmDemod`](super::CpmDemod), so a real transmitter's residual sits near one; a fit
/// outside two-to-one either way is a pattern that noise matched by chance, and folding it in
/// would mis-slice the very bursts the anchor exists to protect.
const GAIN_RANGE: std::ops::RangeInclusive<f32> = 0.5..=2.0;

/// Data-aided centre and gain correction, anchored to known-symbol detections. Until a
/// detection has anchored — and again once the last one has expired — the correction is the
/// identity, so acquisition still runs on the demodulator's own estimates.
pub struct KnownSymbols {
    levels: Vec<f32>,
    /// Largest fitted |centre| believed, in mapping-level units: half the level spacing. A
    /// centre past that would have sliced half the known symbols wrong — the match that led
    /// here could only have been chance.
    centre_bound: f32,
    /// Largest RMS misfit believed, same units and the same half-spacing: a sequence is
    /// matched by Hamming distance under a tolerance, which noise clears now and then — but
    /// noise that matched the pattern's indices still does not *fit* its levels, missing them
    /// by the better part of a level spacing, while any signal clean enough to have matched at
    /// all fits within a fraction of one.
    misfit_bound: f32,
    /// Symbols an anchored estimate survives without a fresh detection. Protocol timing, so
    /// caller data: the longest gap the entry's standard leaves between known sequences (DMR's
    /// 360 ms voice superframe is 1728 symbols; `fsk4` allowed 4800). Past it the channel is
    /// between transmissions, and the next transmitter must meet the demodulator's own
    /// estimates rather than the last transmitter's correction.
    timeout_symbols: u32,
    anchors: [(f32, f32); ANCHORS],
    count: usize,
    next: usize,
    centre: f32,
    gain: f32,
    since_anchor: u32,
}

impl KnownSymbols {
    /// The hook for an entry. Convenience over [`from_mapping`](Self::from_mapping) — the fit
    /// reads nothing but the symbol table.
    #[must_use]
    pub fn new(params: &CpmParams, timeout_symbols: u32) -> Self {
        Self::from_mapping(params.mapping(), timeout_symbols)
    }

    /// The hook for a symbol table alone. The waveform is irrelevant to a least-squares fit
    /// against known levels, so several entries that differ in baud, deviation and pulse but
    /// share one mapping — the six four-level digital-voice modes are exactly that — share one
    /// hook construction instead of nominating an arbitrary one of themselves.
    #[must_use]
    pub fn from_mapping(mapping: &Mapping, timeout_symbols: u32) -> Self {
        let half_spacing = mapping.min_spacing() / 2.0;
        Self {
            levels: mapping.levels().to_vec(),
            centre_bound: half_spacing,
            misfit_bound: half_spacing,
            timeout_symbols,
            anchors: [(0.0, 1.0); ANCHORS],
            count: 0,
            next: 0,
            centre: 0.0,
            gain: 1.0,
            since_anchor: 0,
        }
    }

    /// Account one recovered symbol against the anchor's lifetime; call once per symbol.
    pub fn tick(&mut self) {
        if self.count == 0 {
            return;
        }
        self.since_anchor += 1;
        if self.since_anchor >= self.timeout_symbols {
            self.reset();
        }
    }

    /// One detection: `pattern[i]` is the known symbol index transmitted where `measured[i]`
    /// was received — any consistent order, the fit only needs the pairing. An implausible fit
    /// is discarded rather than folded in (see the field docs for each gate).
    ///
    /// # Panics
    /// If the slices differ in length — the pairing *is* the measurement, so a mismatch is a
    /// caller bug, not a condition to guess through.
    pub fn anchor(&mut self, pattern: &[u8], measured: &[f32]) {
        assert_eq!(
            pattern.len(),
            measured.len(),
            "pattern and measured window must pair one-to-one"
        );
        let n = measured.len() as f32;
        let mask = self.levels.len() - 1;
        let (mut sx, mut sy, mut sxx, mut sxy) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for (&sym, &y) in pattern.iter().zip(measured) {
            let x = self.levels[sym as usize & mask];
            sx += x;
            sy += y;
            sxx += x * x;
            sxy += x * y;
        }
        let det = n * sxx - sx * sx;
        // A pattern of one repeated level fits any gain; no real sync is one. Scale-free form
        // of fsk4's absolute guard, so a table in unusual units changes nothing.
        if det <= 1e-6 * n * sxx {
            return;
        }
        let gain = (n * sxy - sx * sy) / det;
        let centre = (sy - gain * sx) / n;
        if !GAIN_RANGE.contains(&gain) || centre.abs() > self.centre_bound {
            return;
        }
        let misfit = pattern
            .iter()
            .zip(measured)
            .map(|(&sym, &y)| {
                let fit = gain * self.levels[sym as usize & mask] + centre;
                (y - fit) * (y - fit)
            })
            .sum::<f32>()
            / n;
        if misfit > self.misfit_bound * self.misfit_bound {
            return;
        }
        self.anchors[self.next] = (centre, gain);
        self.next = (self.next + 1) % ANCHORS;
        self.count = (self.count + 1).min(ANCHORS);
        self.since_anchor = 0;
        let averaged = &self.anchors[..self.count];
        let n = averaged.len() as f32;
        self.centre = averaged.iter().map(|&(c, _)| c).sum::<f32>() / n;
        self.gain = averaged.iter().map(|&(_, g)| g).sum::<f32>() / n;
    }

    /// The symbol re-scaled by what the last detections taught; the identity until one has.
    #[must_use]
    pub fn correct(&self, symbol: f32) -> f32 {
        (symbol - self.centre) / self.gain
    }

    /// Forget the anchors — the channel moved, and they describe the transmitter it left.
    pub fn reset(&mut self) {
        self.count = 0;
        self.next = 0;
        self.centre = 0.0;
        self.gain = 1.0;
        self.since_anchor = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::{super::params::Mapping, *};
    use crate::pulse::{self, Norm};

    const TIMEOUT: u32 = 4_800;

    fn dmr_params() -> CpmParams {
        CpmParams::from_deviation(
            Mapping::new(vec![1.0, 3.0, -1.0, -3.0]),
            1_944.0,
            4_800.0,
            pulse::root_raised_cosine(10.0, 0.2, 8, Norm::Area),
            10.0,
        )
    }

    /// The DMR BS voice sync's 24 dibits (ETSI TS 102 361-1 §9.1.1), oldest first.
    fn dmr_sync() -> Vec<u8> {
        let bits: u64 = 0x755F_D7DF_75F7;
        (0..24)
            .rev()
            .map(|i| (bits >> (2 * i)) as u8 & 0b11)
            .collect()
    }

    fn measured(pattern: &[u8], params: &CpmParams, gain: f32, centre: f32) -> Vec<f32> {
        pattern
            .iter()
            .map(|&s| gain * params.mapping().level(s) + centre)
            .collect()
    }

    /// The waveform reaches the fit through nothing but the table, so two entries that share a
    /// mapping and agree on nothing else must produce the identical correction.
    #[test]
    fn only_the_mapping_reaches_the_fit() {
        let params = dmr_params();
        let measured = measured(&dmr_sync(), &params, 1.2, 0.4);
        let mut from_entry = KnownSymbols::new(&params, TIMEOUT);
        let mut from_table = KnownSymbols::from_mapping(params.mapping(), TIMEOUT);
        from_entry.anchor(&dmr_sync(), &measured);
        from_table.anchor(&dmr_sync(), &measured);
        for level in [-3.7f32, -0.9, 0.0, 1.1, 4.2] {
            assert_eq!(from_table.correct(level), from_entry.correct(level));
        }
    }

    #[test]
    fn an_anchor_recovers_the_transmitters_centre_and_gain() {
        let params = dmr_params();
        let mut hook = KnownSymbols::new(&params, TIMEOUT);
        hook.anchor(&dmr_sync(), &measured(&dmr_sync(), &params, 1.2, 0.4));
        for sent in [-3.0f32, -1.0, 1.0, 3.0] {
            let corrected = hook.correct(1.2 * sent + 0.4);
            assert!(
                (corrected - sent).abs() < 1e-4,
                "{sent} corrected to {corrected}"
            );
        }
    }

    /// The tolerance a sync is matched at lets noise through now and then; a fit no real
    /// transmitter under a matched sync can produce must not be believed.
    #[test]
    fn an_implausible_fit_is_discarded() {
        let params = dmr_params();
        let mut hook = KnownSymbols::new(&params, TIMEOUT);
        hook.anchor(&dmr_sync(), &[3.0; 24]);
        assert_eq!(hook.correct(1.0), 1.0, "a flat fit was folded in");
        hook.anchor(&dmr_sync(), &measured(&dmr_sync(), &params, 3.0, 0.0));
        assert_eq!(hook.correct(1.0), 1.0, "a triple gain was believed");
        hook.anchor(&dmr_sync(), &measured(&dmr_sync(), &params, 1.0, 1.4));
        assert_eq!(
            hook.correct(1.0),
            1.0,
            "a centre past half a spacing was believed"
        );
    }

    /// Noise that cleared the index-matching tolerance still does not *fit* the levels.
    #[test]
    fn a_chance_match_that_does_not_fit_the_levels_is_discarded() {
        let params = dmr_params();
        let mut hook = KnownSymbols::new(&params, TIMEOUT);
        let scattered: Vec<f32> = measured(&dmr_sync(), &params, 1.0, 0.0)
            .iter()
            .enumerate()
            .map(|(i, &y)| y + if i % 2 == 0 { 2.2 } else { -2.2 })
            .collect();
        hook.anchor(&dmr_sync(), &scattered);
        assert_eq!(hook.correct(1.0), 1.0, "a scattered fit was believed");
    }

    /// A pattern of one repeated level fits any gain, whatever units the table uses.
    #[test]
    fn a_degenerate_pattern_is_rejected() {
        let params = dmr_params();
        let mut hook = KnownSymbols::new(&params, TIMEOUT);
        hook.anchor(&[0b01; 24], &[3.3; 24]);
        assert_eq!(hook.correct(1.0), 1.0);
    }

    /// Four detections is the whole memory: a transmitter that moved is fully forgotten four
    /// bursts later, and each correction is the average of what is in the ring.
    #[test]
    fn the_estimate_averages_the_last_four_detections() {
        let params = dmr_params();
        let mut hook = KnownSymbols::new(&params, TIMEOUT);
        for _ in 0..ANCHORS {
            hook.anchor(&dmr_sync(), &measured(&dmr_sync(), &params, 1.0, 0.5));
        }
        for _ in 0..ANCHORS {
            hook.anchor(&dmr_sync(), &measured(&dmr_sync(), &params, 1.0, -0.2));
        }
        let corrected = hook.correct(-0.2);
        assert!(corrected.abs() < 1e-4, "stale centre survived: {corrected}");
    }

    /// A channel this long without a detection is between transmissions; the next transmitter
    /// must not be sliced by the last one's levels.
    #[test]
    fn an_anchor_expires_between_transmissions() {
        let params = dmr_params();
        let mut hook = KnownSymbols::new(&params, TIMEOUT);
        hook.anchor(&dmr_sync(), &measured(&dmr_sync(), &params, 1.5, 0.8));
        assert!(hook.correct(0.8).abs() < 1e-4);
        for _ in 0..TIMEOUT {
            hook.tick();
        }
        assert_eq!(hook.correct(0.8), 0.8, "the correction outlived its sync");
    }

    /// Any M, any pattern length: an 8-level hook over a 15-symbol training sequence, with the
    /// same half-spacing gates scaled off the 8-level table.
    #[test]
    fn an_eight_level_pattern_anchors_too() {
        let params = CpmParams::from_h(Mapping::natural(8), 0.3, pulse::rect(8.0, Norm::Area), 8.0);
        let pattern: Vec<u8> = (0..15).map(|i| (i * 5) % 8).collect();
        let mut hook = KnownSymbols::new(&params, 1_000);
        hook.anchor(&pattern, &measured(&pattern, &params, 0.8, -0.6));
        for sent in [-7.0f32, -3.0, 1.0, 7.0] {
            let corrected = hook.correct(0.8 * sent - 0.6);
            assert!(
                (corrected - sent).abs() < 1e-3,
                "{sent} corrected to {corrected}"
            );
        }
        // The gates scale with the table: a centre of 0.9 spacings is implausible at any M.
        let mut fresh = KnownSymbols::new(&params, 1_000);
        fresh.anchor(&pattern, &measured(&pattern, &params, 1.0, 1.8));
        assert_eq!(fresh.correct(1.0), 1.0);
    }

    #[test]
    #[should_panic(expected = "one-to-one")]
    fn mismatched_slices_are_a_caller_bug() {
        let params = dmr_params();
        let mut hook = KnownSymbols::new(&params, TIMEOUT);
        hook.anchor(&[1, 2, 3], &[1.0, 2.0]);
    }
}
