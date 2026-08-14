//! The reference M-PPM transmitter: one keyed slot per symbol, rendered onto a sample grid the
//! slot boundaries need not agree with.
use num_complex::Complex;

use crate::pulse::{self, Norm};

/// Sub-sample resolution the waveform is rendered at before aperture integration: a sixteenth
/// of a sample, which is what lets a stated phase mean something and edge samples read partial
/// amplitude. Finer buys nothing a receiver could see; coarser quantises the phase axis a
/// decoder's own phase tables are swept against.
pub const OVERSAMPLE: usize = 16;

/// A keyed-slot timeline renderer: `on[j]` is whether slot `j` radiates.
///
/// Separate from [`PpmMod`] because a protocol's timeline is not always M-PPM symbols — Mode S
/// prefixes four preamble pulses at fixed half-chip positions and only then two slots per bit —
/// and the rendering is identical either way.
#[derive(Clone, Copy, Debug)]
pub struct SlotWaveform {
    samples_per_slot: f64,
    phase: f64,
    level: f32,
}

impl SlotWaveform {
    /// # Panics
    /// If `samples_per_slot` is not positive and finite, `phase` is outside `[0, 1)`, or `level`
    /// is not finite.
    #[must_use]
    pub fn new(samples_per_slot: f64, phase: f64, level: f32) -> Self {
        assert!(
            samples_per_slot.is_finite() && samples_per_slot > 0.0,
            "samples per slot must be positive, got {samples_per_slot}"
        );
        assert!(
            phase.is_finite() && (0.0..1.0).contains(&phase),
            "phase is a sub-sample offset in [0, 1), got {phase}"
        );
        assert!(level.is_finite(), "level must be finite");
        Self {
            samples_per_slot,
            phase,
            level,
        }
    }

    /// Output samples a timeline of `slots` slots occupies.
    #[must_use]
    pub fn samples(&self, slots: usize) -> usize {
        (slots as f64 * self.samples_per_slot + self.phase).round() as usize
    }

    /// Renders the timeline onto complex baseband, appending to `out`. The carrier is
    /// unmodulated in phase — PPM carries its information in *when*, and a receiver that reads
    /// phase at all is doing something this modulation never asked for.
    pub fn render(&self, on: &[bool], out: &mut Vec<Complex<f32>>) {
        let samples = self.samples(on.len());
        let fine_phase = self.phase * OVERSAMPLE as f64;
        let fine_per_slot = self.samples_per_slot * OVERSAMPLE as f64;
        out.reserve(samples);
        for sample in 0..samples {
            let mut acc = Complex::new(0.0f32, 0.0);
            for step in 0..OVERSAMPLE {
                let fine = (sample * OVERSAMPLE + step) as f64;
                let slot = (fine - fine_phase) / fine_per_slot;
                if slot >= 0.0 && on.get(slot as usize).copied().unwrap_or(false) {
                    acc += Complex::new(self.level, 0.0);
                }
            }
            out.push(acc / OVERSAMPLE as f32);
        }
    }
}

/// M-PPM modulator: symbol `k` keys slot `k` of every group of `m`.
#[derive(Clone, Debug)]
pub struct PpmMod {
    m: usize,
    waveform: SlotWaveform,
    timeline: Vec<bool>,
}

impl PpmMod {
    /// # Panics
    /// If `m` is not a power of two of at least two — the symbol index is a bit label, and a
    /// slot count that is not a power of two would leave labels no symbol can carry. Plus
    /// [`SlotWaveform::new`]'s panics.
    #[must_use]
    pub fn new(m: usize, samples_per_slot: f64, phase: f64, level: f32) -> Self {
        assert!(
            m >= 2 && m.is_power_of_two(),
            "M must be a power of two of at least 2, got {m}"
        );
        Self {
            m,
            waveform: SlotWaveform::new(samples_per_slot, phase, level),
            timeline: Vec::new(),
        }
    }

    #[must_use]
    pub fn m(&self) -> usize {
        self.m
    }

