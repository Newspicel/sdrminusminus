use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::{Ddc, SpectrumAnalyzer};

use super::detect::Band;

const ZOOM_OVERSAMPLE: f64 = 4.0;
const MIN_ZOOM_RATE_HZ: f64 = 2_000.0;
const SETTLE: usize = 96;

const FFT_SIZES: [usize; 3] = [256, 1_024, 4_096];
const MIN_ANALYSIS_SAMPLES: usize = FFT_SIZES[0];
const MIN_RUN_SAMPLES: usize = 256;
const MAX_SEGMENTS: usize = 16;

const CLOCK_SKIRT_BINS: usize = 8;

const BACKGROUND_BINS: usize = 32;
const GUARD_BINS: usize = 4;

const MIN_SHIFT_FRACTION: f64 = 1e-4;

const HIST_BINS: usize = 96;
const PEAK_FLOOR: f32 = 0.25;
const PEAK_SEPARATION: usize = HIST_BINS / 12;

const LINE_THRESHOLD_DB: f32 = 6.0;
const MIN_CLOCK_SEGMENTS: usize = 3;

const MAX_HARMONIC: usize = 8;
const SUBHARMONIC_FRACTION: f32 = 0.5;

const KEYED_FRACTION: f32 = 0.35;

const RAYLEIGH_VARIATION: f32 = 0.522_723;

pub(crate) struct Zoom {
    pub(crate) rate: f64,
    pub(crate) iq: Vec<Complex<f32>>,
}

