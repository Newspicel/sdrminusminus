use std::f64::consts::TAU;

use num_complex::Complex;

use super::*;
use crate::array_processor::bench::{Bench, block, noise};

const RATE: f64 = 2_400_000.0;
const BINS: usize = 4_096;
const GROUP_DELAY: f64 = 64.0;
const OFFSET_HZ: f64 = 151_000.0;
const BAND_HZ: f64 = 20_000.0;
const BLOCK: usize = 16_384;
const BLOCKS: usize = 16;
const TONE_STEP_HZ: f64 = 1_700.0;

struct LaneError {
    delay: f64,
    phase: f64,
    gain: f64,
}

const LANES: [LaneError; 4] = [
    LaneError {
        delay: 0.0,
        phase: 0.0,
        gain: 1.0,
    },
    LaneError {
        delay: 0.3,
        phase: 0.698,
        gain: 0.8,
    },
    LaneError {
        delay: -0.45,
        phase: -1.92,
        gain: 1.25,
    },
    LaneError {
        delay: 0.7,
        phase: 2.97,
        gain: 1.0,
    },
];

fn tones(len: usize, error: &LaneError) -> Vec<Complex<f32>> {
    (0..len)
        .map(|n| {
            (-4..=4)
                .map(|k: i32| {
                    let hz = OFFSET_HZ + f64::from(k) * TONE_STEP_HZ;
                    let angle = TAU * hz * (n as f64 - error.delay) / RATE
                        + 0.7 * f64::from(k * k)
                        + error.phase;
                    Complex::from_polar(error.gain, angle)
                })
                .sum::<Complex<f64>>()
        })
        .map(|sample| Complex::new(sample.re as f32, sample.im as f32) * 0.1)
        .collect()
}

fn bin_hz(bin: usize) -> f64 {
    let signed = if bin < BINS / 2 {
        bin as f64
    } else {
        bin as f64 - BINS as f64
    };
    signed * RATE / BINS as f64
}

fn correction_spectra() -> Vec<Vec<Complex<f32>>> {
    LANES
        .iter()
        .map(|error| {
            (0..BINS)
                .map(|bin| {
                    let hz = bin_hz(bin);
                    let angle = -error.phase + TAU * hz * (error.delay - GROUP_DELAY) / RATE;
                    let factor = Complex::from_polar(1.0 / error.gain, angle);
                    Complex::new(factor.re as f32, factor.im as f32)
                })
                .collect()
        })
        .collect()
}

fn covariance(
    lanes: &[Vec<Complex<f32>>],
    corrected: bool,
    spectra: &[Vec<Complex<f32>>],
) -> [[Complex<f64>; 4]; 4] {
    let mut band = LaneBand::new(4, RATE, OFFSET_HZ, Some(BAND_HZ), BLOCK).expect("band");
    let mut sum = [[Complex::new(0.0, 0.0); 4]; 4];
    for index in 0..BLOCKS {
        let range = index * BLOCK..(index + 1) * BLOCK;
        let views: Vec<&[Complex<f32>]> = lanes.iter().map(|lane| &lane[range.clone()]).collect();
        let mut input = block(&views, 0);
        input.corrected = corrected;
        input.correction = CorrectionView::new(1, RATE, spectra);
        let mut out: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
        let len = band.process(&input, &mut out);
        if index < 2 {
            continue;
        }
        for (a, row) in sum.iter_mut().enumerate() {
            for (b, cell) in row.iter_mut().enumerate() {
                for (x, y) in out[a][..len].iter().zip(&out[b][..len]) {
                    let product = x * y.conj();
                    *cell += Complex::new(f64::from(product.re), f64::from(product.im));
                }
            }
        }
    }
    sum
}

#[test]
fn lane_band_applies_the_narrowband_correction() {
    let len = BLOCK * BLOCKS;
    let raw: Vec<_> = LANES.iter().map(|error| tones(len, error)).collect();
    let clean = LaneError {
        delay: 0.0,
        phase: 0.0,
        gain: 1.0,
    };
    let aligned = vec![tones(len, &clean); LANES.len()];
    let spectra = correction_spectra();
    let banded = covariance(&raw, false, &spectra);
    let reference = covariance(&aligned, true, &[]);
    for lane in 1..4 {
        let phase = (banded[0][lane] * reference[0][lane].conj())
            .arg()
            .to_degrees();
        assert!(phase.abs() < 0.1, "lane {lane}: {phase} deg");
        let power_db = 10.0 * (banded[lane][lane].re / banded[0][0].re).log10();
        assert!(power_db.abs() < 0.1, "lane {lane}: {power_db} dB");
    }
    let uncorrected = covariance(&raw, true, &[]);
    let off = (uncorrected[0][1] * reference[0][1].conj())
        .arg()
        .to_degrees();
    assert!(
        off.abs() > 30.0,
        "the test lanes must start misaligned: {off}"
    );
}