    #[must_use]
    pub fn bits_per_symbol(&self) -> u32 {
        self.m.trailing_zeros()
    }

    #[must_use]
    pub fn waveform(&self) -> SlotWaveform {
        self.waveform
    }

    /// Symbols to complex baseband, appended to `out`. Symbol indices are taken modulo M, as
    /// every mapping table in the crate does, so a sliced index is always transmittable.
    pub fn modulate(&mut self, symbols: &[u8], out: &mut Vec<Complex<f32>>) {
        self.timeline.clear();
        self.timeline.resize(symbols.len() * self.m, false);
        for (i, &symbol) in symbols.iter().enumerate() {
            self.timeline[i * self.m + (symbol as usize & (self.m - 1))] = true;
        }
        self.waveform.render(&self.timeline, out);
    }
}

/// The slot pulse as filter taps: a rect of `samples_per_slot` samples at unit energy — the
/// matched filter of one radiated slot, and the shape [`SlotWaveform`] renders when the slot
/// happens to be a whole number of samples wide.
///
/// # Panics
/// As [`crate::pulse::rect`].
#[must_use]
pub fn slot_taps(samples_per_slot: f64) -> Vec<f32> {
    pulse::rect(samples_per_slot, Norm::Energy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(on: &[bool], sps: f64, phase: f64) -> Vec<f32> {
        let mut out = Vec::new();
        SlotWaveform::new(sps, phase, 1.0).render(on, &mut out);
        out.iter().map(|s| s.re).collect()
    }

    /// The claim the module docs make about `pulse/`: at a whole-sample slot on the sample grid,
    /// the renderer's keyed slot *is* the rect pulse, up to the norm each is read at.
    #[test]
    fn an_integer_rate_renders_the_rect_pulse_exactly() {
        let taps = slot_taps(8.0);
        let scale = 1.0 / taps[0];
        let rendered = render(&[false, true, false], 8.0, 0.0);
        assert_eq!(rendered.len(), 24);
        for (i, &s) in rendered.iter().enumerate() {
            let expect = if (8..16).contains(&i) {
                taps[i - 8] * scale
            } else {
                0.0
            };
            assert!((s - expect).abs() < 1e-6, "sample {i}: {s} vs {expect}");
        }
    }

    /// A boundary inside a sample must read as partial amplitude — the property that makes a
    /// stated sub-sample phase mean anything, and the one an idealised on/off generator lacks.
    #[test]
    fn a_boundary_inside_a_sample_reads_partial_amplitude() {
        let rendered = render(&[true, false], 2.0, 0.5);
        assert!((rendered[0] - 0.5).abs() < 1e-6, "{rendered:?}");
        assert!((rendered[1] - 1.0).abs() < 1e-6, "{rendered:?}");
        assert!((rendered[2] - 0.5).abs() < 1e-6, "{rendered:?}");
    }

    /// Energy is conserved whatever the phase: a keyed slot radiates its width, so a receiver
    /// comparing slots is never comparing different amounts of transmitter.
    #[test]
    fn a_keyed_slot_radiates_its_width_at_every_phase() {
        for phase in [0.0, 0.19, 0.5, 0.93] {
            for &sps in &[1.024, 2.4, 7.0] {
                let mut on = vec![false; 12];
                on[5] = true;
                let mut out = Vec::new();
                SlotWaveform::new(sps, phase, 1.0).render(&on, &mut out);
                let sum: f64 = out.iter().map(|s| f64::from(s.re)).sum();
                assert!(
                    (sum - sps).abs() < 0.05,
                    "sps {sps} phase {phase}: radiated {sum}"
                );
            }
        }
    }

    /// The symbol-to-slot rule, and the only thing `PpmMod` adds over the timeline renderer.
    #[test]
    fn symbol_k_keys_slot_k() {
        let mut m = PpmMod::new(4, 2.0, 0.0, 1.0);
        let mut out = Vec::new();
        m.modulate(&[2, 0, 3], &mut out);
        let lit: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, s)| s.re > 0.5)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(lit, vec![4, 5, 8, 9, 22, 23]);
    }
}
