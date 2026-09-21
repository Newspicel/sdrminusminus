use std::{
    num::NonZeroU64,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle, Thread},
    time::{Duration, Instant},
};

use bytemuck::{Pod, Zeroable};
use num_complex::Complex;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use sdrmm_dsp::{coherent_gain, hann};
use wgpu::util::DeviceExt;

use crate::spectrum::SpectrumFrame;

pub(crate) fn context() -> Result<&'static Arc<Context>, &'static str> {
    static CONTEXT: OnceLock<Result<Arc<Context>, String>> = OnceLock::new();
    CONTEXT
        .get_or_init(|| Context::new().map(Arc::new))
        .as_ref()
        .map_err(String::as_str)
}

const WORKGROUP_SIZE: u32 = 256;
#[cfg(not(test))]
pub(crate) const GPU_FRAME_BUDGET: Duration = Duration::from_millis(50);
#[cfg(test)]
pub(crate) const GPU_FRAME_BUDGET: Duration = Duration::from_secs(5);
const DROP_LOG_INTERVAL: Duration = Duration::from_secs(5);

const FFT_SHADER: &str = r#"
struct Complexes { values: array<vec2<f32>>, }
struct Params { span: u32, twiddle_stride: u32, butterflies: u32, _padding: u32, }

@group(0) @binding(0) var<storage, read_write> data: Complexes;
@group(0) @binding(1) var<storage, read> twiddles: Complexes;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(256)
fn fft_stage(@builtin(global_invocation_id) id: vec3<u32>) {
let butterfly = id.x;
if (butterfly >= params.butterflies) { return; }

let half = params.span / 2u;
let block = butterfly / half;
let offset = butterfly % half;
let even_index = block * params.span + offset;
let odd_index = even_index + half;
let a = data.values[even_index];
let b = data.values[odd_index];
let w = twiddles.values[offset * params.twiddle_stride];
let rotated = vec2<f32>(b.x * w.x - b.y * w.y, b.x * w.y + b.y * w.x);
data.values[even_index] = a + rotated;
data.values[odd_index] = a - rotated;
}
"#;

const POWER_SHADER: &str = r#"
struct Complexes { values: array<vec2<f32>>, }
struct Powers { values: array<f32>, }
struct Params { size: u32, inv_gain: f32, _padding0: u32, _padding1: u32, }

@group(0) @binding(0) var<storage, read> data: Complexes;
@group(0) @binding(1) var<storage, read_write> powers: Powers;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(256)
fn power_db(@builtin(global_invocation_id) id: vec3<u32>) {
let raw = id.x;
if (raw >= params.size) { return; }
let value = data.values[raw];
let magnitude = length(value) * params.inv_gain;
let shifted = (raw + params.size / 2u) % params.size;
powers.values[shifted] = 20.0 * log2(magnitude + 1e-12) * 0.30102999566;
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct StageParams {
    span: u32,
    twiddle_stride: u32,
    butterflies: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PowerParams {
    size: u32,
    inv_gain: f32,
    padding: [u32; 2],
}

pub(crate) struct Context {
    device: wgpu::Device,
    queue: wgpu::Queue,
    fft_pipeline: wgpu::ComputePipeline,
    fft_layout: wgpu::BindGroupLayout,
    power_pipeline: wgpu::ComputePipeline,
    power_layout: wgpu::BindGroupLayout,
    adapter_name: String,
}

impl Context {
    pub(crate) fn new() -> Result<Self, String> {
        Self::new_inner(false)
    }

    #[cfg(test)]
    pub(crate) fn new_for_test() -> Result<Self, String> {
        Self::new_inner(true)
    }

    fn new_inner(allow_cpu_adapter: bool) -> Result<Self, String> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|error| format!("request adapter: {error}"))?;
        let info = adapter.get_info();
        if info.device_type == wgpu::DeviceType::Cpu && !allow_cpu_adapter {
            return Err(format!(
                "adapter '{}' is software-rendered and would not accelerate FFTs",
                info.name
            ));
        }
        tracing::info!(adapter = %info.name, backend = ?info.backend, "GPU compute adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("SDR-- GPU compute"),
            ..Default::default()
        }))
        .map_err(|error| format!("request device: {error}"))?;

        let fft_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SDR-- FFT stage layout"),
            entries: &[
                storage_entry(0, false),
                storage_entry(1, true),
                uniform_entry(2),
            ],
        });
        let power_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SDR-- FFT power layout"),
            entries: &[
                storage_entry(0, true),
                storage_entry(1, false),
                uniform_entry(2),
            ],
        });
        let fft_pipeline = pipeline(&device, &fft_layout, FFT_SHADER, "fft_stage");
        let power_pipeline = pipeline(&device, &power_layout, POWER_SHADER, "power_db");

        Ok(Self {
            device,
            queue,
            fft_pipeline,
            fft_layout,
            power_pipeline,
            power_layout,
            adapter_name: info.name,
        })
    }

    pub(crate) fn adapter_name(&self) -> &str {
        &self.adapter_name
    }
}

