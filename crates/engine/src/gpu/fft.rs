use super::{Context, compute::*};

pub(crate) const MIN_SIZE: usize = 16;
pub(crate) const MAX_SIZE: usize = 1 << 22;
const TILE: u64 = 1024;
const THREADS: u64 = 256;
const LOCAL_STAGES: u32 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Transforms {
    pub(crate) size: usize,
    pub(crate) offset: usize,
    pub(crate) batches: usize,
    pub(crate) inverse: bool,
}

pub(crate) struct FftBatch {
    kernels: Vec<Kernel>,
}

impl FftBatch {
    pub(crate) fn new(
        context: &Context,
        data: &wgpu::Buffer,
        size: usize,
        batches: usize,
        inverse: bool,
    ) -> Result<Self, String> {
        Self::over(
            context,
            data,
            Transforms {
                size,
                offset: 0,
                batches,
                inverse,
            },
        )
    }

    pub(crate) fn over(
        context: &Context,
        data: &wgpu::Buffer,
        plan: Transforms,
    ) -> Result<Self, String> {
        check_size(plan.size)?;
        let elements = plan
            .size
            .checked_mul(plan.batches.max(1))
            .ok_or_else(|| "GPU FFT batch overflow".to_owned())?;
        let narrow = |value: usize| {
            u32::try_from(value).map_err(|_| format!("GPU FFT index {value} exceeds 32 bits"))
        };
        let (size, count) = (narrow(plan.size)?, narrow(elements)?);
        narrow(plan.offset.saturating_add(elements))?;
        let offset = narrow(plan.offset)?;
        let twiddles = initialized(context, &twiddles(plan.size));
        let module = module(context, include_str!("fft.wgsl"), "fft");
        let local = compile(context, &module, "local_fft");
        let global = compile(context, &module, "global_fft");
        let inverse = u32::from(plan.inverse);
        let bound = |pipeline: &wgpu::ComputePipeline, span: u32, groups: [u32; 3]| {
            let params = initialized(context, &[size, span, inverse, count, offset]);
            Kernel::bind(
                context,
                pipeline,
                &[(0, data), (1, &twiddles), (2, &params)],
                groups,
            )
        };
        let elements = elements as u64;
        let mut kernels = vec![bound(&local, 0, grid(elements, TILE)?)];
        for stage in LOCAL_STAGES + 1..=plan.size.trailing_zeros() {
            kernels.push(bound(&global, 1 << stage, grid(elements / 2, THREADS)?));
        }
        Ok(Self { kernels })
    }

    pub(crate) fn dispatch(&self, pass: &mut wgpu::ComputePass<'_>) {
        for kernel in &self.kernels {
            kernel.dispatch(pass);
        }
    }
}

pub(crate) fn check_size(size: usize) -> Result<(), String> {
    if size.is_power_of_two() && (MIN_SIZE..=MAX_SIZE).contains(&size) {
        Ok(())
    } else {
        Err(format!(
            "GPU FFT size {size} is not a power of two from {MIN_SIZE} to {MAX_SIZE}"
        ))
    }
}

fn twiddles(size: usize) -> Vec<[f32; 2]> {
    (0..size / 2)
        .map(|index| {
            let angle = -std::f64::consts::TAU * index as f64 / size as f64;
            [angle.cos() as f32, angle.sin() as f32]
        })
        .collect()
}

#[cfg(test)]
mod tests;
