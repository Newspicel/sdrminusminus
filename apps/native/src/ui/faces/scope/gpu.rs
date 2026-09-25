use std::{cell::RefCell, rc::Rc};

use zgui::surface::{SurfaceRenderCx, SurfaceRenderer, wgpu};

use super::{
    colormap::{Colormap, colormap_wgsl},
    history::{HISTORY_ROWS, next_ring_row, rows_for_height, seed_placement},
};

const FIRST_BINS: usize = 1024;
const PARAMS_BYTES: u64 = 32;

pub const VERTEX_WGSL: &str = r"
struct Varying {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> Varying {
    var out: Varying;
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    out.uv = vec2<f32>(x, y);
    out.position = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return out;
}
";

const WATERFALL_WGSL: &str = r"
struct Params {
    write: f32,
    height: f32,
    rows: f32,
    view_start: f32,
    view_width: f32,
    pixels: f32,
    map: u32,
    spare: u32,
}

@group(0) @binding(0) var history: texture_2d<f32>;
@group(0) @binding(1) var linear: sampler;
@group(0) @binding(2) var shifts: texture_2d<f32>;
@group(0) @binding(3) var<uniform> params: Params;

fn wrap(value: f32, size: f32) -> f32 {
    return value - size * floor(value / size);
}

fn footprint_peak(tx: f32, row: i32) -> f32 {
    let bins = i32(textureDimensions(history).x);
    let left = tx * f32(bins);
    let right = left + params.view_width * f32(bins) / max(params.pixels, 1.0);
    let first = i32(floor(left));
    let last = max(first, i32(ceil(right)) - 1);
    var peak = 0.0;
    for (var i = 0; i < 32; i++) {
        let x = first + i;
        if (x > last) {
            break;
        }
        if (x >= 0 && x < bins) {
            peak = max(peak, textureLoad(history, vec2<i32>(x, row), 0).r);
        }
    }
    return peak;
}

@fragment
fn fragment(in: Varying) -> @location(0) vec4<f32> {
    let rows_back = in.uv.y * (params.rows - 1.0);
    let row = wrap(params.write - 1.0 - rows_back, params.height);
    let ty = (row + 0.5) / params.height;
    let tx = params.view_start + in.uv.x * params.view_width
        + textureLoad(shifts, vec2<i32>(0, i32(row)), 0).r;
    let bins = f32(textureDimensions(history).x);
    let per_pixel = params.view_width * bins / max(params.pixels, 1.0);
    var level = 0.0;
    if (tx >= 0.0 && tx <= 1.0) {
        if (per_pixel > 1.0) {
            level = footprint_peak(tx, i32(row));
        } else {
            level = textureSampleLevel(history, linear, vec2<f32>(tx, ty), 0.0).r;
        }
    }
    return vec4<f32>(colormap(level, params.map), 1.0);
}
";

pub enum Pending {
    Row(Vec<u8>),
    Seed {
        rows: Vec<u8>,
        count: usize,
        bins: usize,
    },
    Shift(f64),
}

pub struct WaterfallFeed {
    pending: Vec<Pending>,
    pub start: f64,
    pub width: f64,
    pub colormap: Colormap,
    changed: bool,
}

impl Default for WaterfallFeed {
    fn default() -> Self {
        Self {
            pending: Vec::new(),
            start: 0.0,
            width: 1.0,
            colormap: Colormap::default(),
            changed: true,
        }
    }
}

impl WaterfallFeed {
    pub fn push_row(&mut self, bins: &[u8]) {
        if !bins.is_empty() {
            self.pending.push(Pending::Row(bins.to_vec()));
        }
    }

    pub fn seed(&mut self, rows: Vec<u8>, count: usize, bins: usize) {
        if bins == 0 {
            return;
        }
        self.pending
            .retain(|pending| matches!(pending, Pending::Shift(_)));
        self.pending.push(Pending::Seed { rows, count, bins });
    }

    pub fn shift_rows(&mut self, delta: f64) {
        if delta != 0.0 {
            self.pending.push(Pending::Shift(delta));
        }
    }

    pub fn set_window(&mut self, start: f64, width: f64) {
        self.changed |= self.start != start || self.width != width;
        self.start = start;
        self.width = width;
    }

