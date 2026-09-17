use std::hint::black_box;

use num_complex::Complex;
use sdrmm_dsp::{
    FracResampler, NoiseFloor, SpectrumAnalyzer,
    cfar::{Detection, cluster},
};
use sdrmm_test_support::{CountingAlloc, assert_no_alloc, measure_throughput};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

#[test]
fn tuning_and_downconversion_reuse_storage_across_ragged_blocks_and_retunes() {
    let mut nco = sdrmm_dsp::Nco::new(187_501.25, 20_000_000.0);
    let mut samples = [Complex::new(1.0, 0.5); 4099];
    assert_no_alloc("nco", || {
        for chunk in samples.chunks_mut(17) {
            nco.mix(chunk);
        }
        nco.set_freq(-311.0, 48_000.0);
        nco.reset();
    });
    for output_rate in [48_000.0, 240_000.0] {
        let mut ddc = sdrmm_dsp::Ddc::new(20_000_000.0, output_rate, 187_501.25).unwrap();
        let mut out = Vec::new();
        for _ in 0..8 {
            ddc.process(&samples, &mut out);
        }
        assert_no_alloc("ddc", || {
            for size in [1, 17, 2048, 4099] {
                for chunk in samples.chunks(size) {
                    ddc.process(chunk, &mut out);
                }
                ddc.set_offset(-31_251.0);
                ddc.reset();
            }
        });
    }
}

#[test]
fn spectrum_processing_reuses_scratch_and_meets_the_display_budget() {
    let mut analyzer = SpectrumAnalyzer::new(4096);
    let input: Vec<_> = (0..4096)
        .map(|index| Complex::from_polar(0.5, std::f32::consts::TAU * 32.0 * index as f32 / 4096.0))
        .collect();
    let mut output = vec![0.0; input.len()];
    analyzer.power_db(&input, &mut output);
    assert_no_alloc("spectrum", || analyzer.power_db(&input, &mut output));
    assert_eq!(
        output
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(bin, _)| bin),
        Some(2080)
    );
    let msps = measure_throughput(30, input.len() as u64, || {
        analyzer.power_db(black_box(&input), black_box(&mut output))
    });
    assert!(
        msps > 1.0,
        "spectrum must handle eight 30 Hz displays: {msps} MS/s"
    );
}

#[test]
fn the_local_noise_floor_reuses_scratch_and_keeps_up_with_the_skimmer() {
    let mut noise = NoiseFloor::new(48, 8);
    let input: Vec<f32> = (0..4096)
        .map(|bin| -90.0 + (bin % 17) as f32 - (bin % 5) as f32)
        .collect();
    let mut floor = Vec::new();
    noise.estimate(&input, &mut floor);
    assert_no_alloc("noise floor", || noise.estimate(&input, &mut floor));
    let msps = measure_throughput(30, input.len() as u64, || {
        noise.estimate(black_box(&input), black_box(&mut floor))
    });
    assert!(
        msps > 1.0,
        "a skimmer needs a floor every 2048 samples: {msps} MS/s"
    );
}

#[test]
fn fractional_resampling_reuses_storage_and_exceeds_audio_realtime() {
    let mut resampler = FracResampler::new(48_000.0 / 240_000.0);
    let input = vec![Complex::new(0.5, 0.0); 2048];
    let mut output = Vec::with_capacity(input.len());
    for _ in 0..4 {
        resampler.process(&input, &mut output);
    }
    assert_no_alloc("resampler", || {
        for _ in 0..10 {
            resampler.process(&input, &mut output);
        }
    });
    assert!(output.iter().all(|sample| (sample.re - 0.5).abs() < 1e-5));
    let msps = measure_throughput(20, input.len() as u64, || {
        resampler.process(black_box(&input), black_box(&mut output))
    });
    assert!(
        msps > 0.48,
        "resampler must sustain twice realtime: {msps} MS/s"
    );
}

#[test]
fn clustering_is_deterministic_and_does_not_allocate_even_for_large_inputs() {
    let original: Vec<_> = (0..2048)
        .map(|index| Detection {
            range_bin: index / 4,
            doppler_bin: index % 8,
            snr_db: (index % 17) as f32,
        })
        .collect();
    let mut expected = original.clone();
    cluster(&mut expected);
    let mut input = original.clone();
    input.reverse();
    let allocation = input.as_ptr();
    assert_no_alloc("CFAR clustering", || cluster(&mut input));
    assert_eq!(input, expected);
    assert_eq!(input.as_ptr(), allocation);
    assert!(
        input
            .windows(2)
            .all(|pair| pair[0].snr_db >= pair[1].snr_db)
    );
}
