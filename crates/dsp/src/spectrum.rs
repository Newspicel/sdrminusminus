use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::window::{coherent_gain, hann};

/// Reusable analyzer for a fixed FFT size.
pub struct SpectrumAnalyzer {
    fft: Arc<dyn Fft<f32>>,
    size: usize,
    window: Vec<f32>,
    inv_gain: f32,
    buf: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl SpectrumAnalyzer {
    /// Build an analyzer for `size`-sample blocks (`size` should be a power of two for speed).
    #[must_use]
    pub fn new(size: usize) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(size.max(1));
        let window = hann(size);
        let inv_gain = 1.0 / coherent_gain(&window).max(f32::MIN_POSITIVE);
        let scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()];
        Self {
            fft,
            size,
            window,
            inv_gain,
            buf: vec![Complex::new(0.0, 0.0); size],
            scratch,
        }
    }

    #[must_use]
    pub fn size(&self) -> usize {
        self.size
    }

    /// Compute DC-centered power spectrum in dBFS. `input` and `out` must both be `size` long;
    /// a full-scale complex tone at a bin center reads ~0 dBFS. Allocation-free.
    pub fn power_db(&mut self, input: &[Complex<f32>], out: &mut [f32]) {
        assert_eq!(input.len(), self.size, "input length must equal FFT size");
        assert_eq!(out.len(), self.size, "output length must equal FFT size");

        for ((dst, &s), &w) in self.buf.iter_mut().zip(input).zip(&self.window) {
            *dst = s * w;
        }
        self.fft
            .process_with_scratch(&mut self.buf, &mut self.scratch);

        let half = self.size / 2;
        for (raw_idx, x) in self.buf.iter().enumerate() {
            let shifted = (raw_idx + half) % self.size;
            let mag = x.norm() * self.inv_gain;
            out[shifted] = 20.0 * (mag + 1e-12).log10();
        }
    }
}

pub fn decimate_max(db: &[f32], out: &mut [f32]) {
    let bins = out.len();
    assert!(bins > 0, "need at least one output bin");
    if db.is_empty() {
        out.fill(f32::NEG_INFINITY);
        return;
    }
    let len = db.len();
    if bins >= len {
        // Upsampling (or 1:1): nearest-neighbor keeps the frequency axis aligned. Never
        // reachable today (FFT_SIZE == MAX_BINS), but correct if that ever changes.
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = db[(i * len / bins).min(len - 1)];
        }
        return;
    }
    for (i, slot) in out.iter_mut().enumerate() {
        let start = i * len / bins;
        let end = ((i + 1) * len / bins).max(start + 1).min(len);
        let mut peak = f32::NEG_INFINITY;
        for &v in &db[start..end] {
            peak = peak.max(v);
        }
        *slot = peak;
    }
}

/// Where the noise floor is read from the distribution of bins. A low percentile and not the
/// minimum, which is one unlucky bin, and not the mean, which every signal present drags upward.
const FLOOR_PERCENTILE: f32 = 0.25;
/// How far above `db_min` the noise floor is placed. Nonzero so noise renders as the low end of
/// the colormap rather than as its background — a floor sitting exactly at the bottom is
/// indistinguishable from a dead receiver.
const FLOOR_MARGIN_DB: f32 = 10.0;
/// Dynamic range above the noise floor when nothing louder needs the room.
const DEFAULT_DB_RANGE: f32 = 70.0;
/// Headroom kept above the loudest bin when a signal is strong enough to widen the window past
/// [`DEFAULT_DB_RANGE`]. Generous on purpose: with a tight margin the loudest thing on screen is
/// pinned to the last entry of the colormap, which is the failure this whole function exists to
/// avoid — just applied to a real carrier instead of to noise.
const PEAK_MARGIN_DB: f32 = 15.0;
/// The window for a frame with no finite bin at all — a lane that has not produced a spectrum
/// yet. Any finite pair will do; this one matches the range a quiet receiver settles into.
const EMPTY_WINDOW: (f32, f32) = (-100.0, -100.0 + DEFAULT_DB_RANGE);