    pub fn set_colormap(&mut self, colormap: Colormap) {
        self.changed |= self.colormap != colormap;
        self.colormap = colormap;
    }
}

pub struct Ring {
    pub bins: usize,
    pub write: usize,
    pub shifts: Vec<f32>,
    pub shifts_dirty: bool,
}

impl Default for Ring {
    fn default() -> Self {
        Self {
            bins: FIRST_BINS,
            write: 0,
            shifts: vec![0.0; HISTORY_ROWS],
            shifts_dirty: true,
        }
    }
}

pub enum Upload<'a> {
    Reallocate(usize),
    Rows {
        at: usize,
        rows: &'a [u8],
        count: usize,
    },
}

impl Ring {
    pub fn apply<'a>(&mut self, pending: &'a Pending, mut upload: impl FnMut(Upload<'a>)) {
        match pending {
            Pending::Row(bins) => {
                if bins.len() != self.bins {
                    self.allocate(bins.len());
                    upload(Upload::Reallocate(self.bins));
                }
                if self.shifts[self.write] != 0.0 {
                    self.shifts[self.write] = 0.0;
                    self.shifts_dirty = true;
                }
                upload(Upload::Rows {
                    at: self.write,
                    rows: bins,
                    count: 1,
                });
                self.write = next_ring_row(self.write, HISTORY_ROWS);
            }
            Pending::Seed { rows, count, bins } => {
                let place = seed_placement(*count, HISTORY_ROWS);
                if place.rows == 0 {
                    return;
                }
                self.allocate(*bins);
                upload(Upload::Reallocate(self.bins));
                let from = place.skip * bins;
                upload(Upload::Rows {
                    at: 0,
                    rows: &rows[from..from + place.rows * bins],
                    count: place.rows,
                });
                self.write = place.write;
            }
            Pending::Shift(delta) => {
                for shift in &mut self.shifts {
                    *shift += *delta as f32;
                }
                self.shifts_dirty = true;
            }
        }
    }

    fn allocate(&mut self, bins: usize) {
        self.bins = bins.max(1);
        self.write = 0;
        self.shifts.fill(0.0);
        self.shifts_dirty = true;
    }
}

struct Resources {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    history: wgpu::Texture,
    shifts: wgpu::Texture,
    params: wgpu::Buffer,
    binding: wgpu::BindGroup,
    format: wgpu::TextureFormat,
    device: usize,
}

pub struct WaterfallSurface {
    feed: Rc<RefCell<WaterfallFeed>>,
    ring: Ring,
    held: Option<Resources>,
    drawn: Option<(u32, u32)>,
}

impl WaterfallSurface {
    pub fn new(feed: Rc<RefCell<WaterfallFeed>>) -> Self {
        Self {
            feed,
            ring: Ring::default(),
            held: None,
            drawn: None,
        }
    }
}

fn device_id(device: &wgpu::Device) -> usize {
    std::ptr::from_ref(device) as usize
}

pub fn texture(
    device: &wgpu::Device,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

pub fn texture_entry(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

pub fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

pub fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

pub fn linear_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("scope.linear"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..wgpu::SamplerDescriptor::default()
    })
}

pub fn uniform_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

pub fn pipeline(
    device: &wgpu::Device,
    label: &str,
    source: &str,
    layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vertex"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fragment"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(format.into())],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

pub fn draw(cx: &SurfaceRenderCx<'_>, pipeline: &wgpu::RenderPipeline, binding: &wgpu::BindGroup) {
    let mut encoder = cx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("scope.draw"),
        });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scope.pass"),
            multiview_mask: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: cx.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, binding, &[]);
        pass.draw(0..3, 0..1);
    }
    cx.queue.submit([encoder.finish()]);
}

pub fn write_rows(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    at: u32,
    bytes: &[u8],
    width: u32,
    rows: u32,
    texel: u32,
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x: 0, y: at, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * texel),
            rows_per_image: Some(rows),
        },
        wgpu::Extent3d {
            width,
            height: rows,
            depth_or_array_layers: 1,
        },
    );
}

#[must_use]
pub fn waterfall_source() -> String {
    format!("{VERTEX_WGSL}\n{}\n{WATERFALL_WGSL}", colormap_wgsl())
}

