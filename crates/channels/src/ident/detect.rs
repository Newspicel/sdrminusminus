use num_complex::Complex;
use sdrmm_dsp::SpectrumAnalyzer;

pub(crate) const DETECT_FFT: usize = 4_096;

const MAX_SEGMENTS: usize = 24;

const GAP_HZ: f64 = 8_000.0;

const BAND_EDGE_DB: f32 = 20.0;

const SMOOTH_BINS: usize = 3;

fn from_db(db: f32) -> f32 {
    (db * 0.332_192_8).exp2()
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
pub(crate) struct Measurement {
    pub(crate) floor_db: f32,
    pub(crate) peak_db: f32,
    pub(crate) band: Option<Band>,
}

pub(crate) struct Detector {
    analyzer: SpectrumAnalyzer,
    segment_db: Vec<f32>,
    power: Vec<f32>,
    smoothed: Vec<f32>,
    scratch: Vec<f32>,
}

impl Detector {
    pub(crate) fn new() -> Self {
        Self {
            analyzer: SpectrumAnalyzer::new(DETECT_FFT),
            segment_db: vec![0.0; DETECT_FFT],
            power: vec![0.0; DETECT_FFT],
            smoothed: vec![0.0; DETECT_FFT],
            scratch: vec![0.0; DETECT_FFT],
        }
    }

    pub(crate) fn measure(
        &mut self,
        iq: &[Complex<f32>],
        rate: f64,
        half_span_hz: f64,
        threshold_db: f32,
    ) -> Measurement {
        let quiet = Measurement {
            floor_db: -200.0,
            peak_db: -200.0,
            band: None,
        };
        if iq.len() < DETECT_FFT || rate <= 0.0 {
            return quiet;
        }
        self.accumulate(iq);

        let bin_hz = rate / DETECT_FFT as f64;
        let center = DETECT_FFT / 2;
        let half_bins = ((half_span_hz / bin_hz).floor() as usize).clamp(1, center);
        let lo = center - half_bins;
        let hi = (center + half_bins).min(DETECT_FFT - 1);

        let floor = self.median_of(Series::Smoothed, lo, hi);
        let floor_db = 10.0 * (floor.max(f32::MIN_POSITIVE)).log10();
        let Some(peak) = (lo..=hi).max_by(|&a, &b| self.smoothed[a].total_cmp(&self.smoothed[b]))
        else {
            return quiet;
        };
        let peak_db = 10.0 * (self.smoothed[peak].max(f32::MIN_POSITIVE)).log10();
        if peak_db - floor_db < threshold_db {
            return Measurement {
                floor_db,
                peak_db,
                band: None,
            };
        }

        let edge = from_db((floor_db + (threshold_db * 0.5).max(3.0)).max(peak_db - BAND_EDGE_DB));
        let gap = ((GAP_HZ / bin_hz) as usize).max(1);
        let (start, end) = self.extent(peak, lo, hi, edge, gap);
        let bins = end - start + 1;

        let occupied: f32 = self.smoothed[start..=end].iter().sum();
        let noise = floor * bins as f32;
        let signal = (occupied - noise).max(f32::MIN_POSITIVE);
        let snr_db = 10.0 * (signal / noise.max(f32::MIN_POSITIVE)).log10();

        let raw_peak_db = 10.0 * self.power[peak].max(f32::MIN_POSITIVE).log10();
        let median_db = 10.0
            * self
                .median_of(Series::Raw, start, end)
                .max(f32::MIN_POSITIVE)
                .log10();
        let bin_index_hz = |i: usize| (i as f64 - center as f64) * bin_hz;

        Measurement {
            floor_db,
            peak_db,
            band: Some(Band {
                center_hz: self.centroid_hz(start, end, floor, bin_hz, center),
                bandwidth_hz: bins as f64 * bin_hz,
                snr_db,
                carrier_db: raw_peak_db - median_db,
                flatness: self.flatness(start, end),
                skew: self.skew(start, end, floor, bin_hz),
                peak_hz: bin_index_hz(peak),
            }),
        }
    }

    fn accumulate(&mut self, iq: &[Complex<f32>]) {
        let spare = iq.len() - DETECT_FFT;
        let segments = (spare / (DETECT_FFT / 2) + 1).min(MAX_SEGMENTS);
        let hop = if segments > 1 {
            spare / (segments - 1)
        } else {
            0
        };

        self.power.fill(0.0);
        for s in 0..segments {
            let start = s * hop;
            self.analyzer
                .power_db(&iq[start..start + DETECT_FFT], &mut self.segment_db);
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
        for i in 0..DETECT_FFT {
            let lo = i.saturating_sub(half);
            let hi = (i + half).min(DETECT_FFT - 1);
            let span = &self.power[lo..=hi];
            self.smoothed[i] = span.iter().sum::<f32>() / span.len() as f32;
        }
    }

    fn median_of(&mut self, series: Series, lo: usize, hi: usize) -> f32 {
        self.scratch.clear();
        self.scratch.extend_from_slice(match series {
            Series::Raw => &self.power[lo..=hi],
            Series::Smoothed => &self.smoothed[lo..=hi],
        });
        let mid = self.scratch.len() / 2;
        let (_, median, _) = self.scratch.select_nth_unstable_by(mid, f32::total_cmp);
        *median
    }

    fn extent(&self, peak: usize, lo: usize, hi: usize, edge: f32, gap: usize) -> (usize, usize) {
        let mut start = peak;
        let mut i = peak;
        while i > lo {
            i -= 1;
            if self.smoothed[i] >= edge {
                start = i;
            } else if start - i > gap {
                break;
            }
        }
        let mut end = peak;
        let mut j = peak;
        while j < hi {
            j += 1;
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
            .map(|k| Complex::from_polar(amp, (TAU * freq_hz * k as f64 / rate) as f32))
            .collect()
    }

    const RATE: f64 = 250_000.0;

    #[test]
    fn empty_air_reports_no_band() {
        let mut detector = Detector::new();
        let noise = complex_noise(0x51d3, 0.01, 32_768);
        let measured = detector.measure(&noise, RATE, 100_000.0, 8.0);
        assert!(measured.band.is_none());
    }

    #[test]
    fn a_carrier_is_found_at_its_offset() {
        let mut detector = Detector::new();
        let mut iq = tone(37_500.0, RATE, 32_768, 0.5);
        for (s, n) in iq.iter_mut().zip(complex_noise(0x7a11, 0.002, 32_768)) {
            *s += n;
        }
        let band = detector
            .measure(&iq, RATE, 100_000.0, 8.0)
            .band
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
            .measure(&iq, RATE, 20_000.0, 8.0)
            .band
            .expect("the quiet carrier is inside the slice");
        assert!(
            (band.center_hz - 10_000.0).abs() < 500.0,
            "centre {} Hz",
            band.center_hz
        );
    }
}