pub(crate) fn zoom(iq: &[Complex<f32>], rate: f64, band: &Band) -> Option<Zoom> {
    let target = (band.bandwidth_hz * ZOOM_OVERSAMPLE).clamp(MIN_ZOOM_RATE_HZ, rate);
    let mut ddc = Ddc::new(rate, target, band.center_hz).ok()?;
    let mut out = Vec::with_capacity((iq.len() as f64 * target / rate) as usize + 1);
    ddc.process(iq, &mut out);
    if out.len() <= SETTLE + MIN_ANALYSIS_SAMPLES {
        return None;
    }
    out.drain(..SETTLE);
    Some(Zoom {
        rate: target,
        iq: out,
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Waveform {
    pub(crate) envelope_variation: f32,
    pub(crate) noise_variation: f32,
    pub(crate) duty: f32,
    pub(crate) on_off_db: f32,
    pub(crate) frequency_levels: u8,
    pub(crate) deviation_hz: f64,
    pub(crate) frequency_spread_hz: f64,
    pub(crate) level_valley: f32,
    pub(crate) symbol_rate_hz: Option<f64>,
    pub(crate) square_line_db: f32,
    pub(crate) quartic_line_db: f32,
}

pub(crate) struct Meter {
    spectra: Vec<SpectrumAnalyzer>,
    amplitude: Vec<f32>,
    frequency: Vec<f32>,
    keyed: Vec<bool>,
    weight: Vec<f32>,
    edge: Vec<f32>,
    runs: Vec<(usize, usize)>,
    starts: Vec<usize>,
    fft_in: Vec<Complex<f32>>,
    fft_db: Vec<f32>,
    power: Vec<f32>,
    contrast: Vec<f32>,
    scratch: Vec<f32>,
    histogram: Vec<f32>,
    smoothed: Vec<f32>,
}

impl Meter {
    pub(crate) fn new() -> Self {
        let largest = FFT_SIZES[FFT_SIZES.len() - 1];
        Self {
            spectra: FFT_SIZES
                .iter()
                .map(|&n| SpectrumAnalyzer::new(n))
                .collect(),
            amplitude: Vec::new(),
            frequency: Vec::new(),
            keyed: Vec::new(),
            weight: Vec::new(),
            edge: Vec::new(),
            runs: Vec::new(),
            starts: Vec::new(),
            fft_in: vec![Complex::new(0.0, 0.0); largest],
            fft_db: vec![0.0; largest],
            power: vec![0.0; largest],
            contrast: vec![0.0; largest],
            scratch: Vec::new(),
            histogram: vec![0.0; HIST_BINS],
            smoothed: vec![0.0; HIST_BINS],
        }
    }

    pub(crate) fn measure(&mut self, zoom: &Zoom, band: &Band) -> Waveform {
        let (duty, on_off_db, gate) = self.envelope(&zoom.iq);
        let envelope_variation = self.envelope_variation(gate);
        self.discriminate(&zoom.iq, zoom.rate, gate);

        let levels = self.levels(zoom.rate);
        let symbol_rate_hz = self.symbol_rate(zoom.rate, levels.count);
        let (square_line_db, quartic_line_db) = self.nonlinearity_lines(&zoom.iq);

        Waveform {
            envelope_variation,
            noise_variation: noise_variation(zoom.rate, band),
            duty,
            on_off_db,
            frequency_levels: levels.count,
            deviation_hz: levels.deviation_hz,
            frequency_spread_hz: levels.spread_hz,
            level_valley: levels.valley,
            symbol_rate_hz,
            square_line_db,
            quartic_line_db,
        }
    }

    fn envelope(&mut self, iq: &[Complex<f32>]) -> (f32, f32, f32) {
        self.amplitude.clear();
        self.amplitude.extend(iq.iter().map(|s| s.norm()));
        let high = self.percentile(0.90);
        let low = self.percentile(0.10);
        let gate = high * KEYED_FRACTION;
        let on = self.amplitude.iter().filter(|&&a| a >= gate).count();
        let duty = on as f32 / self.amplitude.len().max(1) as f32;
        let on_off_db = 20.0 * (high / low.max(f32::MIN_POSITIVE)).log10();
        (duty, on_off_db, gate)
    }

    fn percentile(&mut self, fraction: f32) -> f32 {
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.amplitude);
        let n = self.scratch.len();
        if n == 0 {
            return 0.0;
        }
        let index = ((n - 1) as f32 * fraction) as usize;
        let (_, value, _) = self.scratch.select_nth_unstable_by(index, f32::total_cmp);
        *value
    }

    fn envelope_variation(&self, gate: f32) -> f32 {
        let mut n = 0u32;
        let mut sum = 0.0f64;
        let mut sum_sq = 0.0f64;
        for &a in self.amplitude.iter().filter(|&&a| a >= gate) {
            n += 1;
            sum += f64::from(a);
            sum_sq += f64::from(a) * f64::from(a);
        }
        if n < 2 {
            return 0.0;
        }
        let mean = sum / f64::from(n);
        let variance = (sum_sq / f64::from(n) - mean * mean).max(0.0);
        (variance.sqrt() / mean.max(f64::MIN_POSITIVE)) as f32
    }

    fn discriminate(&mut self, iq: &[Complex<f32>], rate: f64, gate: f32) {
        let scale = (rate / TAU) as f32;
        self.frequency.clear();
        self.frequency.push(0.0);
        for pair in iq.windows(2) {
            self.frequency
                .push((pair[1] * pair[0].conj()).arg() * scale);
        }

        let mut slope_sq = 0.0f64;
        for pair in self.frequency.windows(2) {
            let d = f64::from(pair[1] - pair[0]);
            slope_sq += d * d;
        }
        let slope_rms = (slope_sq / self.frequency.len().max(1) as f64)
            .sqrt()
            .max(1.0) as f32;

        self.keyed.clear();
        self.keyed.extend(self.amplitude.iter().map(|&a| a >= gate));

        self.weight.clear();
        self.weight.push(0.0);
        for i in 1..self.frequency.len() {
            let carrier = self.keyed[i] && self.keyed[i - 1];
            let slope = (self.frequency[i] - self.frequency[i - 1]) / slope_rms;
            let dwell = 1.0 / (1.0 + slope * slope);
            self.weight.push(if carrier {
                self.amplitude[i] * self.amplitude[i] * dwell
            } else {
                0.0
            });
        }
    }

    fn levels(&mut self, rate: f64) -> Levels {
        let bare = |spread: f64| Levels {
            spread_hz: spread,
            count: if spread > 0.0 { 1 } else { 0 },
            deviation_hz: spread,
            valley: 1.0,
        };
        let total: f64 = self.weight.iter().map(|&w| f64::from(w)).sum();
        if total <= 0.0 {
            return bare(0.0);
        }
        let weighted = |power: i32, mean: f64| -> f64 {
            self.frequency
                .iter()
                .zip(&self.weight)
                .map(|(&f, &w)| f64::from(w) * (f64::from(f) - mean).powi(power))
                .sum::<f64>()
                / total
        };
        let mean = weighted(1, 0.0);
        let spread = weighted(2, mean).max(0.0).sqrt();
        if spread < rate * MIN_SHIFT_FRACTION {
            return bare(spread);
        }
        let span = spread * 3.0;

        let scale = HIST_BINS as f64 / (2.0 * span);
        self.histogram.fill(0.0);
        for (&f, &w) in self.frequency.iter().zip(&self.weight) {
            if w <= 0.0 {
                continue;
            }
            let position = (f64::from(f) - mean + span) * scale;
            if position < 0.0 || position >= HIST_BINS as f64 {
                continue;
            }
            self.histogram[position as usize] += w;
        }
        smooth(&self.histogram, &mut self.smoothed);
        smooth(&self.smoothed, &mut self.histogram);
        std::mem::swap(&mut self.histogram, &mut self.smoothed);

        let peaks = self.peaks();
        let to_hz = |bin: usize| (bin as f64 + 0.5) / scale - span + mean;
        let (deviation, valley) = match peaks.as_slice() {
            [] => (spread, 1.0),
            [only] => ((to_hz(*only) - mean).abs().max(spread), 1.0),
            [first, .., last] => (
                (to_hz(*last) - to_hz(*first)) / 2.0,
                self.valley(*first, *last),
            ),
        };
        Levels {
            spread_hz: spread,
            count: peaks.len().min(u8::MAX as usize) as u8,
            deviation_hz: deviation,
            valley,
        }
    }

    fn valley(&self, first: usize, last: usize) -> f32 {
        let rim = self.smoothed[first].min(self.smoothed[last]);
        if rim <= 0.0 {
            return 1.0;
        }
        let floor = self.smoothed[first..=last]
            .iter()
            .copied()
            .fold(f32::INFINITY, f32::min);
        (floor / rim).clamp(0.0, 1.0)
    }

    fn peaks(&self) -> Vec<usize> {
        let max = self.smoothed.iter().copied().fold(0.0f32, f32::max);
        if max <= 0.0 {
            return Vec::new();
        }
        let floor = max * PEAK_FLOOR;
        let mut peaks: Vec<usize> = Vec::new();
        for i in 1..HIST_BINS - 1 {
            let v = self.smoothed[i];
            if v < floor || v < self.smoothed[i - 1] || v < self.smoothed[i + 1] {
                continue;
            }
            match peaks.last() {
                Some(&previous) if i - previous < PEAK_SEPARATION => {
                    if v > self.smoothed[previous] {
                        let last = peaks.len() - 1;
                        peaks[last] = i;
                    }
                }
                _ => peaks.push(i),
            }
        }
        peaks
    }

    fn symbol_rate(&mut self, rate: f64, levels: u8) -> Option<f64> {
        let series: &[Series] = if levels >= 2 {
            &[Series::Frequency]
        } else {
            &[Series::Envelope, Series::Frequency]
        };
        let mut best: Option<(f64, f32)> = None;
        for &series in series {
            for &detector in &[Detector::Square, Detector::Difference] {
                self.build_edge(series, detector);
                if let Some((hz, db)) = self.line_search(rate)
                    && best.is_none_or(|(_, previous)| db > previous)
                {
                    best = Some((hz, db));
                }
            }
        }
        best.filter(|&(_, db)| db >= LINE_THRESHOLD_DB)
            .map(|(hz, _)| hz)
    }

    fn build_edge(&mut self, series: Series, detector: Detector) {
        let values: &[f32] = match series {
            Series::Frequency => &self.frequency,
            Series::Envelope => &self.amplitude,
        };
        self.runs.clear();
        match series {
            Series::Frequency => keyed_runs(&self.keyed, MIN_RUN_SAMPLES, &mut self.runs),
            Series::Envelope => self.runs.push((0, values.len())),
        }
        let live: usize = self.runs.iter().map(|&(a, b)| b - a).sum();
        let mean = if live == 0 {
            0.0
        } else {
            (self
                .runs
                .iter()
                .flat_map(|&(a, b)| values[a..b].iter())
                .map(|&v| f64::from(v))
                .sum::<f64>()
                / live as f64) as f32
        };

        self.edge.clear();
        match detector {
            Detector::Square => self
                .edge
                .extend(values.iter().map(|&v| (v - mean) * (v - mean))),
            Detector::Difference => {
                self.edge.push(0.0);
                self.edge.extend(values.windows(2).map(|p| {
                    let d = p[1] - p[0];
                    d * d
                }));
            }
        }
    }

    fn line_search(&mut self, rate: f64) -> Option<(f64, f32)> {
        let size = self.spectrum_of_runs()?;

        let bin_hz = rate / size as f64;
        let center = size / 2;
        let first = center + CLOCK_SKIRT_BINS;
        let last = (center + (rate / ZOOM_OVERSAMPLE / bin_hz) as usize).min(size - 2);
        if first >= last {
            return None;
        }
        self.contrast(size);
        let tallest =
            (first..=last).max_by(|&a, &b| self.contrast[a].total_cmp(&self.contrast[b]))?;
        let db = 10.0 * self.contrast[tallest].max(f32::MIN_POSITIVE).log10();
        let peak = self.fundamental(tallest, center, first);
        Some((interpolate(&self.contrast, peak, center, bin_hz), db))
    }

    fn contrast(&mut self, size: usize) {
        self.scratch.clear();
        self.scratch.push(0.0);
        let mut running = 0.0f64;
        for &p in &self.power[..size] {
            running += f64::from(p);
            self.scratch.push(running as f32);
        }
        let quiet = (running / size as f64 * 1e-6) as f32;
        if quiet <= 0.0 || !quiet.is_finite() {
            self.contrast[..size].fill(1.0);
            return;
        }
        let sum =
            |prefix: &[f32], lo: usize, hi: usize| prefix[hi.min(size)] - prefix[lo.min(size)];
        for i in 0..size {
            let wide = sum(
                &self.scratch,
                i.saturating_sub(BACKGROUND_BINS),
                i + BACKGROUND_BINS + 1,
            );
            let guarded = sum(
                &self.scratch,
                i.saturating_sub(GUARD_BINS),
                i + GUARD_BINS + 1,
            );
            let bins = (i + BACKGROUND_BINS + 1).min(size)
                - i.saturating_sub(BACKGROUND_BINS)
                - ((i + GUARD_BINS + 1).min(size) - i.saturating_sub(GUARD_BINS));
            let background = (wide - guarded) / bins.max(1) as f32;
            self.contrast[i] = self.power[i] / background.max(quiet);
        }
    }

    fn fundamental(&self, tallest: usize, center: usize, first: usize) -> usize {
        let strong = self.contrast[tallest] * SUBHARMONIC_FRACTION;
        let offset = tallest - center;
        for divisor in (2..=MAX_HARMONIC).rev() {
            let candidate = center + offset / divisor;
            if candidate <= first + 2 {
                continue;
            }
            let Some(local) = (candidate - 1..=candidate + 1)
                .max_by(|&a, &b| self.contrast[a].total_cmp(&self.contrast[b]))
            else {
                continue;
            };
            let shoulder = self.contrast[local - 2].max(self.contrast[local + 2]);
            if self.contrast[local] >= strong && self.contrast[local] > shoulder {
                return local;
            }
        }
        tallest
    }

    fn nonlinearity_lines(&mut self, iq: &[Complex<f32>]) -> (f32, f32) {
        if iq.len() < MIN_ANALYSIS_SAMPLES {
            return (0.0, 0.0);
        }
        let unit = |x: Complex<f32>| {
            let n = x.norm();
            if n > f32::MIN_POSITIVE {
                x / n
            } else {
                Complex::new(0.0, 0.0)
            }
        };
        let square = self.line_strength(iq.len(), |i| {
            let u = unit(iq[i]);
            u * u
        });
        let quartic = self.line_strength(iq.len(), |i| {
            let u = unit(iq[i]);
            let s = u * u;
            s * s
        });
        (square, quartic)
    }

    fn line_strength(&mut self, len: usize, sample: impl Fn(usize) -> Complex<f32>) -> f32 {
        let size = self.average_spectrum(len, sample);
        let Some(peak) = (0..size).max_by(|&a, &b| self.power[a].total_cmp(&self.power[b])) else {
            return 0.0;
        };
        let median = self.median_power(0, size - 1);
        10.0 * (self.power[peak] / median.max(f32::MIN_POSITIVE)).log10()
    }

    fn spectrum_of_runs(&mut self) -> Option<usize> {
        let mut index = 0;
        let mut size = 0;
        for (candidate, &n) in FFT_SIZES.iter().enumerate().rev() {
            self.plan_segments(n);
            if self.starts.len() >= MIN_CLOCK_SEGMENTS {
                (index, size) = (candidate, n);
                break;
            }
        }
        if size == 0 {
            return None;
        }
        let stride = self.starts.len().div_ceil(MAX_SEGMENTS);

        self.power[..size].fill(0.0);
        let mut segments = 0u32;
        for k in (0..self.starts.len()).step_by(stride) {
            let start = self.starts[k];
            for (slot, &v) in self.fft_in[..size]
                .iter_mut()
                .zip(&self.edge[start..start + size])
            {
                *slot = Complex::new(v, 0.0);
            }
            let (input, output) = (&self.fft_in[..size], &mut self.fft_db[..size]);
            self.spectra[index].power_db(input, output);
            for (acc, &db) in self.power[..size].iter_mut().zip(&self.fft_db[..size]) {
                *acc += (db * 0.332_192_8).exp2();
            }
            segments += 1;
        }
        let scale = 1.0 / f32::from(u16::try_from(segments).unwrap_or(u16::MAX)).max(1.0);
        for p in &mut self.power[..size] {
            *p *= scale;
        }
        Some(size)
    }

    fn plan_segments(&mut self, size: usize) {
        self.starts.clear();
        for &(a, b) in &self.runs {
            let mut start = a;
            while start + size <= b {
                self.starts.push(start);
                start += size / 2;
            }
        }
    }

    fn average_spectrum(&mut self, len: usize, sample: impl Fn(usize) -> Complex<f32>) -> usize {
        let index = FFT_SIZES.iter().rposition(|&n| n <= len).unwrap_or(0);
        let size = FFT_SIZES[index];
        let spare = len - size;
        let segments = (spare / (size / 2) + 1).min(MAX_SEGMENTS);
        let hop = if segments > 1 {
            spare / (segments - 1)
        } else {
            0
        };

        self.power[..size].fill(0.0);
        for s in 0..segments {
            let start = s * hop;
            for (k, slot) in self.fft_in[..size].iter_mut().enumerate() {
                *slot = sample(start + k);
            }
            let (input, output) = (&self.fft_in[..size], &mut self.fft_db[..size]);
            self.spectra[index].power_db(input, output);
            for (acc, &db) in self.power[..size].iter_mut().zip(&self.fft_db[..size]) {
                *acc += (db * 0.332_192_8).exp2();
            }
        }
        let scale = 1.0 / segments as f32;
        for p in &mut self.power[..size] {
            *p *= scale;
        }
        size
    }

    fn median_power(&mut self, lo: usize, hi: usize) -> f32 {
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.power[lo..=hi]);
        let mid = self.scratch.len() / 2;
        let (_, median, _) = self.scratch.select_nth_unstable_by(mid, f32::total_cmp);
        *median
    }
}

fn noise_variation(zoom_rate: f64, band: &Band) -> f32 {
    let signal_to_noise = 10.0_f64.powf(f64::from(band.snr_db) / 10.0);
    if !signal_to_noise.is_finite() || signal_to_noise <= 0.0 {
        return RAYLEIGH_VARIATION;
    }
    let widening = (zoom_rate / band.bandwidth_hz.max(1.0)).max(1.0);
    ((widening / (2.0 * signal_to_noise)).sqrt() as f32).min(RAYLEIGH_VARIATION)
}

struct Levels {
    spread_hz: f64,
    count: u8,
    deviation_hz: f64,
    valley: f32,
}

#[derive(Clone, Copy)]
enum Series {
    Frequency,
    Envelope,
}

#[derive(Clone, Copy)]
enum Detector {
    Square,
    Difference,
}

fn keyed_runs(keyed: &[bool], minimum: usize, out: &mut Vec<(usize, usize)>) {
    let mut start = 0;
    for i in 0..=keyed.len() {
        if i < keyed.len() && keyed[i] {
            continue;
        }
        if i - start >= minimum {
            out.push((start, i));
        }
        start = i + 1;
    }
}

fn smooth(input: &[f32], out: &mut [f32]) {
    debug_assert_eq!(input.len(), out.len());
    for i in 0..input.len() {
        let left = input[i.saturating_sub(1)];
        let right = input[(i + 1).min(input.len() - 1)];
        out[i] = (left + 2.0 * input[i] + right) * 0.25;
    }
}

fn interpolate(power: &[f32], peak: usize, center: usize, bin_hz: f64) -> f64 {
    let left = f64::from(power[peak - 1]);
    let mid = f64::from(power[peak]);
    let right = f64::from(power[peak + 1]);
    let denominator = left - 2.0 * mid + right;
    let shift = if denominator.abs() > f64::MIN_POSITIVE {
        (0.5 * (left - right) / denominator).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    (peak as f64 - center as f64 + shift) * bin_hz
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    fn fsk(
        levels: &[f64],
        baud: f64,
        deviation_hz: f64,
        symbols: usize,
        seed: u32,
    ) -> Vec<Complex<f32>> {
        let sps = (RATE / baud) as usize;
        let mut state = seed | 1;
        let mut phase = 0.0f64;
        let mut out = Vec::with_capacity(symbols * sps);
        for _ in 0..symbols {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let level = levels[state as usize % levels.len()];
            for _ in 0..sps {
                phase += TAU * level * deviation_hz / RATE;
                out.push(Complex::from_polar(1.0, phase as f32));
            }
        }
        out
    }

    fn band() -> Band {
        Band {
            center_hz: 0.0,
            bandwidth_hz: RATE / ZOOM_OVERSAMPLE,
            snr_db: 40.0,
            carrier_db: 3.0,
            flatness: 0.3,
            skew: 0.0,
            peak_hz: 0.0,
        }
    }

    fn measure(iq: Vec<Complex<f32>>) -> Waveform {
        Meter::new().measure(&Zoom { rate: RATE, iq }, &band())
    }

    #[test]
    fn two_level_keying_reads_two_levels_and_its_shift() {
        let w = measure(fsk(&[-1.0, 1.0], 2_400.0, 2_000.0, 4_000, 0x1234));
        assert_eq!(w.frequency_levels, 2, "levels");
        assert!(w.level_valley < 0.2, "valley {}", w.level_valley);
        assert!(
            (w.deviation_hz - 2_000.0).abs() < 400.0,
            "deviation {} Hz",
            w.deviation_hz
        );
        let baud = w
            .symbol_rate_hz
            .expect("a keyed carrier has a symbol clock");
        assert!((baud - 2_400.0).abs() < 120.0, "baud {baud}");
        assert!(w.envelope_variation < 0.05, "constant envelope");
        assert!(w.duty > 0.99, "continuous");
    }

    #[test]
    fn four_level_keying_reads_four_levels_and_its_outer_deviation() {
        let w = measure(fsk(&[-3.0, -1.0, 1.0, 3.0], 4_800.0, 648.0, 6_000, 0x5678));
        assert_eq!(w.frequency_levels, 4, "levels");
        assert!(
            (w.deviation_hz - 1_944.0).abs() < 400.0,
            "deviation {} Hz",
            w.deviation_hz
        );
        let baud = w
            .symbol_rate_hz
            .expect("a keyed carrier has a symbol clock");
        assert!((baud - 4_800.0).abs() < 240.0, "baud {baud}");
    }

    #[test]
    fn an_unmodulated_carrier_has_one_level_and_no_spread() {
        let iq: Vec<Complex<f32>> = (0..24_000)
            .map(|k| Complex::from_polar(1.0, (TAU * 500.0 * f64::from(k) / RATE) as f32))
            .collect();
        let w = measure(iq);
        assert!(
            w.frequency_spread_hz < 50.0,
            "spread {}",
            w.frequency_spread_hz
        );
        assert!(w.square_line_db > 20.0, "square line {}", w.square_line_db);
    }

    #[test]
    fn a_swept_carrier_has_no_ground_between_its_levels() {
        let mut state = 0x51f3u32;
        let mut smoothed = 0.0f64;
        let mut phase = 0.0f64;
        let iq: Vec<Complex<f32>> = (0..48_000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let noise = f64::from(state) / f64::from(u32::MAX) - 0.5;
                smoothed += 0.02 * (noise - smoothed);
                phase += TAU * smoothed * 12.0 * 3_000.0 / RATE;
                Complex::from_polar(1.0, phase as f32)
            })
            .collect();
        let w = measure(iq);
        assert!(w.level_valley > 0.5, "valley {}", w.level_valley);
    }

    #[test]
    fn the_noise_alone_accounts_for_more_envelope_spread_as_the_signal_weakens() {
        let quiet = Band {
            snr_db: 40.0,
            ..band()
        };
        let weak = Band {
            snr_db: 12.0,
            ..band()
        };
        assert!(noise_variation(RATE, &quiet) < 0.05);
        assert!(noise_variation(RATE, &weak) > 0.2);
        let none = Band {
            snr_db: -40.0,
            ..band()
        };
        assert_eq!(noise_variation(RATE, &none), RAYLEIGH_VARIATION);
    }

    #[test]
    fn a_keyed_carrier_reports_its_duty_and_depth() {
        let mut iq = Vec::new();
        let mut state = 0x2c9fu32;
        for symbol in 0..4_800i32 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let on = !state.is_multiple_of(3);
            for k in 0..10i32 {
                let phase = (TAU * 300.0 * f64::from(symbol * 10 + k) / RATE) as f32;
                iq.push(Complex::from_polar(if on { 1.0 } else { 0.001 }, phase));
            }
        }
        let w = measure(iq);
        assert!(w.duty > 0.55 && w.duty < 0.8, "duty {}", w.duty);
        assert!(w.on_off_db > 30.0, "depth {} dB", w.on_off_db);
        assert!(w.envelope_variation < 0.05, "on-state is flat");
        let baud = w.symbol_rate_hz.expect("keying has a clock");
        assert!((baud - 4_800.0).abs() < 250.0, "baud {baud}");
    }
}