struct Processor {
    context: Arc<Context>,
    size: usize,
    window: Vec<f32>,
    bit_reversed: Vec<usize>,
    upload: Vec<[f32; 2]>,
    data: wgpu::Buffer,
    output: wgpu::Buffer,
    readback: wgpu::Buffer,
    stage_bind_groups: Vec<wgpu::BindGroup>,
    batched_fft: Option<fft::FftBatch>,
    power_bind_group: wgpu::BindGroup,
}

impl Processor {
    fn new(context: Arc<Context>, size: usize) -> Result<Self, String> {
        let (size_u32, complex_bytes, power_bytes, limits) = validate_shape(&context, size)?;
        let window = hann(size);
        let inv_gain = 1.0 / coherent_gain(&window).max(f32::MIN_POSITIVE);
        let (bits, bit_reversed, twiddles) = fft_tables(size);
        let data = buffer(
            &context.device,
            "SDR-- FFT data",
            complex_bytes,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let twiddle_buffer = context
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("SDR-- FFT twiddles"),
                contents: bytemuck::cast_slice(&twiddles),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let output = buffer(
            &context.device,
            "SDR-- FFT dB output",
            power_bytes,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let readback = buffer(
            &context.device,
            "SDR-- FFT readback",
            power_bytes,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let stage_bind_groups = stage_bind_groups(
            &context,
            &data,
            &twiddle_buffer,
            size_u32,
            bits as usize,
            &limits,
        )?;
        let batched_fft =
            (size >= 1024).then(|| fft::FftBatch::new(&context, &data, size, 1, false));
        let power_bind_group = power_bind_group(&context, &data, &output, size_u32, inv_gain);

        Ok(Self {
            context,
            size,
            window,
            bit_reversed,
            upload: vec![[0.0; 2]; size],
            data,
            output,
            readback,
            stage_bind_groups,
            batched_fft,
            power_bind_group,
        })
    }

    fn power_db(&mut self, input: &[Complex<f32>], out: &mut [f32]) -> Result<(), String> {
        assert_eq!(input.len(), self.size, "input length must equal FFT size");
        assert_eq!(out.len(), self.size, "output length must equal FFT size");
        for (slot, &source) in self.upload.iter_mut().zip(&self.bit_reversed) {
            let value = input[source] * self.window[source];
            *slot = [value.re, value.im];
        }
        self.context.queue.write_buffer(
            &self.data,
            0,
            bytemuck::cast_slice(self.upload.as_slice()),
        );

        let mut encoder =
            self.context
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("SDR-- spectrum FFT"),
                });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("SDR-- FFT stages"),
                timestamp_writes: None,
            });
            if let Some(fft) = &self.batched_fft {
                fft.dispatch(&mut pass);
            } else {
                pass.set_pipeline(&self.context.fft_pipeline);
                for bind_group in &self.stage_bind_groups {
                    pass.set_bind_group(0, bind_group, &[]);
                    pass.dispatch_workgroups((self.size as u32 / 2).div_ceil(WORKGROUP_SIZE), 1, 1);
                }
            }
            pass.set_pipeline(&self.context.power_pipeline);
            pass.set_bind_group(0, &self.power_bind_group, &[]);
            pass.dispatch_workgroups((self.size as u32).div_ceil(WORKGROUP_SIZE), 1, 1);
        }
        encoder.copy_buffer_to_buffer(
            &self.output,
            0,
            &self.readback,
            0,
            bytes_for::<f32>(self.size)?,
        );
        let submission = self.context.queue.submit([encoder.finish()]);

        let (sender, receiver) = mpsc::sync_channel(1);
        self.readback
            .map_async(wgpu::MapMode::Read, .., move |result| {
                let _ = sender.send(result);
            });
        self.context
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(GPU_FRAME_BUDGET),
            })
            .map_err(|error| format!("wait for FFT: {error}"))?;
        receiver
            .recv_timeout(GPU_FRAME_BUDGET)
            .map_err(|error| format!("wait for FFT map callback: {error}"))?
            .map_err(|error| format!("map FFT output: {error}"))?;
        {
            let view = self
                .readback
                .get_mapped_range(..)
                .map_err(|error| format!("read FFT output: {error}"))?;
            out.copy_from_slice(bytemuck::cast_slice(&view));
        }
        self.readback.unmap();
        Ok(())
    }
}

