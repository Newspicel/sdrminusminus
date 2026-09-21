use super::*;

fn samples(size: usize, seed: u32) -> Vec<Complex<f32>> {
    let mut state = seed;
    (0..size)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            Complex::new(
                state as i32 as f32 / i32::MAX as f32,
                state.rotate_left(13) as i32 as f32 / i32::MAX as f32,
            )
        })
        .collect()
}

#[test]
#[ignore = "requires a GPU adapter"]
fn radar_matches_cpu_across_tiles_and_fractional_doppler() {
    let context = Arc::new(Context::new_for_test().expect("GPU adapter required"));
    let info = context.device.adapter_info();
    println!("adapter={},backend={:?}", info.name, info.backend);
    if cfg!(target_os = "macos") {
        assert_eq!(info.backend, wgpu::Backend::Metal);
    }
    for (cpi, ranges, dopplers) in [
        (4096, 127, 17),
        (4096, 256, 34),
        (5000, 129, 33),
        (65_536, 256, 129),
    ] {
        if info.device_type == wgpu::DeviceType::Cpu && cpi > 5000 {
            continue;
        }
        let reference = samples(cpi, 0x12345678);
        let surveillance: Vec<_> = (0..cpi)
            .map(|index| {
                if index < 37 {
                    Complex::default()
                } else {
                    reference[index - 37]
                        * Complex::from_polar(
                            0.75,
                            std::f32::consts::TAU * 3.0 * index as f32 / cpi as f32,
                        )
                }
            })
            .collect();
        let mut cpu = Caf::new(cpi, ranges, dopplers, 2_000_000.0);
        let mut gpu = GpuCaf::new(context.clone(), &cpu).unwrap();
        let mut expected = Surface::default();
        let mut actual = Surface::default();
        cpu.compute(&reference, &surveillance, &mut expected);
        gpu.compute(&reference, &surveillance, &mut actual).unwrap();
        assert_eq!(actual.ranges, expected.ranges);
        assert_eq!(actual.dopplers, expected.dopplers);
        assert_eq!(actual.range_step_s, expected.range_step_s);
        assert_eq!(actual.doppler_step_hz, expected.doppler_step_hz);
        let peak = expected.power.iter().copied().fold(0.0f32, f32::max);
        let error = expected
            .power
            .iter()
            .zip(&actual.power)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            error < peak * 2e-4,
            "cpi={cpi} ranges={ranges} dopplers={dopplers} error={error} peak={peak}"
        );
        let peak_at = |surface: &Surface| {
            surface
                .power
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .unwrap()
                .0
        };
        assert_eq!(peak_at(&actual), peak_at(&expected));
        let noise = samples(cpi, 0x87654321);
        cpu.compute(&reference, &noise, &mut expected);
        gpu.compute(&reference, &noise, &mut actual).unwrap();
        let peak = expected.power.iter().copied().fold(0.0f32, f32::max);
        for (actual, expected) in actual.power.iter().zip(&expected.power) {
            assert!((actual - expected).abs() < peak * 2e-4);
        }
        assert!(
            gpu.compute(&reference[..cpi - 1], &surveillance, &mut actual)
                .is_err()
        );
        gpu.compute(
            &vec![Complex::default(); cpi],
            &vec![Complex::default(); cpi],
            &mut actual,
        )
        .unwrap();
        assert!(actual.power.iter().all(|&value| value == 0.0));
    }
}
