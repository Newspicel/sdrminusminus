use std::time::Duration;

use sdrmm_channels::passive_radar::CafBackend;

use super::worker::Shared;

const FASTER_BY: f64 = 1.3;

pub(super) enum Offer {
    Gpu(Box<dyn CafBackend>),
    Cpu(Box<dyn CafBackend>),
}

pub(super) fn keeps_gpu(gpu: Duration, cpu: Duration) -> bool {
    gpu.as_secs_f64() * FASTER_BY <= cpu.as_secs_f64()
}

#[cfg(not(feature = "gpu-fft"))]
pub(super) fn offer(
    _stage: &sdrmm_channels::passive_radar::RadarPlan,
    cpu: Box<dyn CafBackend>,
    _shared: &std::sync::Arc<Shared>,
) -> Offer {
    tracing::info!("radar CAF stays on the CPU, this build has no GPU support");
    Offer::Cpu(cpu)
}

#[cfg(feature = "gpu-fft")]
pub(super) use backend::offer;
#[cfg(all(test, feature = "gpu-fft"))]
pub(super) use backend::{GpuBackend, decide};

#[cfg(feature = "gpu-fft")]
mod backend {
    use std::{
        sync::{Arc, atomic::Ordering},
        time::{Duration, Instant},
    };

    use sdrmm_channels::passive_radar::{
        CafBackend, CafError, CpiJob, CubeOut, RadarPlan as StagePlan,
    };

    use super::{super::cpu_backend, Offer, Shared, keeps_gpu};
    use crate::gpu::{self, caf::GpuCaf};

    const RACE_RUNS: usize = 2;

    pub(in crate::array::radar) struct GpuBackend {
        gpu: Option<GpuCaf>,
        cpu: Box<dyn CafBackend>,
        shared: Arc<Shared>,
    }

    pub(in crate::array::radar) fn offer(
        stage: &StagePlan,
        cpu: Box<dyn CafBackend>,
        shared: &Arc<Shared>,
    ) -> Offer {
        match GpuBackend::new(stage, cpu, shared) {
            Ok(mut backend) => {
                let race = backend.race(stage);
                decide(backend, race)
            }
            Err((cpu, reason)) => {
                tracing::info!(%reason, "radar CAF stays on the CPU");
                Offer::Cpu(cpu)
            }
        }
    }

    pub(in crate::array::radar) fn decide(
        backend: GpuBackend,
        race: Result<(Duration, Duration), String>,
    ) -> Offer {
        match race {
            Ok((gpu, cpu)) if keeps_gpu(gpu, cpu) => {
                tracing::info!(
                    gpu_ms = gpu.as_secs_f64() * 1e3,
                    cpu_ms = cpu.as_secs_f64() * 1e3,
                    "radar CAF runs on the GPU"
                );
                Offer::Gpu(Box::new(backend))
            }
            Ok((gpu, cpu)) => {
                tracing::info!(
                    gpu_ms = gpu.as_secs_f64() * 1e3,
                    cpu_ms = cpu.as_secs_f64() * 1e3,
                    "radar CAF stays on the CPU, the GPU is not faster"
                );
                Offer::Cpu(backend.cpu)
            }
            Err(reason) => {
                tracing::warn!(%reason, "radar GPU check failed, the CAF stays on the CPU");
                Offer::Cpu(backend.cpu)
            }
        }
    }

    impl GpuBackend {
        pub(in crate::array::radar) fn new(
            stage: &StagePlan,
            cpu: Box<dyn CafBackend>,
            shared: &Arc<Shared>,
        ) -> Result<Self, (Box<dyn CafBackend>, String)> {
            let built = gpu::context()
                .map_err(str::to_owned)
                .and_then(|context| GpuCaf::new(Arc::clone(context), stage));
            match built {
                Ok(gpu) => Ok(Self {
                    gpu: Some(gpu),
                    cpu,
                    shared: Arc::clone(shared),
                }),
                Err(reason) => Err((cpu, reason)),
            }
        }

        fn race(&mut self, stage: &StagePlan) -> Result<(Duration, Duration), String> {
            let job = Arc::new(CpiJob::new(stage.shape.lanes + 1, stage.shape.window()));
            let mut out = CubeOut::new(&stage.shape);
            let gpu = self.gpu.as_mut().ok_or_else(|| "no GPU".to_owned())?;
            let on_gpu = fastest(|| gpu.run(&job, &mut out))?;
            let (mut rival, _) = cpu_backend(stage, &Arc::new(Shared::default()))
                .map_err(|error| error.to_string())?;
            let on_cpu = fastest(|| {
                rival
                    .run_shared(&job, &mut out)
                    .map_err(|error| error.to_string())
            })?;
            Ok((on_gpu, on_cpu))
        }

        fn on_gpu(&mut self, job: &CpiJob, out: &mut CubeOut) -> bool {
            let Some(gpu) = self.gpu.as_mut() else {
                return false;
            };
            match gpu.run(job, out) {
                Ok(()) => true,
                Err(error) => {
                    self.gpu = None;
                    self.shared.gpu_failures.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(%error, "radar GPU failed, the CAF moves to the CPU");
                    false
                }
            }
        }

        #[cfg(test)]
        pub(in crate::array::radar) fn sabotage(&self) {
            if let Some(gpu) = &self.gpu {
                gpu.sabotage();
            }
        }
    }

    impl CafBackend for GpuBackend {
        fn run(&mut self, job: &CpiJob, out: &mut CubeOut) -> Result<(), CafError> {
            if self.on_gpu(job, out) {
                return Ok(());
            }
            self.cpu.run(job, out)
        }

        fn run_shared(&mut self, job: &Arc<CpiJob>, out: &mut CubeOut) -> Result<(), CafError> {
            if self.on_gpu(job, out) {
                return Ok(());
            }
            self.cpu.run_shared(job, out)
        }

        fn gpu(&self) -> bool {
            self.gpu.is_some()
        }

        fn threads(&self) -> u32 {
            if self.gpu.is_some() {
                0
            } else {
                self.cpu.threads()
            }
        }
    }

    fn fastest(mut run: impl FnMut() -> Result<(), String>) -> Result<Duration, String> {
        let mut best = Duration::MAX;
        for _ in 0..RACE_RUNS {
            let started = Instant::now();
            run()?;
            best = best.min(started.elapsed());
        }
        Ok(best)
    }
}

#[cfg(test)]
mod tests;