fn validate_shape(context: &Context, size: usize) -> Result<(u32, u64, u64, wgpu::Limits), String> {
    if !size.is_power_of_two() || size < 2 {
        return Err(format!(
            "FFT size {size} is not a power of two of at least 2"
        ));
    }
    let size_u32 =
        u32::try_from(size).map_err(|_| format!("FFT size {size} exceeds the GPU index range"))?;
    let complex_bytes = bytes_for::<[f32; 2]>(size)?;
    let power_bytes = bytes_for::<f32>(size)?;
    let limits = context.device.limits();
    if complex_bytes > limits.max_buffer_size
        || complex_bytes > limits.max_storage_buffer_binding_size
    {
        return Err(format!(
            "FFT size {size} exceeds the adapter's storage-buffer limit"
        ));
    }
    if size_u32.div_ceil(WORKGROUP_SIZE) > limits.max_compute_workgroups_per_dimension {
        return Err(format!(
            "FFT size {size} exceeds the adapter's dispatch limit"
        ));
    }
    Ok((size_u32, complex_bytes, power_bytes, limits))
}

fn fft_tables(size: usize) -> (u32, Vec<usize>, Vec<[f32; 2]>) {
    let bits = size.trailing_zeros();
    let bit_reversed = (0..size)
        .map(|index| index.reverse_bits() >> (usize::BITS - bits))
        .collect();
    let twiddles = (0..size / 2)
        .map(|index| {
            let angle = -std::f32::consts::TAU * index as f32 / size as f32;
            [angle.cos(), angle.sin()]
        })
        .collect();
    (bits, bit_reversed, twiddles)
}

fn stage_bind_groups(
    context: &Context,
    data: &wgpu::Buffer,
    twiddles: &wgpu::Buffer,
    size: u32,
    stage_count: usize,
    limits: &wgpu::Limits,
) -> Result<Vec<wgpu::BindGroup>, String> {
    let param_stride =
        u64::from(limits.min_uniform_buffer_offset_alignment).max(size_of::<StageParams>() as u64);
    let param_stride_usize = usize::try_from(param_stride)
        .map_err(|_| "uniform-buffer alignment exceeds the host range".to_string())?;
    let stage_bytes_len = stage_count
        .checked_mul(param_stride_usize)
        .ok_or_else(|| "FFT stage-parameter buffer size overflow".to_string())?;
    let mut stage_bytes = vec![0_u8; stage_bytes_len];
    for stage in 0..stage_count {
        let span = 1_u32 << (stage + 1);
        let params = StageParams {
            span,
            twiddle_stride: size / span,
            butterflies: size / 2,
            padding: 0,
        };
        let start = stage * param_stride_usize;
        stage_bytes[start..start + size_of::<StageParams>()]
            .copy_from_slice(bytemuck::bytes_of(&params));
    }
    let stage_params = context
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SDR-- FFT stage parameters"),
            contents: &stage_bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
    Ok((0..stage_count)
        .map(|stage| {
            context
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("SDR-- FFT stage bind group"),
                    layout: &context.fft_layout,
                    entries: &[
                        bind(0, data.as_entire_binding()),
                        bind(1, twiddles.as_entire_binding()),
                        bind(
                            2,
                            wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &stage_params,
                                offset: stage as u64 * param_stride,
                                size: NonZeroU64::new(size_of::<StageParams>() as u64),
                            }),
                        ),
                    ],
                })
        })
        .collect())
}

