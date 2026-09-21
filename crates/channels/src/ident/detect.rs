use num_complex::Complex;
use sdrmm_dsp::SpectrumAnalyzer;

pub(crate) const DETECT_FFT: usize = 4_096;

const MAX_SEGMENTS: usize = 24;

const MAX_BANDS: usize = 8;

const MIN_BAND_SNR_DB: f32 = 3.0;

const DOMINATED_OCCUPANCY: f64 = 0.5;

const BAND_EDGE_DB: f32 = 20.0;

const SMOOTH_BINS: usize = 3;

const FLOOR_QUANTILE: f32 = 0.1;

/// How far either side of the LO artifact its main lobe reaches, in bins.
const ARTIFACT_BINS: usize = 2;

fn from_db(db: f32) -> f32 {
    (db * 0.332_192_8).exp2()
}

fn artifact_bins(
    artifact_hz: Option<f64>,
    bin_hz: f64,
    center: usize,
    size: usize,
) -> Option<std::ops::RangeInclusive<usize>> {
    let at = artifact_hz.filter(|hz| hz.is_finite())? / bin_hz + center as f64;
    if !at.is_finite() {
        return None;
    }
    let lo = (at - ARTIFACT_BINS as f64).ceil();
    let hi = (at + ARTIFACT_BINS as f64).floor();
    if hi < 0.0 || lo > (size - 1) as f64 {
        return None;
    }
    Some((lo.max(0.0) as usize)..=(hi.max(0.0) as usize).min(size - 1))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Band {
    pub(crate) center_hz: f64,
    pub(crate) bandwidth_hz: f64,
    pub(crate) snr_db: f32,
    pub(crate) carrier_db: f32,
    pub(crate) flatness: f32,
    pub(crate) skew: f32,
    pub(crate) peak_hz: f64,
}

#[derive(Clone, Copy)]
enum Series {
    Raw,
    Smoothed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Search {
    pub(crate) half_span_hz: f64,
    pub(crate) threshold_db: f32,
    pub(crate) gap_hz: f64,
    pub(crate) dominated: bool,
    pub(crate) artifact_hz: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Survey {
    pub(crate) floor_db: f32,
    pub(crate) peak_db: f32,
    pub(crate) bands: Vec<Band>,
}

pub(crate) struct Detector {
    analyzer: SpectrumAnalyzer,
    size: usize,
    max_bands: usize,
    segment_db: Vec<f32>,
    power: Vec<f32>,
    smoothed: Vec<f32>,
    scratch: Vec<f32>,
    masked: Vec<bool>,
    covered: usize,
}

struct Slice {
    lo: usize,
    hi: usize,
    floor: f32,
    floor_db: f32,
    bin_hz: f64,
    gap: usize,
}

impl Detector {
    pub(crate) fn new() -> Self {
        Self::with_size(DETECT_FFT, MAX_BANDS)
    }

    pub(crate) fn with_size(size: usize, max_bands: usize) -> Self {
        Self {
            analyzer: SpectrumAnalyzer::new(size),
            size,
            max_bands,
            segment_db: vec![0.0; size],
            power: vec![0.0; size],
            smoothed: vec![0.0; size],
            scratch: vec![0.0; size],
            masked: vec![false; size],
            covered: 0,
        }
    }

    pub(crate) fn measure(&mut self, iq: &[Complex<f32>], rate: f64, search: &Search) -> Survey {
        let Search {
            half_span_hz,
            threshold_db,
            gap_hz,
            dominated,
            artifact_hz,
        } = *search;
        let quiet = Survey {
            floor_db: -200.0,
            peak_db: -200.0,
            bands: Vec::new(),
        };
        if iq.len() < self.size || rate <= 0.0 {
            return quiet;
        }
        self.accumulate(iq);

        let bin_hz = rate / self.size as f64;
        let center = self.size / 2;
        let half_bins = ((half_span_hz / bin_hz).floor() as usize).clamp(1, center);
        let lo = center - half_bins;
        let hi = (center + half_bins).min(self.size - 1);
        self.masked.fill(false);
        if let Some(artifact) = artifact_bins(artifact_hz, bin_hz, center, self.size) {
            self.masked[artifact].fill(true);
        }

        let floor = self.quantile_of(Series::Smoothed, lo, hi, FLOOR_QUANTILE);
        let floor_db = 10.0 * (floor.max(f32::MIN_POSITIVE)).log10();
        let Some(peak) = self.loudest(lo, hi) else {
            return quiet;
        };
        let peak_db = self.db_at(peak);
        let slice = Slice {
            lo,
            hi,
            floor,
            floor_db,
            bin_hz,
            gap: ((gap_hz / bin_hz) as usize).max(1),
        };
        let mut survey = Survey {
            floor_db,
            peak_db,
            bands: Vec::new(),
        };
        if peak_db - floor_db < threshold_db {
            if dominated {
                survey.bands.push(self.describe(&slice, lo, hi, peak));
            }
            return survey;
        }
        self.covered = 0;
        self.survey(&slice, threshold_db, peak, &mut survey.bands);
        if dominated && self.covered as f64 >= DOMINATED_OCCUPANCY * (hi - lo + 1) as f64 {
            survey.bands.clear();
            survey.bands.push(self.describe(&slice, lo, hi, peak));
        }
        survey
    }

    fn survey(&mut self, slice: &Slice, threshold_db: f32, first: usize, bands: &mut Vec<Band>) {
        let spill = from_db(slice.floor_db + threshold_db);
        let mut peak = first;
        while bands.len() < self.max_bands {
            let peak_db = self.db_at(peak);
            if peak_db - slice.floor_db < threshold_db {
                break;
            }
            let edge = from_db(
                (slice.floor_db + (threshold_db * 0.5).max(3.0)).max(peak_db - BAND_EDGE_DB),
            );
            let (start, end) = self.extent(peak, slice.lo, slice.hi, edge, slice.gap);
            let band = self.describe(slice, start, end, peak);
            if band.snr_db >= MIN_BAND_SNR_DB {
                bands.push(band);
                self.covered += end - start + 1;
            }
            let (spill_start, spill_end) = self.extent(peak, slice.lo, slice.hi, spill, slice.gap);
            let masked_lo = start
                .min(spill_start)
                .saturating_sub(slice.gap)
                .max(slice.lo);
            let masked_hi = (end.max(spill_end) + slice.gap).min(slice.hi);
            self.masked[masked_lo..=masked_hi].fill(true);
            let Some(next) = self.loudest(slice.lo, slice.hi) else {
                break;
            };
            peak = next;
        }
    }

    fn loudest(&self, lo: usize, hi: usize) -> Option<usize> {
        (lo..=hi)
            .filter(|&i| !self.masked[i])
            .max_by(|&a, &b| self.smoothed[a].total_cmp(&self.smoothed[b]))
    }

    fn db_at(&self, bin: usize) -> f32 {
        10.0 * (self.smoothed[bin].max(f32::MIN_POSITIVE)).log10()
    }

    fn describe(&mut self, slice: &Slice, start: usize, end: usize, peak: usize) -> Band {
        let center = self.size / 2;
        let bins = end - start + 1;
        let occupied: f32 = self.smoothed[start..=end].iter().sum();
        let noise = slice.floor * bins as f32;
        let signal = (occupied - noise).max(f32::MIN_POSITIVE);
        let snr_db = 10.0 * (signal / noise.max(f32::MIN_POSITIVE)).log10();
        let raw_peak_db = 10.0 * self.power[peak].max(f32::MIN_POSITIVE).log10();
        let median_db = 10.0
            * self
                .quantile_of(Series::Raw, start, end, 0.5)
                .max(f32::MIN_POSITIVE)
                .log10();
        Band {
            center_hz: self.centroid_hz(start, end, slice.floor, slice.bin_hz, center),
            bandwidth_hz: bins as f64 * slice.bin_hz,
            snr_db,
            carrier_db: raw_peak_db - median_db,
            flatness: self.flatness(start, end),
            skew: self.skew(start, end, slice.floor, slice.bin_hz),
            peak_hz: (peak as f64 - center as f64) * slice.bin_hz,
        }
    }

    fn accumulate(&mut self, iq: &[Complex<f32>]) {
        let spare = iq.len() - self.size;
        let segments = (spare / (self.size / 2) + 1).min(MAX_SEGMENTS);
        let hop = if segments > 1 {
            spare / (segments - 1)
        } else {
            0
        };

        self.power.fill(0.0);
        for s in 0..segments {
            let start = s * hop;
            self.analyzer
                .power_db(&iq[start..start + self.size], &mut self.segment_db);
            for (acc, &db) in self.power.iter_mut().zip(&self.segment_db) {
                *acc += from_db(db);
            }
        }
        let scale = 1.0 / segments as f32;
        for p in &mut self.power {
            *p *= scale;
        }
        self.smooth();
    }

    fn smooth(&mut self) {
        let half = SMOOTH_BINS / 2;
        for i in 0..self.size {
            let lo = i.saturating_sub(half);
            let hi = (i + half).min(self.size - 1);
            let span = &self.power[lo..=hi];
            self.smoothed[i] = span.iter().sum::<f32>() / span.len() as f32;
        }
    }

    fn quantile_of(&mut self, series: Series, lo: usize, hi: usize, fraction: f32) -> f32 {
        self.scratch.clear();
        self.scratch.extend_from_slice(match series {
            Series::Raw => &self.power[lo..=hi],
            Series::Smoothed => &self.smoothed[lo..=hi],
        });
        let index = ((self.scratch.len() - 1) as f32 * fraction) as usize;
        let (_, value, _) = self.scratch.select_nth_unstable_by(index, f32::total_cmp);
        *value
    }

    fn extent(&self, peak: usize, lo: usize, hi: usize, edge: f32, gap: usize) -> (usize, usize) {
        let mut start = peak;
        let mut end = peak;
        let mut i = peak;
        while i > lo {
            i -= 1;
            if self.masked[i] {
                break;
            }
            if self.smoothed[i] >= edge {
                start = i;
            } else if start - i > gap {
                break;
            }
        }
        let mut j = peak;
        while j < hi {
            j += 1;
            if self.masked[j] {
                break;
            }
            if self.smoothed[j] >= edge {
                end = j;
            } else if j - end > gap {
                break;
            }
        }
        (start, end)
    }

    fn flatness(&self, start: usize, end: usize) -> f32 {
        let bins = (end - start + 1) as f32;
        let mut log_sum = 0.0;
        let mut sum = 0.0;
        for &p in &self.smoothed[start..=end] {
            let p = p.max(f32::MIN_POSITIVE);
            log_sum += p.ln();
            sum += p;
        }
        ((log_sum / bins).exp() / (sum / bins).max(f32::MIN_POSITIVE)).clamp(0.0, 1.0)
    }

    fn centroid_hz(&self, start: usize, end: usize, floor: f32, bin_hz: f64, center: usize) -> f64 {
        let weight = |i: usize| f64::from((self.smoothed[i] - floor).max(0.0));
        let total: f64 = (start..=end).map(weight).sum();
        if total <= 0.0 {
            return (start as f64 + end as f64) / 2.0 - center as f64;
        }
        let mean: f64 = (start..=end).map(|i| weight(i) * i as f64).sum::<f64>() / total;
        (mean - center as f64) * bin_hz
    }

    fn skew(&self, start: usize, end: usize, floor: f32, bin_hz: f64) -> f32 {
        let weight = |i: usize| f64::from((self.smoothed[i] - floor).max(0.0));
        let total: f64 = (start..=end).map(weight).sum();
        if total <= 0.0 {
            return 0.0;
        }
        let mean: f64 = (start..=end).map(|i| weight(i) * i as f64).sum::<f64>() / total;
        let moment = |order: i32| -> f64 {
            (start..=end)
                .map(|i| weight(i) * (i as f64 - mean).powi(order))
                .sum::<f64>()
                / total
        };
        let variance = moment(2);
        if variance * bin_hz * bin_hz < 1.0 {
            return 0.0;
        }
        (moment(3) / variance.powf(1.5)) as f32
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::*;
    use crate::testutil::complex_noise;

    fn tone(freq_hz: f64, rate: f64, len: usize, amp: f32) -> Vec<Complex<f32>> {
        (0..len)
            .map(|k| {
                Complex::from_polar(
                    amp,
                    (TAU * freq_hz * k as f64 / rate).rem_euclid(TAU) as f32,
                )
            })
            .collect()
    }

    const RATE: f64 = 250_000.0;

    fn search(half_span_hz: f64, gap_hz: f64, dominated: bool, artifact_hz: Option<f64>) -> Search {
        Search {
            half_span_hz,
            threshold_db: 8.0,
            gap_hz,
            dominated,
            artifact_hz,
        }
    }

    #[test]
    fn empty_air_reports_no_band() {
        let mut detector = Detector::new();
        let noise = complex_noise(0x51d3, 0.01, 32_768);
        let measured = detector.measure(&noise, RATE, &search(100_000.0, 8_000.0, false, None));
        assert!(measured.bands.is_empty());
    }

    #[test]
    fn a_carrier_is_found_at_its_offset() {
        let mut detector = Detector::new();
        let mut iq = tone(37_500.0, RATE, 32_768, 0.5);
        for (s, n) in iq.iter_mut().zip(complex_noise(0x7a11, 0.002, 32_768)) {
            *s += n;
        }
        let band = detector
            .measure(&iq, RATE, &search(100_000.0, 8_000.0, false, None))
            .bands
            .first()
            .copied()
            .expect("a carrier 40 dB out of the noise is a signal");
        assert!(
            (band.center_hz - 37_500.0).abs() < 500.0,
            "centre {} Hz",
            band.center_hz
        );
        assert!(
            band.bandwidth_hz < 2_000.0,
            "width {} Hz",
            band.bandwidth_hz
        );
        assert!(band.snr_db > 20.0, "snr {} dB", band.snr_db);
        assert!(band.carrier_db > 10.0, "carrier {} dB", band.carrier_db);
        assert!(band.flatness < 0.5, "flatness {}", band.flatness);
    }

    #[test]
    fn the_front_ends_dc_term_does_not_become_the_band() {
        let mut detector = Detector::new();
        let mut iq = tone(20_000.0, RATE, 32_768, 0.2);
        for (s, n) in iq.iter_mut().zip(complex_noise(0x2b91, 0.002, 32_768)) {
            *s += n + Complex::new(0.9, 0.6);
        }

        let fooled = detector
            .measure(&iq, RATE, &search(100_000.0, 8_000.0, false, None))
            .bands
            .first()
            .copied()
            .expect("the dc term alone reads as a band");
        assert!(
            fooled.center_hz.abs() < 2_000.0,
            "the unguarded detector was expected to sit on dc, got {} Hz",
            fooled.center_hz
        );

        let guarded = detector
            .measure(&iq, RATE, &search(100_000.0, 8_000.0, false, Some(0.0)))
            .bands
            .first()
            .copied()
            .expect("the real carrier is still there");
        assert!(
            (guarded.center_hz - 20_000.0).abs() < 500.0,
            "the guarded detector reported {} Hz, not the carrier at 20 kHz",
            guarded.center_hz
        );
    }

    #[test]
    fn a_guard_away_from_dc_leaves_a_carrier_on_dc_alone() {
        let mut detector = Detector::new();
        let mut iq = tone(0.0, RATE, 32_768, 0.5);
        for (s, n) in iq.iter_mut().zip(complex_noise(0x6c02, 0.002, 32_768)) {
            *s += n;
        }
        let band = detector
            .measure(
                &iq,
                RATE,
                &search(100_000.0, 8_000.0, false, Some(-60_000.0)),
            )
            .bands
            .first()
            .copied()
            .expect("a carrier on dc is a signal when the LO is elsewhere");
        assert!(band.center_hz.abs() < 500.0, "centre {} Hz", band.center_hz);
    }

    #[test]
    fn the_search_stays_inside_the_requested_slice() {
        let mut detector = Detector::new();
        let mut iq = tone(10_000.0, RATE, 32_768, 0.2);
        for ((s, loud), n) in iq
            .iter_mut()
            .zip(tone(60_000.0, RATE, 32_768, 0.8))
            .zip(complex_noise(0x3f0b, 0.002, 32_768))
        {
            *s += loud + n;
        }
        let band = detector
            .measure(&iq, RATE, &search(20_000.0, 8_000.0, false, None))
            .bands
            .first()
            .copied()
            .expect("the quiet carrier is inside the slice");
        assert!(
            (band.center_hz - 10_000.0).abs() < 500.0,
            "centre {} Hz",
            band.center_hz
        );
    }

    #[test]
    fn every_carrier_in_the_slice_is_listed_loudest_first() {
        let mut detector = Detector::new();
        let len = 32_768;
        let mut iq = tone(-70_000.0, RATE, len, 0.3);
        for ((s, loud), quiet) in iq
            .iter_mut()
            .zip(tone(20_000.0, RATE, len, 0.8))
            .zip(tone(55_000.0, RATE, len, 0.1))
        {
            *s += loud + quiet;
        }
        for (s, n) in iq.iter_mut().zip(complex_noise(0x6d21, 0.002, len)) {
            *s += n;
        }
        let survey = detector.measure(&iq, RATE, &search(100_000.0, 8_000.0, false, None));
        let centres: Vec<f64> = survey.bands.iter().map(|b| b.center_hz).collect();
        assert_eq!(centres.len(), 3, "{centres:?}");
        for (found, expected) in centres.iter().zip([20_000.0, -70_000.0, 55_000.0]) {
            assert!((found - expected).abs() < 500.0, "{centres:?}");
        }
    }

    #[test]
    fn a_loud_signals_skirt_is_not_a_second_signal() {
        let mut detector = Detector::new();
        let len = 32_768;
        let mut state = 0x4471u32;
        let mut phase = 0.0f64;
        let mut smoothed = 0.0f64;
        let mut iq: Vec<Complex<f32>> = (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let noise = f64::from(state) / f64::from(u32::MAX) - 0.5;
                smoothed += 0.3 * (noise - smoothed);
                phase += TAU * smoothed * 60_000.0 / RATE;
                Complex::from_polar(1.0, phase as f32)
            })
            .collect();
        for (s, n) in iq.iter_mut().zip(complex_noise(0x1b0c, 0.0005, len)) {
            *s += n;
        }
        let survey = detector.measure(&iq, RATE, &search(100_000.0, 8_000.0, false, None));
        assert_eq!(survey.bands.len(), 1, "{:?}", survey.bands);
    }

    #[test]
    fn on_a_crowded_band_two_narrow_neighbours_stay_apart() {
        let mut detector = Detector::new();
        let len = 32_768;
        let mut iq = tone(-3_500.0, RATE, len, 0.3);
        for (s, other) in iq.iter_mut().zip(tone(4_200.0, RATE, len, 0.3)) {
            *s += other;
        }
        for (s, n) in iq.iter_mut().zip(complex_noise(0x2ac1, 0.002, len)) {
            *s += n;
        }
        let survey = detector.measure(&iq, RATE, &search(24_000.0, 500.0, false, None));
        assert_eq!(survey.bands.len(), 2, "{:?}", survey.bands);
        assert!(
            survey.bands.iter().all(|b| b.bandwidth_hz < 1_500.0),
            "{:?}",
            survey.bands
        );
    }

    #[test]
    fn a_steady_signal_across_the_slice_is_one_signal_whatever_its_lines() {
        let mut detector = Detector::new();
        let len = 32_768;
        let mut phase = 0.0f64;
        let mut iq: Vec<Complex<f32>> = (0..len)
            .map(|k| {
                phase += TAU * 75_000.0 * (TAU * 1_000.0 * k as f64 / RATE).cos() / RATE;
                Complex::from_polar(0.5, phase.rem_euclid(TAU) as f32)
            })
            .collect();
        for (s, n) in iq.iter_mut().zip(complex_noise(0x77d1, 0.002, len)) {
            *s += n;
        }
        let survey = detector.measure(&iq, RATE, &search(100_000.0, 8_000.0, true, None));
        assert_eq!(survey.bands.len(), 1, "{:?}", survey.bands);
        assert!(
            survey.bands[0].bandwidth_hz > 150_000.0,
            "{:?}",
            survey.bands
        );
    }
}