impl Resources {
    fn build(cx: &SurfaceRenderCx<'_>, bins: usize) -> Self {
        let device = cx.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waterfall.layout"),
            entries: &[
                texture_entry(0, true),
                sampler_entry(1),
                texture_entry(2, false),
                uniform_entry(3),
            ],
        });
        let format = cx.texture.format();
        let pipeline = pipeline(device, "waterfall", &waterfall_source(), &layout, format);
        let sampler = linear_sampler(device);
        let shifts = texture(
            device,
            "waterfall.shifts",
            1,
            HISTORY_ROWS as u32,
            wgpu::TextureFormat::R32Float,
        );
        let params = uniform_buffer(device, "waterfall.params", PARAMS_BYTES);
        let history = texture(
            device,
            "waterfall.history",
            bins as u32,
            HISTORY_ROWS as u32,
            wgpu::TextureFormat::R8Unorm,
        );
        let binding = bind(device, &layout, &history, &sampler, &shifts, &params);
        Self {
            pipeline,
            layout,
            sampler,
            history,
            shifts,
            params,
            binding,
            format,
            device: device_id(device),
        }
    }

    fn reallocate(&mut self, device: &wgpu::Device, bins: usize) {
        self.history = texture(
            device,
            "waterfall.history",
            bins as u32,
            HISTORY_ROWS as u32,
            wgpu::TextureFormat::R8Unorm,
        );
        self.binding = bind(
            device,
            &self.layout,
            &self.history,
            &self.sampler,
            &self.shifts,
            &self.params,
        );
    }
}

fn bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    history: &wgpu::Texture,
    sampler: &wgpu::Sampler,
    shifts: &wgpu::Texture,
    params: &wgpu::Buffer,
) -> wgpu::BindGroup {
    let view =
        |texture: &wgpu::Texture| texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("waterfall.binding"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view(history)),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&view(shifts)),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: params.as_entire_binding(),
            },
        ],
    })
}