fn power_bind_group(
    context: &Context,
    data: &wgpu::Buffer,
    output: &wgpu::Buffer,
    size: u32,
    inv_gain: f32,
) -> wgpu::BindGroup {
    let power_params = context
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SDR-- FFT power parameters"),
            contents: bytemuck::bytes_of(&PowerParams {
                size,
                inv_gain,
                padding: [0; 2],
            }),
            usage: wgpu::BufferUsages::UNIFORM,
        });
    context
        .device
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SDR-- FFT power bind group"),
            layout: &context.power_layout,
            entries: &[
                bind(0, data.as_entire_binding()),
                bind(1, output.as_entire_binding()),
                bind(2, power_params.as_entire_binding()),
            ],
        })
}

struct Job {
    input: Vec<Complex<f32>>,
    output: Vec<f32>,
    frame: SpectrumFrame,
}

struct Completion {
    job: Job,
    error: Option<String>,
}

pub(crate) struct Analyzer {
    size: usize,
    requests: Producer<Job>,
    completions: Consumer<Completion>,
    available: Option<Job>,
    worker_thread: Thread,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    dropped_frames: u64,
    last_drop_log: Option<Instant>,
}

impl Analyzer {
    pub(crate) fn new(context: Arc<Context>, size: usize) -> Result<Self, String> {
        let processor = Processor::new(context, size)?;
        let (requests, request_rx) = RingBuffer::new(1);
        let (completion_tx, completions) = RingBuffer::new(1);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("sdrmm-gpu-spectrum".to_string())
            .spawn(move || gpu_worker(processor, request_rx, completion_tx, &worker_stop))
            .map_err(|error| format!("start GPU spectrum worker: {error}"))?;
        let worker_thread = worker.thread().clone();

        Ok(Self {
            size,
            requests,
            completions,
            available: Some(Job {
                input: vec![Complex::new(0.0, 0.0); size],
                output: vec![0.0; size],
                frame: SpectrumFrame {
                    timestamp: 0,
                    center_hz: 0.0,
                    span_hz: 0.0,
                },
            }),
            worker_thread,
            stop,
            worker: Some(worker),
            dropped_frames: 0,
            last_drop_log: None,
        })
    }

    pub(crate) fn power_db(
        &mut self,
        input: &[Complex<f32>],
        out: &mut [f32],
        frame: SpectrumFrame,
    ) -> Result<Option<SpectrumFrame>, String> {
        assert_eq!(input.len(), self.size, "input length must equal FFT size");
        assert_eq!(out.len(), self.size, "output length must equal FFT size");
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
        {
            return Err("GPU spectrum worker stopped unexpectedly".to_string());
        }

        let completed = match self.completions.pop() {
            Ok(completion) => {
                if let Some(error) = completion.error {
                    return Err(error);
                }
                out.copy_from_slice(&completion.job.output);
                let frame = completion.job.frame;
                self.available = Some(completion.job);
                Some(frame)
            }
            Err(_) => None,
        };

        let dropped = if let Some(mut job) = self.available.take() {
            job.input.copy_from_slice(input);
            job.frame = frame;
            match self.requests.push(job) {
                Ok(()) => {
                    self.worker_thread.unpark();
                    false
                }
                Err(PushError::Full(job)) => {
                    self.available = Some(job);
                    true
                }
            }
        } else {
            true
        };
        if dropped {
            self.record_drop();
        }

        Ok(completed)
    }

