use super::{Context, compute::*};

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
    ) -> Self {
        assert!(size >= 1024 && size.is_power_of_two());
        let twiddles: Vec<_> = (0..size / 2)
            .map(|index| {
                let angle = -std::f64::consts::TAU * index as f64 / size as f64;
                [angle.cos() as f32, angle.sin() as f32]
            })
            .collect();
        let twiddles = initialized(context, &twiddles);
        let shader = include_str!("fft.wgsl");
        let params = initialized(
            context,
            &[size as u32, 1024, u32::from(inverse), batches as u32],
        );
        let mut kernels = vec![Kernel::new(
            context,
            shader,
            "local_fft",
            &[data, &twiddles, &params],
            [size as u32 / 1024, batches as u32, 1],
        )];
        for stage in 11..=size.trailing_zeros() {
            let params = initialized(
                context,
                &[size as u32, 1 << stage, u32::from(inverse), batches as u32],
            );
            kernels.push(Kernel::new(
                context,
                shader,
                "global_fft",
                &[data, &twiddles, &params],
                [(size as u32 / 2).div_ceil(256), batches as u32, 1],
            ));
        }
        Self { kernels }
    }

    pub(crate) fn dispatch(&self, pass: &mut wgpu::ComputePass<'_>) {
        for kernel in &self.kernels {
            kernel.dispatch(pass);
        }
    }
}