/// The dB window a frame is quantized over: `(db_min, db_max)`.
///
/// Anchored to the noise floor, never to the peak. A peak-anchored window puts whatever is
/// loudest at the top of the scale, so a receiver hearing nothing — an RTL-SDR at zero gain, an
/// antenna left off — paints its own noise in the colour a full-strength carrier should have had.
/// The floor is what the operator needs held still; the ceiling only has to stay above the
/// loudest bin.
///
/// Pass the *decimated* bins, the ones that will actually be drawn, rather than the raw FFT:
/// [`decimate_max`] takes a maximum over each output bin's share and so lifts the apparent floor
/// by several dB, and the estimate has to describe what reaches the screen.
///
/// `scratch` is the caller's reusable buffer; it is overwritten and its contents are not
/// meaningful afterwards. Nothing is allocated once it has grown to `db.len()`.
///
/// The result is a pure function of one frame, so a peak that moves between frames moves the
/// ceiling with it. That only leads while something sits more than
/// `DEFAULT_DB_RANGE - PEAK_MARGIN_DB` above the floor; below that the floor term wins and the
/// window holds still on its own.
#[must_use]
pub fn adaptive_db_window(db: &[f32], scratch: &mut Vec<f32>) -> (f32, f32) {
    let Some(floor) = percentile(db, scratch, FLOOR_PERCENTILE) else {
        return EMPTY_WINDOW;
    };
    let peak = db
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold(f32::NEG_INFINITY, f32::max);
    let min = floor - FLOOR_MARGIN_DB;
    (min, (min + DEFAULT_DB_RANGE).max(peak + PEAK_MARGIN_DB))
}

/// The value at `q` of `db`'s finite entries, `q` in `[0, 1]`. `None` when nothing is finite.
/// Non-finite bins are dropped rather than clamped: `decimate_max` writes `-inf` for a bin with
/// no input, which is an absence, not a very quiet measurement.
fn percentile(db: &[f32], scratch: &mut Vec<f32>, q: f32) -> Option<f32> {
    scratch.clear();
    scratch.extend(db.iter().copied().filter(|v| v.is_finite()));
    let last = scratch.len().checked_sub(1)?;
    let at = (last as f32 * q.clamp(0.0, 1.0)) as usize;
    let (_, nth, _) = scratch.select_nth_unstable_by(at, f32::total_cmp);
    Some(*nth)
}

