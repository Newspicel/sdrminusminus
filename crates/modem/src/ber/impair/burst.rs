//! The burst/TDMA model: on/off keying frames, raised-cosine keying ramps, an optional
//! per-burst level step, and a receiver noise floor in the gaps. The floor is not optional
//! decoration — a carrier gate measures its noise floor from the dead time, and digital
//! silence would teach it a floor of zero and break the very behaviour the model exists to
//! stress (the shape the phase-0 four-level front end's `keyed` test fixture established, now
//! `cpm::demod`'s; this is that fixture promoted to a calibrated instrument).

use num_complex::Complex;

use super::{Impairment, rms};
use crate::ber::rng::Rng;

/// Carves a continuous waveform into TDMA frames of `on_samples` radiated + `off_samples`
/// dead, with raised-cosine keying over `ramp_samples` at each burst edge (inside the on
/// window — a PA ramps while radiating, it does not stretch the slot). Odd-numbered bursts
/// are scaled by `level_step_db`, modelling the alternating-source level steps a repeater
/// handoff or two-slot TDMA produces; 0 dB disables the step. The receiver noise floor sits
/// `floor_db` below the waveform's pre-carving RMS, across the whole waveform — the receiver
/// hears its own front end whether or not anyone transmits.
#[derive(Clone, Copy, Debug)]
pub struct BurstModel {
    on_samples: usize,
    off_samples: usize,
    ramp_samples: usize,
    level_step_db: f64,
    floor_db: f64,
}

impl BurstModel {
    #[must_use]
    pub fn new(
        on_samples: usize,
        off_samples: usize,
        ramp_samples: usize,
        level_step_db: f64,
        floor_db: f64,
    ) -> Self {
        debug_assert!(
            2 * ramp_samples <= on_samples,
            "ramps longer than the burst leave nothing at full level"
        );
        Self {
            on_samples,
            off_samples,
            ramp_samples,
            level_step_db,
            floor_db,
        }
    }

    /// The keying gain at `pos` samples into the on window — the stated ramp shape, exposed
    /// so the calibration test asserts the *waveform* against the *statement* rather than
    /// against a copy of the implementation.
    #[must_use]
    pub fn keying_gain(&self, pos: usize) -> f64 {
        let ramp = self.ramp_samples as f64;
        if pos >= self.on_samples {
            return 0.0;
        }
        let rising = pos as f64;
        let falling = (self.on_samples - pos) as f64;
        if self.ramp_samples == 0 {
            return 1.0;
        }
        if rising < ramp {
            0.5 * (1.0 - (std::f64::consts::PI * rising / ramp).cos())
        } else if falling < ramp {
            0.5 * (1.0 - (std::f64::consts::PI * falling / ramp).cos())
        } else {
            1.0
        }
    }
}

impl Impairment for BurstModel {
    fn apply(&self, x: &mut Vec<Complex<f32>>, rng: &mut Rng) {
        let frame = self.on_samples + self.off_samples;
        if frame == 0 {
            return;
        }
        let signal_rms = rms(x);
        let sigma = signal_rms * 10f64.powf(-self.floor_db / 20.0) / std::f64::consts::SQRT_2;
        let step = 10f64.powf(self.level_step_db / 20.0);
        for (n, s) in x.iter_mut().enumerate() {
            let pos = n % frame;
            let level = if (n / frame) % 2 == 1 { step } else { 1.0 };
            let gain = (self.keying_gain(pos) * level) as f32;
            let (ni, nq) = rng.normal_pair();
            s.re = s.re * gain + (sigma * ni) as f32;
            s.im = s.im * gain + (sigma * nq) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BurstModel;
    use crate::ber::{
        impair::{Impairment, mean_power, testutil::ones},
        rng::Rng,
    };

    const ON: usize = 200;
    const OFF: usize = 100;
    const RAMP: usize = 20;

    /// Applied == measured for the frame timing: with the envelope thresholded at half
    /// amplitude, each frame's on-stretch measures `on − ramp` (the two half-ramps fall
    /// below threshold) and each gap `off + ramp`.
    #[test]
    fn gap_and_on_lengths_read_back_from_the_envelope() {
        let mut x = ones(30 * (ON + OFF));
        BurstModel::new(ON, OFF, RAMP, 0.0, 60.0).apply(&mut x, &mut Rng::new(0xb0b));
        let above: Vec<bool> = x.iter().map(|s| s.norm() > 0.5).collect();
        let mut runs: Vec<(bool, usize)> = Vec::new();
        for &a in &above {
            match runs.last_mut() {
                Some((state, len)) if *state == a => *len += 1,
                _ => runs.push((a, 1)),
            }
        }
        // Skip the first and last runs — they are truncated by the waveform edges.
        for &(state, len) in &runs[1..runs.len() - 1] {
            let expected = if state { ON - RAMP } else { OFF + RAMP };
            assert!(
                (len as i64 - expected as i64).abs() <= 2,
                "{} run of {len}, expected {expected}",
                if state { "on" } else { "off" }
            );
        }
        assert!(
            runs.len() > 50,
            "keying did not happen: {} runs",
            runs.len()
        );
    }

    /// Applied == measured for the ramp: the envelope through a keying edge follows the
    /// stated raised-cosine shape sample by sample, floor-noise deep below the 1% gate.
    #[test]
    fn ramp_shape_reads_back_from_the_envelope() {
        let model = BurstModel::new(ON, OFF, RAMP, 0.0, 80.0);
        let mut x = ones(4 * (ON + OFF));
        model.apply(&mut x, &mut Rng::new(0x4a3));
        let frame = ON + OFF;
        // Second frame: past any first-frame edge truncation, level step disabled.
        for pos in 0..ON {
            let measured = f64::from(x[frame + pos].norm());
            let stated = model.keying_gain(pos);
            assert!(
                (measured - stated).abs() < 0.01,
                "pos {pos}: stated gain {stated}, measured {measured}"
            );
        }
    }

    /// Applied == measured for the level step: burst 1's flat top sits the stated dB above
    /// burst 0's.
    #[test]
    fn level_step_reads_back() {
        let step_db = 6.0;
        let mut x = ones(2 * (ON + OFF));
        BurstModel::new(ON, OFF, RAMP, step_db, 80.0).apply(&mut x, &mut Rng::new(0x57e9));
        let frame = ON + OFF;
        let flat = |start: usize| mean_power(&x[start + RAMP + 5..start + ON - RAMP - 5]);
        let measured = 10.0 * (flat(frame) / flat(0)).log10();
        assert!(
            (measured - step_db).abs() < 0.1,
            "applied step {step_db} dB, measured {measured} dB"
        );
    }

    /// Applied == measured for the noise floor: the gap centres carry power at the stated
    /// level below the signal, not digital silence.
    #[test]
    fn gap_noise_floor_reads_back() {
        let floor_db = 30.0;
        let frames = 100;
        let mut x = ones(frames * (ON + OFF));
        BurstModel::new(ON, OFF, RAMP, 0.0, floor_db).apply(&mut x, &mut Rng::new(0xf100));
        let frame = ON + OFF;
        let mut gap: Vec<num_complex::Complex<f32>> = Vec::new();
        for f in 0..frames {
            let start = f * frame + ON + RAMP;
            gap.extend_from_slice(&x[start..start + OFF - 2 * RAMP]);
        }
        let measured = -10.0 * mean_power(&gap).log10();
        assert!(
            (measured - floor_db).abs() < 0.5,
            "applied floor {floor_db} dB, measured {measured} dB"
        );
    }
}
