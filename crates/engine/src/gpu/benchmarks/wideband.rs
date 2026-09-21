use super::{super::*, compute::*};

pub(super) struct Wideband {
    context: Arc<Context>,
    input: wgpu::Buffer,
    output: wgpu::Buffer,
    params: wgpu::Buffer,
    kernel: Kernel,
    readback: Readback,
    upload: Vec<[f32; 2]>,
    phase: usize,
    size: usize,
}

impl Wideband {
    pub(super) fn new(context: Arc<Context>, size: usize) -> Self {
        let input = storage(&context, (size + 68) * 2);
        let output = storage(&context, size.div_ceil(5) * 13 * 2);
        let taps = initialized(&context, &sdrmm_dsp::design_lowpass(69, 0.1));
        let twiddles: Vec<_> = (0..25)
            .map(|index| {
                let angle = std::f64::consts::TAU * index as f64 / 25.0;
                [angle.cos() as f32, angle.sin() as f32]
            })
            .collect();
        let twiddles = initialized(&context, &twiddles);
        let params = initialized(&context, &[size.div_ceil(5) as u32, 0u32]);
        let kernel = Kernel::new(
            &context,
            include_str!("wideband.wgsl"),
            "filter_bank",
            &[&input, &output, &taps, &twiddles, &params],
            [size.div_ceil(5) as u32, 1, 1],
        );
        let readback = Readback::new(&context, size.div_ceil(5) * 13 * 2);
        Self {
            context,
            input,
            output,
            params,
            kernel,
            readback,
            upload: vec![[0.0; 2]; 68],
            phase: 0,
            size,
        }
    }

    pub(super) fn process(&mut self, input: &[Complex<f32>], output: &mut [f32]) -> usize {
        assert_eq!(input.len(), self.size);
        self.upload.extend(input.iter().map(|v| [v.re, v.im]));
        let frames = (self.upload.len() - 68).div_ceil(5);
        self.context
            .queue
            .write_buffer(&self.input, 0, bytemuck::cast_slice(&self.upload));
        self.context.queue.write_buffer(
            &self.params,
            0,
            bytemuck::cast_slice(&[frames as u32, self.phase as u32]),
        );
        let mut encoder = self
            .context
            .device
            .create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            self.kernel.dispatch(&mut pass);
        }
        self.readback
            .finish(&self.context, encoder, &self.output, output)
            .unwrap();
        self.phase = (self.phase + frames) % 5;
        self.upload.drain(..frames * 5);
        frames
    }
}