#[test]
fn lane_band_passes_through_without_bandwidth() {
    let lanes: Vec<_> = (0..3).map(|seed| noise(1_000, seed + 1)).collect();
    let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
    let mut band = LaneBand::new(3, RATE, 25_000.0, None, 1_024).expect("band");
    assert!((band.output_rate() - RATE).abs() < f64::EPSILON);
    assert!((band.independent_fraction() - 1.0).abs() < f64::EPSILON);
    assert!(band.center_offset_hz().abs() < f64::EPSILON);
    let input = block(&views, 0);
    let mut out: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
    assert_eq!(band.process(&input, &mut out), 1_000);
    for (view, lane) in out.iter().zip(&views) {
        assert_eq!(view.as_ptr(), lane.as_ptr());
    }
    let mut uncorrected = block(&views, 0);
    uncorrected.corrected = false;
    let mut copied: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
    assert_eq!(band.process(&uncorrected, &mut copied), 1_000);
    for (view, lane) in copied.iter().zip(&views) {
        assert_eq!(view, lane);
    }
}

#[test]
fn lane_band_decimates_to_its_band() {
    let band = LaneBand::new(2, RATE, OFFSET_HZ, Some(BAND_HZ), 4_096).expect("band");
    assert_eq!(decimation(RATE, Some(BAND_HZ)), 96);
    assert!((band.output_rate() - 25_000.0).abs() < 1e-9);
    assert!((band.independent_fraction() - 0.8).abs() < 1e-12);
    assert!((band.center_offset_hz() - OFFSET_HZ).abs() < f64::EPSILON);
    let bench = Bench::together("band", 2, RATE, 16_384);
    let format = band_lane_format(&bench.ctx(), OFFSET_HZ, Some(BAND_HZ));
    assert!((format.center_hz - bench.center_hz - OFFSET_HZ).abs() < 1e-6);
    assert!((format.sample_rate - 25_000.0).abs() < 1e-9);
    assert_eq!(format.capacity, 171 + BAND_MARGIN);
    let full = band_lane_format(&bench.ctx(), OFFSET_HZ, None);
    assert!((full.center_hz - bench.center_hz).abs() < f64::EPSILON);
    assert_eq!(full.capacity, 16_384 + BAND_MARGIN);
}

#[test]
fn lane_band_refuses_impossible_settings() {
    let refused = |result: Result<LaneBand, ChannelError>| result.err().map(|e| e.to_string());
    assert_eq!(
        refused(LaneBand::new(0, RATE, 0.0, None, 16)),
        Some("Too few elements".to_owned())
    );
    assert_eq!(
        refused(LaneBand::new(MAX_LANES + 1, RATE, 0.0, None, 16)),
        Some("Too many elements".to_owned())
    );
    assert_eq!(
        refused(LaneBand::new(2, RATE, f64::INFINITY, None, 16)),
        Some("Offset out of range".to_owned())
    );
    assert_eq!(
        refused(LaneBand::new(2, f64::NAN, 0.0, None, 16)),
        Some("Rate out of range".to_owned())
    );
    assert_eq!(
        refused(LaneBand::new(2, RATE, 0.0, Some(0.0), 16)),
        Some("Bandwidth out of range".to_owned())
    );
}

#[test]
fn a_new_correction_generation_refreshes_the_factors() {
    let lane = vec![Complex::new(1.0, 0.0); 64];
    let views: [&[Complex<f32>]; 2] = [&lane, &lane];
    let turned = |angle: f32| {
        vec![
            vec![Complex::new(1.0, 0.0); 8],
            vec![Complex::from_polar(1.0, angle); 8],
        ]
    };
    let (first, second) = (turned(0.5), turned(-1.0));
    let mut band = LaneBand::new(2, RATE, 0.0, None, 64).expect("band");
    for (generation, spectra, angle) in [(1, &first, 0.5), (1, &second, 0.5), (2, &second, -1.0)] {
        let mut input = block(&views, 0);
        input.corrected = false;
        input.correction = CorrectionView::new(generation, RATE, spectra);
        let mut out: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
        band.process(&input, &mut out);
        assert!(
            (out[1][0].arg() - angle).abs() < 1e-6,
            "generation {generation}"
        );
    }
}

#[test]
fn a_picked_band_reads_its_lanes_and_their_corrections() {
    let lanes: Vec<Vec<Complex<f32>>> = (0..4)
        .map(|lane| vec![Complex::new(lane as f32 + 1.0, 0.0); 32])
        .collect();
    let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
    let spectra: Vec<Vec<Complex<f32>>> = (0..4)
        .map(|lane| vec![Complex::from_polar(1.0, 0.25 * lane as f32); 8])
        .collect();
    let mut band = LaneBand::picked(4, &[3, 1], RATE, 0.0, None, 64).expect("band");
    assert_eq!(band.lanes(), 2);
    let mut out: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
    assert_eq!(band.process(&block(&views, 0), &mut out), 32);
    assert_eq!((out[0][0].re, out[1][0].re), (4.0, 2.0));
    let mut input = block(&views, 0);
    input.corrected = false;
    input.correction = CorrectionView::new(1, RATE, &spectra);
    let mut corrected: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
    band.process(&input, &mut corrected);
    assert!((corrected[0][0].arg() - 0.75).abs() < 1e-6);
    assert!((corrected[1][0].arg() - 0.25).abs() < 1e-6);
    let two: Vec<&[Complex<f32>]> = views[..2].to_vec();
    let mut none: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
    assert_eq!(band.process(&block(&two, 0), &mut none), 0);
    assert_eq!(
        LaneBand::picked(2, &[0, 2], RATE, 0.0, None, 16)
            .err()
            .map(|error| error.to_string()),
        Some("Lane out of range".to_owned())
    );
}
