use num_complex::Complex;
use sdrmm_dsp::{IqDcBlocker, Nco};

use super::FFT_SIZE;

#[derive(Clone, Copy, PartialEq)]
pub struct DspMeta {
    pub center_hz: f64,
    pub sample_rate: f64,
    pub lo_offset_hz: f64,
    pub dc_block: bool,
}

impl DspMeta {
    pub(super) fn lo_hz(&self) -> f64 {
        self.center_hz - self.lo_offset_hz
    }
}

fn dc_block_corner_hz(sample_rate: f64) -> f64 {
    (sample_rate / FFT_SIZE as f64 * 0.25).clamp(1.0, 500.0)
}

pub(super) struct Frontend {
    blocker: IqDcBlocker,
    nco: Nco,
    scratch: Vec<Complex<f32>>,
    meta: DspMeta,
}

impl Frontend {
    pub(super) fn new(meta: DspMeta) -> Self {
        Self {
            blocker: IqDcBlocker::new(meta.sample_rate, dc_block_corner_hz(meta.sample_rate)),
            nco: Nco::new(-meta.lo_offset_hz as f32, meta.sample_rate as f32),
            scratch: Vec::new(),
            meta,
        }
    }

    pub(super) fn reset(&mut self) {
        self.blocker.reset();
        self.nco.reset();
    }

    pub(super) fn follow(&mut self, meta: DspMeta) {
        if meta == self.meta {
            return;
        }
        if meta.sample_rate != self.meta.sample_rate {
            self.blocker = IqDcBlocker::new(meta.sample_rate, dc_block_corner_hz(meta.sample_rate));
        }
        if meta.lo_offset_hz != self.meta.lo_offset_hz || meta.sample_rate != self.meta.sample_rate
        {
            self.nco
                .set_freq(-meta.lo_offset_hz as f32, meta.sample_rate as f32);
        }
        self.meta = meta;
    }

    pub(super) fn apply<'a>(&'a mut self, input: &'a [Complex<f32>]) -> &'a [Complex<f32>] {
        let shift = self.meta.lo_offset_hz != 0.0;
        if !self.meta.dc_block && !shift {
            return input;
        }
        self.scratch.clear();
        self.scratch.extend_from_slice(input);
        if self.meta.dc_block {
            self.blocker.process(&mut self.scratch);
        }
        if shift {
            self.nco.mix(&mut self.scratch);
        }
        &self.scratch
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::*;

    const FRONTEND_RATE: f64 = 2_400_000.0;

    fn frontend_meta(lo_offset_hz: f64, dc_block: bool) -> DspMeta {
        DspMeta {
            center_hz: 100_000_000.0,
            sample_rate: FRONTEND_RATE,
            lo_offset_hz,
            dc_block,
        }
    }

    fn wideband_tone(freq_hz: f64, len: usize) -> Vec<Complex<f32>> {
        (0..len)
            .map(|k| {
                let p = TAU * freq_hz * k as f64 / FRONTEND_RATE;
                Complex::new(p.cos() as f32, p.sin() as f32)
            })
            .collect()
    }

    fn dominant_hz(samples: &[Complex<f32>]) -> f64 {
        const BINS: usize = 4_096;
        let mut db = vec![0.0f32; BINS];
        sdrmm_dsp::SpectrumAnalyzer::new(BINS).power_db(&samples[..BINS], &mut db);
        let peak = db
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .unwrap_or(0);
        (peak as f64 - BINS as f64 / 2.0) * FRONTEND_RATE / BINS as f64
    }

    #[test]
    fn a_quiet_frontend_hands_the_capture_through_untouched() {
        let mut frontend = Frontend::new(frontend_meta(0.0, false));
        let input = wideband_tone(120_000.0, 4_096);
        let out = frontend.apply(&input);
        assert_eq!(out.as_ptr(), input.as_ptr(), "the samples were copied");
    }

    #[test]
    fn the_frontend_takes_the_dc_term_out_and_leaves_the_signal() {
        let mut frontend = Frontend::new(frontend_meta(0.0, true));
        let offset = Complex::new(0.08, -0.05);
        let mut input = wideband_tone(120_000.0, 1 << 18);
        for s in &mut input {
            *s += offset;
        }

        let out = frontend.apply(&input).to_vec();
        let tail = &out[out.len() / 2..];
        let mean: Complex<f32> = tail.iter().sum::<Complex<f32>>() / tail.len() as f32;
        assert!(mean.norm() < 1e-3, "dc survived at {}", mean.norm());
        assert!(
            (dominant_hz(tail) - 120_000.0).abs() < 600.0,
            "the tone moved to {} Hz",
            dominant_hz(tail)
        );
    }

    #[test]
    fn an_lo_offset_slides_the_capture_back_under_the_frequency_that_was_asked_for() {
        let mut frontend = Frontend::new(frontend_meta(200_000.0, false));
        let input = wideband_tone(320_000.0, 1 << 16);
        let out = frontend.apply(&input).to_vec();
        assert!(
            (dominant_hz(&out) - 120_000.0).abs() < 600.0,
            "a signal 320 kHz above a displaced LO landed at {} Hz, not 120 kHz",
            dominant_hz(&out)
        );
    }

    #[test]
    fn the_dc_term_is_removed_before_the_offset_carries_it_away_from_centre() {
        let mut frontend = Frontend::new(frontend_meta(200_000.0, true));
        let mut input = wideband_tone(320_000.0, 1 << 18);
        for s in &mut input {
            *s += Complex::new(0.5, 0.5);
        }
        let out = frontend.apply(&input).to_vec();
        let tail = &out[out.len() / 2..];
        assert!(
            (dominant_hz(tail) - 120_000.0).abs() < 600.0,
            "the dc term outshouted the signal at {} Hz",
            dominant_hz(tail)
        );
    }

    #[test]
    fn a_rate_change_rebuilds_the_estimator_and_a_retune_does_not_disturb_it() {
        let mut frontend = Frontend::new(frontend_meta(0.0, true));
        let settled = vec![Complex::new(1.0, 0.0); 1 << 18];
        frontend.apply(&settled);

        let mut retuned = frontend_meta(0.0, true);
        retuned.center_hz += 1_000_000.0;
        frontend.follow(retuned);
        let held = frontend.apply(&[Complex::new(1.0, 0.0); 8])[0];
        assert!(
            held.norm() < 0.05,
            "a retune threw away a still-valid dc estimate"
        );

        let mut faster = retuned;
        faster.sample_rate = 3_200_000.0;
        frontend.follow(faster);
        let fresh = frontend.apply(&[Complex::new(1.0, 0.0); 8])[0];
        assert!(
            fresh.norm() > 0.9,
            "a new sample rate kept an estimator built for the old one"
        );
    }
}
