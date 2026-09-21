use super::{super::*, compute::*};

pub(super) struct Spectrum {
    context: Arc<Context>,
    input: wgpu::Buffer,
    output: wgpu::Buffer,
    prepare: Kernel,
    fft: FftBatch,
    power: Kernel,
    readback: Readback,
    upload: Vec<[f32; 2]>,
}

impl Spectrum {
    pub(super) fn new(context: Arc<Context>, size: usize, batches: usize) -> Self {
        let input = storage(&context, size * batches * 2);
        let data = storage(&context, size * batches * 2);
        let output = storage(&context, size * batches);
        let window = hann(size);
        let inv_gain = 1.0 / coherent_gain(&window).max(f32::MIN_POSITIVE);
        let window = initialized(&context, &window);
        let params = initialized(
            &context,
            &[size as u32, size.trailing_zeros(), inv_gain.to_bits()],
        );
        let prepare = Kernel::new(
            &context,
            include_str!("spectrum.wgsl"),
            "prepare",
            &[&input, &data, &params, &window],
            [size as u32 / 256, batches as u32, 1],
        );
        let fft = FftBatch::new(&context, &data, size, batches, false);
        let power = Kernel::new(
            &context,
            include_str!("spectrum.wgsl"),
            "power",
            &[&data, &output, &params],
            [size as u32 / 512, batches as u32, 1],
        );
        let readback = Readback::new(&context, size * batches);
        Self {
            context,
            input,
            output,
            prepare,
            fft,
            power,
            readback,
            upload: vec![[0.0; 2]; size * batches],
        }
    }

    pub(super) fn compute(&mut self, input: &[Complex<f32>], output: &mut [f32]) {
        for (target, source) in self.upload.iter_mut().zip(input) {
            *target = [source.re, source.im];
        }
        self.context
            .queue
            .write_buffer(&self.input, 0, bytemuck::cast_slice(&self.upload));
        let mut encoder = self
            .context
            .device
            .create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            self.prepare.dispatch(&mut pass);
            self.fft.dispatch(&mut pass);
            self.power.dispatch(&mut pass);
        }
        self.readback
            .finish(&self.context, encoder, &self.output, output)
            .unwrap();
    }
}
