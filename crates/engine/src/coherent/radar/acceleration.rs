use num_complex::Complex;
use sdrmm_channels::RadarCorrelation;
use sdrmm_dsp::caf::{Caf, Surface};

use crate::gpu::{self, caf::GpuCaf};

pub(super) struct Correlation {
    cpu: Caf,
    gpu: Option<GpuCaf>,
}

impl Correlation {
    pub(super) fn new(cpu: Caf) -> Self {
        if cpu.cpi().saturating_mul(cpu.dopplers()) < 131_072 {
            return Self { cpu, gpu: None };
        }
        let gpu = match gpu::context()
            .map_err(str::to_owned)
            .and_then(|context| GpuCaf::new(context.clone(), &cpu))
        {
            Ok(gpu) => {
                tracing::info!(
                    cpi = cpu.cpi(),
                    dopplers = cpu.dopplers(),
                    "using GPU radar correlation"
                );
                Some(gpu)
            }
            Err(error) => {
                tracing::info!(%error, "using CPU radar correlation");
                None
            }
        };
        Self { cpu, gpu }
    }
    #[cfg(test)]
    pub(super) fn is_accelerated(&self) -> bool {
        self.gpu.is_some()
    }
}

impl RadarCorrelation for Correlation {
    fn compute(
        &mut self,
        reference: &[Complex<f32>],
        surveillance: &[Complex<f32>],
        out: &mut Surface,
    ) {
        if let Some(gpu) = &mut self.gpu {
            match gpu.compute(reference, surveillance, out) {
                Ok(()) => return,
                Err(error) => {
                    tracing::warn!(%error, "GPU radar failed; switching to CPU");
                    self.gpu = None;
                }
            }
        }
        self.cpu.compute(reference, surveillance, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a hardware GPU"]
    fn a_failed_gpu_frame_is_recomputed_and_disables_the_backend() {
        let cpu = Caf::new(4096, 128, 17, 2e6);
        let context = gpu::context().expect("hardware GPU required");
        let accelerated = GpuCaf::new(context.clone(), &cpu).unwrap();
        let mut correlation = Correlation {
            cpu,
            gpu: Some(accelerated),
        };
        let reference = crate::gpu::benchmarks::samples(4095, 0x12345678);
        let mut expected = Surface::default();
        Caf::new(4096, 128, 17, 2e6).compute(&reference, &reference, &mut expected);
        let mut actual = Surface::default();
        correlation.compute(&reference, &reference, &mut actual);
        assert!(correlation.gpu.is_none());
        assert_eq!(actual, expected);
        correlation.compute(&reference, &reference, &mut actual);
        assert_eq!(actual, expected);
    }
}
