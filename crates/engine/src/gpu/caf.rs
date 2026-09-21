use num_complex::Complex;
use sdrmm_dsp::caf::{Caf, Surface};

use super::{compute::*, fft::FftBatch, *};

const MAX_WORK_BYTES: usize = 128 * 1024 * 1024;
const MAX_BATCH: usize = 16;

struct Tile {
    prepare: Kernel,
    correlate: Option<Kernel>,
    power: Kernel,
}

pub(crate) struct GpuCaf {
    context: Arc<Context>,
    cpi: usize,
    ranges: usize,
    dopplers: usize,
    doppler_step_hz: f32,
    range_step_s: f32,
    input: wgpu::Buffer,
    output: wgpu::Buffer,
    prepare: Kernel,
    forward: FftBatch,
    mixed_forward: Option<FftBatch>,
    inverse: FftBatch,
    tiles: Vec<Tile>,
    readback: Readback,
    upload: Vec<[f32; 2]>,
}

impl GpuCaf {
    pub(crate) fn new(context: Arc<Context>, cpu: &Caf) -> Result<Self, String> {
        let cpi = cpu.cpi();
        let ranges = cpu.ranges();
        let dopplers = cpu.dopplers();
        let size = cpi
            .checked_add(ranges)
            .and_then(usize::checked_next_power_of_two)
            .ok_or_else(|| "radar FFT size overflow".to_owned())?;
        if size < 1024 || cpi < 4096 || dopplers < 9 {
            return Err("radar workload is too small for GPU offload".to_owned());
        }
        let batch = (MAX_WORK_BYTES / size / 16).min(MAX_BATCH).min(dopplers);
        if batch == 0 {
            return Err("radar exceeds the GPU memory budget".to_owned());
        }
        validate_shape(&context, size)?;
        let limits = context.device.limits();
        let cells = ranges
            .checked_mul(dopplers)
            .ok_or_else(|| "radar surface size overflow".to_owned())?;
        for bytes in [
            cpi as u64 * 16,
            size as u64 * 16,
            size as u64 * batch as u64 * 8,
            cells as u64 * 4,
        ] {
            if bytes > limits.max_storage_buffer_binding_size || bytes > limits.max_buffer_size {
                return Err("radar exceeds the adapter buffer limit".to_owned());
            }
        }
        if ranges.div_ceil(256) > limits.max_compute_workgroups_per_dimension as usize {
            return Err("radar exceeds the adapter dispatch limit".to_owned());
        }
        let validation = context
            .device
            .push_error_scope(wgpu::ErrorFilter::Validation);
        let memory = context
            .device
            .push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let result = Self::build(context.clone(), cpu, size, batch);
        let memory_error = pollster::block_on(memory.pop());
        let validation_error = pollster::block_on(validation.pop());
        if let Some(error) = memory_error.or(validation_error) {
            return Err(error.to_string());
        }
        Ok(result)
    }