/// Quantize dB values to `u8` over `[db_min, db_max]` (: the window travels in the
/// frame header so the client can map bytes back to dB). Values clamp to the range.
pub fn quantize_db(db: &[f32], db_min: f32, db_max: f32, out: &mut [u8]) {
    assert_eq!(db.len(), out.len(), "quantize length mismatch");
    let span = (db_max - db_min).max(f32::MIN_POSITIVE);
    for (&v, slot) in db.iter().zip(out.iter_mut()) {
        let t = ((v - db_min) / span).clamp(0.0, 1.0);
        *slot = (t * 255.0 + 0.5) as u8;
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::PI;

    use super::*;

    /// A full-scale complex tone at a known offset must peak at the expected fft-shifted bin,
    /// near 0 dBFS, with a low floor elsewhere.
    #[test]
    fn tone_lands_in_expected_bin_near_0dbfs() {
        let size = 1024;
        let mut an = SpectrumAnalyzer::new(size);

        let bin = size / 8;
        let input: Vec<Complex<f32>> = (0..size)
            .map(|n| Complex::from_polar(1.0, 2.0 * PI * bin as f32 * n as f32 / size as f32))
            .collect();

        let mut db = vec![0.0f32; size];
        an.power_db(&input, &mut db);

        let expected = size / 2 + bin;
        let (peak_idx, &peak_val) = db
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        assert_eq!(peak_idx, expected, "peak bin");
        assert!(peak_val > -1.0, "peak near 0 dBFS, got {peak_val}");

        assert!(
            db[expected / 2] < -60.0,
            "floor too high: {}",
            db[expected / 2]
        );
    }

    #[test]
    fn decimate_max_preserves_peaks() {
        let mut db = vec![-100.0f32; 100];
        db[37] = -3.0; // a narrow spike
        let mut out = vec![0.0f32; 10];
        decimate_max(&db, &mut out);
        // The spike falls in the 4th output bin (37/10) and must survive.
        assert_eq!(out[3], -3.0);
    }

    /// Where a bin lands in the colormap, as the client computes it.
    fn position(db: f32, window: (f32, f32)) -> f32 {
        (db - window.0) / (window.1 - window.0)
    }

    /// A receiver hearing only its own noise — an RTL-SDR at zero gain, the report this window
    /// was rewritten for. Nothing about a flat spectrum may reach the top of the scale.
    #[test]
    fn bare_noise_stays_at_the_bottom_of_the_scale() {
        let mut scratch = Vec::new();
        // Exponential-ish spread around a -60 dBFS floor, no signal anywhere.
        let noise: Vec<f32> = (0..512)
            .map(|i| -60.0 + ((i * 37) % 23) as f32 * 0.5 - 5.0)
            .collect();
        let window = adaptive_db_window(&noise, &mut scratch);

        let hottest = noise.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            position(hottest, window) < 0.4,
            "loudest noise bin reached {:.2} of the scale, window {window:?}",
            position(hottest, window)
        );
    }

    /// The floor holds still while a carrier comes and goes: only the ceiling may move, or the
    /// waterfall would recolour its whole noise field every time a transmission starts.
    #[test]
    fn a_carrier_moves_the_ceiling_and_not_the_floor() {
        let mut scratch = Vec::new();
        let mut bins = vec![-95.0f32; 512];
        let quiet = adaptive_db_window(&bins, &mut scratch);
        bins[100] = -12.0;
        let loud = adaptive_db_window(&bins, &mut scratch);

        assert!((quiet.0 - loud.0).abs() < f32::EPSILON, "floor moved");
        assert!(
            loud.1 > quiet.1,
            "ceiling did not make room for the carrier"
        );
        let at = position(-12.0, loud);
        assert!(
            (0.75..1.0).contains(&at),
            "carrier at {at:.2} of the scale, window {loud:?}"
        );
    }

    /// A quarter of the band occupied must not drag the floor estimate up into the signal.
    #[test]
    fn floor_ignores_an_occupied_quarter_of_the_band() {
        let mut scratch = Vec::new();
        let mut bins = vec![-95.0f32; 400];
        bins.extend(std::iter::repeat_n(-40.0f32, 100));
        let (min, _) = adaptive_db_window(&bins, &mut scratch);
        assert!(
            (-95.0 - FLOOR_MARGIN_DB - min).abs() < 1.0,
            "floor read as {min}, expected the noise and not the occupancy"
        );
    }

    /// `decimate_max` writes `-inf` into bins it had no input for; those are an absence, and a
    /// frame of nothing else must still produce a usable window rather than an infinite one.
    #[test]
    fn non_finite_bins_do_not_reach_the_window() {
        let mut scratch = Vec::new();
        assert_eq!(
            adaptive_db_window(&[f32::NEG_INFINITY; 8], &mut scratch),
            EMPTY_WINDOW
        );
        assert_eq!(adaptive_db_window(&[], &mut scratch), EMPTY_WINDOW);

        let mixed = [f32::NEG_INFINITY, -70.0, -70.0, f32::NAN, -70.0];
        let (min, max) = adaptive_db_window(&mixed, &mut scratch);
        assert!(min.is_finite() && max.is_finite(), "{min} {max}");
        assert!((min - (-80.0)).abs() < f32::EPSILON, "floor read as {min}");
    }

    /// The window always has width, or `quantize_db` would divide by nothing.
    #[test]
    fn window_is_never_empty() {
        let mut scratch = Vec::new();
        for bins in [vec![0.0f32; 4], vec![-200.0f32; 4], vec![-60.0f32; 1]] {
            let (min, max) = adaptive_db_window(&bins, &mut scratch);
            assert!(max > min, "degenerate window {min}..{max}");
        }
    }

    #[test]
    fn quantize_maps_range_to_bytes() {
        let db = [-120.0, -70.0, -20.0];
        let mut out = [0u8; 3];
        quantize_db(&db, -120.0, -20.0, &mut out);
        assert_eq!(out[0], 0);
        assert_eq!(out[2], 255);
        assert!((out[1] as i32 - 128).abs() <= 1);
    }
}