    fn record_drop(&mut self) {
        self.dropped_frames = self.dropped_frames.saturating_add(1);
        let now = Instant::now();
        if self
            .last_drop_log
            .is_none_or(|last| now.duration_since(last) >= DROP_LOG_INTERVAL)
        {
            tracing::warn!(
                dropped_frames = self.dropped_frames,
                "GPU spectrum worker busy; dropping spectrum frames"
            );
            self.last_drop_log = Some(now);
        }
    }
}

impl Drop for Analyzer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker_thread.unpark();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn gpu_worker(
    mut processor: Processor,
    mut requests: Consumer<Job>,
    mut completions: Producer<Completion>,
    stop: &AtomicBool,
) {
    while !stop.load(Ordering::Acquire) {
        let Ok(mut job) = requests.pop() else {
            thread::park_timeout(Duration::from_millis(10));
            continue;
        };
        let error = processor.power_db(&job.input, &mut job.output).err();
        let mut completion = Completion { job, error };
        loop {
            match completions.push(completion) {
                Ok(()) => break,
                Err(PushError::Full(returned)) => completion = returned,
            }
            if stop.load(Ordering::Acquire) {
                return;
            }
            thread::yield_now();
        }
    }
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn pipeline(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
    source: &str,
    entry_point: &str,
) -> wgpu::ComputePipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(entry_point),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(entry_point),
        bind_group_layouts: &[Some(bind_group_layout)],
        immediate_size: 0,
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry_point),
        layout: Some(&layout),
        module: &shader,
        entry_point: Some(entry_point),
        compilation_options: Default::default(),
        cache: None,
    })
}

fn buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: false,
    })
}

fn bind(binding: u32, resource: wgpu::BindingResource<'_>) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource }
}

fn bytes_for<T>(len: usize) -> Result<u64, String> {
    len.checked_mul(size_of::<T>())
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or_else(|| "FFT buffer size overflow".to_string())
}

#[cfg(test)]
pub(crate) mod benchmarks;

pub(crate) mod caf;
mod compute;
mod fft;

pub(crate) fn spectrum_is_faster(context: Arc<Context>, size: usize) -> bool {
    match measure_spectrum(context, size) {
        Ok((cpu, gpu)) => {
            let faster = gpu.as_secs_f64() * 1.2 < cpu.as_secs_f64();
            tracing::info!(
                size,
                cpu_us = cpu.as_micros(),
                gpu_us = gpu.as_micros(),
                faster,
                "spectrum backend timing"
            );
            faster
        }
        Err(error) => {
            tracing::warn!(%error, "GPU spectrum timing failed; using CPU");
            false
        }
    }
}

fn measure_spectrum(context: Arc<Context>, size: usize) -> Result<(Duration, Duration), String> {
    let mut gpu = Processor::new(context, size)?;
    let mut cpu = sdrmm_dsp::SpectrumAnalyzer::new(size);
    let input: Vec<_> = (0..size)
        .map(|index| {
            Complex::new(
                (index % 127) as f32 / 127.0 - 0.5,
                (index % 257) as f32 / 257.0 - 0.5,
            )
        })
        .collect();
    let mut output = vec![0.0; size];
    for _ in 0..3 {
        cpu.power_db(&input, &mut output);
        gpu.power_db(&input, &mut output)?;
    }
    let mut cpu_times = [Duration::ZERO; 7];
    let mut gpu_times = [Duration::ZERO; 7];
    for (cpu_time, gpu_time) in cpu_times.iter_mut().zip(&mut gpu_times) {
        let start = Instant::now();
        cpu.power_db(std::hint::black_box(&input), &mut output);
        *cpu_time = start.elapsed();
        let start = Instant::now();
        gpu.power_db(std::hint::black_box(&input), &mut output)?;
        *gpu_time = start.elapsed();
    }
    cpu_times.sort_unstable();
    gpu_times.sort_unstable();
    Ok((cpu_times[3], gpu_times[3]))
}
