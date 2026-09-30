use std::sync::Arc;

use num_complex::Complex;
use sdrmm_dsp::fft::FftPair;
use wgpu::util::DeviceExt;

use super::*;
use crate::gpu::buffer;

type C32 = Complex<f32>;

fn gpu() -> Arc<Context> {
    Arc::clone(crate::gpu::context().expect("GPU adapter"))
}

fn noise(len: usize, seed: u64) -> Vec<C32> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32 - 0.5
    };
    (0..len).map(|_| C32::new(next(), next())).collect()
}

fn bit_reversed(values: &[C32], size: usize) -> Vec<[f32; 2]> {
    let bits = size.trailing_zeros();
    let mut out = vec![[0.0; 2]; values.len()];
    for (start, chunk) in values.chunks(size).enumerate() {
        for (index, value) in chunk.iter().enumerate() {
            let at = index.reverse_bits() >> (usize::BITS - bits);
            out[start * size + at] = [value.re, value.im];
        }
    }
    out
}

fn upload(context: &Context, values: &[[f32; 2]]) -> wgpu::Buffer {
    context
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(values),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        })
}

fn run(context: &Context, fft: &FftBatch, data: &wgpu::Buffer, len: usize) -> Vec<C32> {
    let bytes = len as u64 * 8;
    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        fft.dispatch(&mut pass);
    }
    let readback = buffer(
        &context.device,
        "FFT test readback",
        bytes,
        wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
    );
    encoder.copy_buffer_to_buffer(data, 0, &readback, 0, bytes);
    context.queue.submit([encoder.finish()]);
    readback.map_async(wgpu::MapMode::Read, .., |result| result.expect("mapped"));
    context
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("GPU finished");
    let view = readback.get_mapped_range(..).expect("mapped range");
    bytemuck::cast_slice::<u8, [f32; 2]>(&view)
        .iter()
        .map(|value| C32::new(value[0], value[1]))
        .collect()
}

fn relative_error(actual: &[C32], expected: &[C32]) -> f64 {
    let error: f64 = actual
        .iter()
        .zip(expected)
        .map(|(a, b)| f64::from((a - b).norm_sqr()))
        .sum();
    let norm: f64 = expected.iter().map(|b| f64::from(b.norm_sqr())).sum();
    (error / norm).sqrt()
}

fn expected(input: &[C32], size: usize, inverse: bool) -> Vec<C32> {
    let mut pair = FftPair::new(size);
    let mut out = input.to_vec();
    for chunk in out.chunks_mut(size) {
        if inverse {
            pair.inverse(chunk);
        } else {
            pair.forward(chunk);
        }
    }
    out
}

#[test]
fn sizes_outside_16_to_2_22_are_refused() {
    for size in [0, 1, 8, 24, 3 << 10, 1 << 23] {
        assert!(check_size(size).is_err(), "{size}");
    }
    for bits in 4..=22 {
        assert!(check_size(1 << bits).is_ok(), "{bits}");
    }
}

#[test]
#[ignore = "requires a GPU adapter"]
fn every_power_of_two_from_16_to_2_22_matches_rustfft() {
    let context = gpu();
    for bits in 4..=22 {
        let size = 1usize << bits;
        let batches = ((1usize << 16) / size).max(1) + 1;
        let input = noise(size * batches, bits as u64);
        for inverse in [false, true] {
            let data = upload(&context, &bit_reversed(&input, size));
            let fft = FftBatch::new(&context, &data, size, batches, inverse).expect("fft");
            let actual = run(&context, &fft, &data, input.len());
            let error = relative_error(&actual, &expected(&input, size, inverse));
            assert!(error < 1e-5, "size {size} inverse {inverse}: {error}");
        }
    }
}

#[test]
#[ignore = "requires a GPU adapter"]
fn a_range_leaves_the_rest_of_the_buffer_alone() {
    let context = gpu();
    let size = 64;
    let input = noise(5 * size, 7);
    let flat = bit_reversed(&input, size);
    let data = upload(&context, &flat);
    let plan = Transforms {
        size,
        offset: 2 * size,
        batches: 2,
        inverse: false,
    };
    let fft = FftBatch::over(&context, &data, plan).expect("fft");
    let actual = run(&context, &fft, &data, input.len());
    let untouched: Vec<C32> = flat
        .iter()
        .map(|value| C32::new(value[0], value[1]))
        .collect();
    assert_eq!(actual[..2 * size], untouched[..2 * size]);
    assert_eq!(actual[4 * size..], untouched[4 * size..]);
    let transformed = expected(&input[2 * size..4 * size], size, false);
    assert!(relative_error(&actual[2 * size..4 * size], &transformed) < 1e-5);
}