    fn build(context: Arc<Context>, cpu: &Caf, size: usize, batch: usize) -> Self {
        let (cpi, ranges, dopplers) = (cpu.cpi(), cpu.ranges(), cpu.dopplers());
        let input = storage(&context, cpi * 4);
        let spectra = storage(&context, size * 4);
        let work = storage(&context, size * batch * 2);
        let inverse_data = storage(&context, size * batch * 2);
        let output = storage(&context, ranges * dopplers);
        let params = initialized(
            &context,
            &[
                cpi as u32,
                size as u32,
                ranges as u32,
                dopplers as u32,
                size.trailing_zeros(),
                0,
                batch as u32,
                0,
            ],
        );
        let shader = include_str!("caf.wgsl");
        let prepare = Kernel::new(
            &context,
            shader,
            "prepare",
            &[&input, &spectra, &params],
            [size as u32 / 256, 2, 1],
        );
        let forward = FftBatch::new(&context, &spectra, size, 2, false);
        let fractional = !size.is_multiple_of(cpi)
            || (dopplers.is_multiple_of(2) && !(size / cpi).is_multiple_of(2));
        let mixed_forward = fractional.then(|| FftBatch::new(&context, &work, size, batch, false));
        let inverse = FftBatch::new(&context, &inverse_data, size, batch, true);
        let mut tiles = Vec::with_capacity(dopplers.div_ceil(batch));
        for row in (0..dopplers).step_by(batch) {
            let rows = batch.min(dopplers - row);
            let params = initialized(
                &context,
                &[
                    cpi as u32,
                    size as u32,
                    ranges as u32,
                    dopplers as u32,
                    size.trailing_zeros(),
                    row as u32,
                    rows as u32,
                    0,
                ],
            );
            let prepare = if fractional {
                Kernel::new(
                    &context,
                    shader,
                    "mix",
                    &[&input, &work, &params],
                    [size as u32 / 256, batch as u32, 1],
                )
            } else {
                Kernel::new(
                    &context,
                    shader,
                    "correlate",
                    &[&spectra, &inverse_data, &params],
                    [size as u32 / 256, batch as u32, 1],
                )
            };
            let correlate = fractional.then(|| {
                Kernel::new(
                    &context,
                    include_str!("caf_product.wgsl"),
                    "product",
                    &[&spectra, &work, &inverse_data, &params],
                    [size as u32 / 256, batch as u32, 1],
                )
            });
            let power = Kernel::new(
                &context,
                include_str!("caf_power.wgsl"),
                "power",
                &[&inverse_data, &output, &params],
                [ranges.div_ceil(256) as u32, rows as u32, 1],
            );
            tiles.push(Tile {
                prepare,
                correlate,
                power,
            });
        }
        let readback = Readback::new(&context, ranges * dopplers);
        Self {
            context,
            cpi,
            ranges,
            dopplers,
            doppler_step_hz: cpu.doppler_step_hz(),
            range_step_s: cpu.range_step_s(),
            input,
            output,
            prepare,
            forward,
            mixed_forward,
            inverse,
            tiles,
            readback,
            upload: vec![[0.0; 2]; cpi * 2],
        }
    }

    pub(crate) fn compute(
        &mut self,
        reference: &[Complex<f32>],
        surveillance: &[Complex<f32>],
        out: &mut Surface,
    ) -> Result<(), String> {
        if reference.len() != self.cpi || surveillance.len() != self.cpi {
            return Err("GPU radar requires one complete integration interval".to_owned());
        }
        for (target, source) in self
            .upload
            .iter_mut()
            .zip(reference.iter().chain(surveillance))
        {
            *target = [source.re, source.im];
        }
        let validation = self
            .context
            .device
            .push_error_scope(wgpu::ErrorFilter::Validation);
        let memory = self
            .context
            .device
            .push_error_scope(wgpu::ErrorFilter::OutOfMemory);
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
            self.forward.dispatch(&mut pass);
            for tile in &self.tiles {
                tile.prepare.dispatch(&mut pass);
                if let Some(forward) = &self.mixed_forward {
                    forward.dispatch(&mut pass);
                }
                if let Some(correlate) = &tile.correlate {
                    correlate.dispatch(&mut pass);
                }
                self.inverse.dispatch(&mut pass);
                tile.power.dispatch(&mut pass);
            }
        }
        out.power.resize(self.ranges * self.dopplers, 0.0);
        let completed = self
            .readback
            .finish(&self.context, encoder, &self.output, &mut out.power);
        let memory_error = pollster::block_on(memory.pop());
        let validation_error = pollster::block_on(validation.pop());
        if let Some(error) = memory_error.or(validation_error) {
            return Err(error.to_string());
        }
        completed?;
        out.ranges = self.ranges;
        out.dopplers = self.dopplers;
        out.doppler_step_hz = self.doppler_step_hz;
        out.range_step_s = self.range_step_s;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
