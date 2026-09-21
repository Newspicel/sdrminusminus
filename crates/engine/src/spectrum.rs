#[cfg(feature = "gpu-fft")]
use std::sync::Arc;

use num_complex::Complex;
use sdrmm_dsp::SpectrumAnalyzer as CpuSpectrumAnalyzer;

#[cfg(any(feature = "gpu-fft", test))]
const GPU_MIN_FFT_SIZE: usize = 65_536;

pub(crate) struct SpectrumPlan {
    size: usize,
    #[cfg(feature = "gpu-fft")]
    gpu: Option<Arc<gpu::Context>>,
}

impl SpectrumPlan {
    pub(crate) fn new(size: usize, lanes: usize) -> Self {
        #[cfg(feature = "gpu-fft")]
        {
            let gpu = if should_use_gpu(size, lanes) {
                match gpu::context() {
                    Ok(context) if gpu::spectrum_is_faster(context.clone(), size) => {
                        tracing::info!(
                            adapter = context.adapter_name(),
                            fft_size = size,
                            lanes,
                            "using GPU spectrum FFT"
                        );
                        Some(context.clone())
                    }
                    Ok(_) => None,
                    Err(error) => {
                        tracing::warn!(%error, "GPU spectrum FFT unavailable; using CPU");
                        None
                    }
                }
            } else {
                None
            };
            Self { size, gpu }
        }

        #[cfg(not(feature = "gpu-fft"))]
        {
            let _ = lanes;
            Self { size }
        }
    }

    pub(crate) fn analyzer(&self) -> SpectrumAnalyzer {
        #[cfg(feature = "gpu-fft")]
        if let Some(context) = &self.gpu {
            match gpu::Analyzer::new(context.clone(), self.size) {
                Ok(analyzer) => {
                    return SpectrumAnalyzer::Gpu {
                        analyzer: Box::new(analyzer),
                        fallback: CpuSpectrumAnalyzer::new(self.size),
                        failed: false,
                    };
                }
                Err(error) => {
                    tracing::warn!(%error, "could not create GPU spectrum FFT; using CPU");
                }
            }
        }

        SpectrumAnalyzer::Cpu(CpuSpectrumAnalyzer::new(self.size))
    }
}

pub(crate) enum SpectrumAnalyzer {
    Cpu(CpuSpectrumAnalyzer),
    #[cfg(feature = "gpu-fft")]
    Gpu {
        analyzer: Box<gpu::Analyzer>,
        fallback: CpuSpectrumAnalyzer,
        failed: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SpectrumFrame {
    pub(crate) timestamp: u64,
    pub(crate) center_hz: f64,
    pub(crate) span_hz: f32,
}

impl SpectrumAnalyzer {
    pub(crate) fn power_db(
        &mut self,
        input: &[Complex<f32>],
        out: &mut [f32],
        frame: SpectrumFrame,
    ) -> Option<SpectrumFrame> {
        assert_eq!(
            input.len(),
            out.len(),
            "spectrum input/output length mismatch"
        );
        match self {
            Self::Cpu(analyzer) => {
                analyzer.power_db(input, out);
                Some(frame)
            }
            #[cfg(feature = "gpu-fft")]
            Self::Gpu {
                analyzer,
                fallback,
                failed,
            } => {
                if !*failed {
                    match analyzer.power_db(input, out, frame) {
                        Ok(completed) => return completed,
                        Err(error) => {
                            tracing::warn!(%error, "GPU spectrum FFT failed; switching to CPU");
                            *failed = true;
                        }
                    }
                }
                fallback.power_db(input, out);
                Some(frame)
            }
        }
    }
}

#[cfg(any(feature = "gpu-fft", test))]
fn should_use_gpu(size: usize, _lanes: usize) -> bool {
    size.is_power_of_two() && size >= GPU_MIN_FFT_SIZE
}

#[cfg(feature = "gpu-fft")]
use crate::gpu;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_selection_only_targets_expensive_workloads() {
        assert!(!should_use_gpu(4096, 1));
        assert!(should_use_gpu(GPU_MIN_FFT_SIZE, 1));
        assert!(!should_use_gpu(4096, 4));
        assert!(!should_use_gpu(4096, 32));
        assert!(!should_use_gpu(16_384, 4));
        assert!(!should_use_gpu(12_345, 4));
    }

    #[cfg(feature = "gpu-fft")]
    #[test]
    #[ignore = "CI runs this explicitly with a guaranteed headless adapter"]
    fn gpu_matches_cpu_when_a_hardware_adapter_is_available() {
        let context = match gpu::Context::new_for_test().map(Arc::new) {
            Ok(context) => context,
            Err(error) if std::env::var_os("SDRMM_REQUIRE_GPU_FFT_TEST").is_some() => {
                panic!("required GPU FFT test adapter is unavailable: {error}")
            }
            Err(_) => return,
        };
        let size = 1024;
        let mut noise = 0x1234_5678_u32;
        let input: Vec<_> = (0..size)
            .map(|index| {
                noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let phase = std::f32::consts::TAU * 117.0 * index as f32 / size as f32;
                let dither = (noise as f32 / u32::MAX as f32 - 0.5) * 0.02;
                Complex::from_polar(0.75, phase) + Complex::new(dither, -0.5 * dither)
            })
            .collect();
        let mut expected = vec![0.0; size];
        CpuSpectrumAnalyzer::new(size).power_db(&input, &mut expected);
        let mut actual = vec![0.0; size];
        let mut analyzer = gpu::Analyzer::new(context, size).unwrap();
        let frame = SpectrumFrame {
            timestamp: 1,
            center_hz: 100_000_000.0,
            span_hz: 2_400_000.0,
        };
        assert_eq!(analyzer.power_db(&input, &mut actual, frame).unwrap(), None);
        let deadline = std::time::Instant::now() + gpu::GPU_FRAME_BUDGET.saturating_mul(3);
        while analyzer.power_db(&input, &mut actual, frame).unwrap() != Some(frame) {
            assert!(
                std::time::Instant::now() < deadline,
                "GPU spectrum worker did not return a frame"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        for (bin, (&cpu, &gpu)) in expected.iter().zip(&actual).enumerate() {
            assert!(
                (cpu - gpu).abs() < 0.1,
                "bin {bin}: CPU {cpu} dB, GPU {gpu} dB"
            );
        }
        let mut fallback = SpectrumAnalyzer::Gpu {
            analyzer: Box::new(analyzer),
            fallback: CpuSpectrumAnalyzer::new(size),
            failed: true,
        };
        sdrmm_test_support::assert_no_alloc("spectrum CPU fallback", || {
            assert_eq!(fallback.power_db(&input, &mut actual, frame), Some(frame));
        });
        assert_eq!(actual, expected);
    }
}