#[must_use]
pub fn params_bytes(values: [f32; 6], map: u32) -> [u8; PARAMS_BYTES as usize] {
    let mut bytes = [0u8; PARAMS_BYTES as usize];
    for (at, value) in values.iter().enumerate() {
        bytes[at * 4..at * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[24..28].copy_from_slice(&map.to_le_bytes());
    bytes
}

impl WaterfallSurface {
    fn ready(&mut self, cx: &SurfaceRenderCx<'_>) -> bool {
        let stale = self.held.as_ref().is_none_or(|held| {
            held.format != cx.texture.format() || held.device != device_id(cx.device)
        });
        if stale {
            self.ring = Ring::default();
            self.held = Some(Resources::build(cx, self.ring.bins));
            self.drawn = None;
        }
        !stale
    }

    fn upload(&mut self, cx: &SurfaceRenderCx<'_>) -> bool {
        let pending = std::mem::take(&mut self.feed.borrow_mut().pending);
        let Some(held) = self.held.as_mut() else {
            return false;
        };
        for entry in &pending {
            self.ring.apply(entry, |upload| match upload {
                Upload::Reallocate(bins) => held.reallocate(cx.device, bins),
                Upload::Rows { at, rows, count } => write_rows(
                    cx.queue,
                    &held.history,
                    at as u32,
                    rows,
                    (rows.len() / count.max(1)) as u32,
                    count as u32,
                    1,
                ),
            });
        }
        if self.ring.shifts_dirty {
            let bytes: Vec<u8> = self
                .ring
                .shifts
                .iter()
                .flat_map(|shift| shift.to_le_bytes())
                .collect();
            write_rows(cx.queue, &held.shifts, 0, &bytes, 1, HISTORY_ROWS as u32, 4);
            self.ring.shifts_dirty = false;
        }
        !pending.is_empty()
    }
}

impl SurfaceRenderer for WaterfallSurface {
    fn render(&mut self, cx: &mut SurfaceRenderCx<'_>) {
        cx.request_animation_frame();
        if cx.size.width == 0 || cx.size.height == 0 {
            return;
        }
        let fresh = !self.ready(cx);
        let uploaded = self.upload(cx);
        let size = (cx.size.width, cx.size.height);
        let (start, width, map, changed) = {
            let mut feed = self.feed.borrow_mut();
            let changed = std::mem::take(&mut feed.changed);
            (feed.start, feed.width, feed.colormap, changed)
        };
        if !(fresh || uploaded || changed || self.drawn != Some(size)) {
            return;
        }
        let Some(held) = &self.held else {
            return;
        };
        let rows = rows_for_height(f64::from(cx.size.height), f64::from(cx.scale), HISTORY_ROWS);
        let values = [
            self.ring.write as f32,
            HISTORY_ROWS as f32,
            rows as f32,
            start as f32,
            width as f32,
            cx.size.width as f32,
        ];
        cx.queue
            .write_buffer(&held.params, 0, &params_bytes(values, map.index()));
        draw(cx, &held.pipeline, &held.binding);
        self.drawn = Some(size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows_written(ring: &mut Ring, pending: &Pending) -> Vec<(usize, usize)> {
        let mut written = Vec::new();
        ring.apply(pending, |upload| {
            if let Upload::Rows { at, count, .. } = upload {
                written.push((at, count));
            }
        });
        written
    }

    #[test]
    fn a_row_lands_at_the_cursor_which_then_moves_on() {
        let mut ring = Ring::default();
        assert_eq!(
            rows_written(&mut ring, &Pending::Row(vec![0; FIRST_BINS])),
            [(0, 1)]
        );
        assert_eq!(
            rows_written(&mut ring, &Pending::Row(vec![0; FIRST_BINS])),
            [(1, 1)]
        );
        assert_eq!(ring.write, 2);
    }

    #[test]
    fn a_new_bin_count_clears_the_ring_first() {
        let mut ring = Ring::default();
        ring.apply(&Pending::Row(vec![0; FIRST_BINS]), |_| {});
        let mut reallocated = None;
        ring.apply(&Pending::Row(vec![0; 8]), |upload| {
            if let Upload::Reallocate(bins) = upload {
                reallocated = Some(bins);
            }
        });
        assert_eq!(reallocated, Some(8));
        assert_eq!(ring.write, 1);
    }

    #[test]
    fn a_seed_keeps_the_newest_rows_and_leaves_the_cursor_above_them() {
        let mut ring = Ring::default();
        let rows = vec![0u8; 3 * 4];
        let written = rows_written(
            &mut ring,
            &Pending::Seed {
                rows,
                count: 3,
                bins: 4,
            },
        );
        assert_eq!(written, [(0, 3)]);
        assert_eq!((ring.bins, ring.write), (4, 3));
    }

    #[test]
    fn a_shift_moves_every_row_and_a_new_row_starts_unshifted() {
        let mut ring = Ring::default();
        ring.apply(&Pending::Shift(0.25), |_| {});
        assert!(ring.shifts.iter().all(|shift| *shift == 0.25));
        ring.apply(&Pending::Row(vec![0; FIRST_BINS]), |_| {});
        assert_eq!(ring.shifts[0], 0.0);
        assert_eq!(ring.shifts[1], 0.25);
    }

    #[test]
    fn the_feed_drops_rows_a_seed_replaces_and_ignores_no_ops() {
        let mut feed = WaterfallFeed::default();
        feed.push_row(&[1, 2]);
        feed.shift_rows(0.0);
        feed.shift_rows(0.1);
        feed.seed(vec![0; 4], 2, 2);
        assert_eq!(feed.pending.len(), 2);
        assert!(matches!(feed.pending[0], Pending::Shift(_)));
        feed.changed = false;
        feed.set_window(0.0, 1.0);
        feed.set_colormap(Colormap::Classic);
        assert!(!feed.changed);
        feed.set_colormap(Colormap::Viridis);
        assert!(feed.changed);
    }

    #[test]
    fn the_shader_parameters_pack_six_floats_and_the_map() {
        let bytes = params_bytes([1.0, 1024.0, 300.0, 0.25, 0.5, 640.0], 4);
        assert_eq!(&bytes[..4], &1.0f32.to_le_bytes());
        assert_eq!(&bytes[20..24], &640.0f32.to_le_bytes());
        assert_eq!(&bytes[24..28], &4u32.to_le_bytes());
    }

    #[test]
    fn the_waterfall_shader_parses() {
        let source = waterfall_source();
        let module = wgpu::naga::front::wgsl::parse_str(&source);
        assert!(module.is_ok(), "{module:?}");
    }
}
