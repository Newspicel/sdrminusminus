use std::sync::Arc;

use zgui::{
    prelude::*,
    surface::{SurfaceRenderCx, wgpu},
};

use crate::{socket::Spectrum, ui::plot::Palette};

pub const HISTORY_ROWS: u32 = 512;
const PALETTE_STEPS: u32 = 256;
const ROW_ALIGNMENT: u32 = 256;

#[must_use]
pub fn history_width(bins: usize) -> u32 {
    let bins = bins.max(1) as u32;
    bins.div_ceil(ROW_ALIGNMENT) * ROW_ALIGNMENT
}

#[must_use]
pub fn palette_texels(palette: Palette) -> Vec<u8> {
    let mut texels = Vec::with_capacity(PALETTE_STEPS as usize * 4);
    for step in 0..PALETTE_STEPS {
        let [r, g, b] = palette.rgb(step as f32 / (PALETTE_STEPS - 1) as f32);
        texels.extend_from_slice(&[
            (r.clamp(0.0, 1.0) * 255.0) as u8,
            (g.clamp(0.0, 1.0) * 255.0) as u8,
            (b.clamp(0.0, 1.0) * 255.0) as u8,
            255,
        ]);
    }
    texels
}

#[must_use]
pub fn padded_row(bins: &[u8], width: u32) -> Vec<u8> {
    let mut row = vec![0u8; width as usize];
    let take = bins.len().min(row.len());
    row[..take].copy_from_slice(&bins[..take]);
    row
}

struct Device {
    pipeline: wgpu::RenderPipeline,
    binding: wgpu::BindGroup,
    history: wgpu::Texture,
    params: wgpu::Buffer,
    width: u32,
    format: wgpu::TextureFormat,
}

pub struct WaterfallSurface {
    spectrum: Signal<Option<Arc<Spectrum>>>,
    palette: Signal<Palette>,
    device: Option<Device>,
    drawn_palette: Option<Palette>,
    cursor: u32,
    last: Option<(u16, u32)>,
}

impl WaterfallSurface {
    pub fn new(spectrum: Signal<Option<Arc<Spectrum>>>, palette: Signal<Palette>) -> Self {
        Self {
            spectrum,
            palette,
            device: None,
            drawn_palette: None,
            cursor: 0,
            last: None,
        }
    }

    fn build(&mut self, cx: &SurfaceRenderCx<'_>, width: u32) -> Device {
        let device = cx.device;
        let history = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("waterfall.history"),
            size: wgpu::Extent3d {
                width,
                height: HISTORY_ROWS,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let palette = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("waterfall.palette"),
            size: wgpu::Extent3d {
                width: PALETTE_STEPS,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("waterfall.params"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("waterfall.shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waterfall.layout"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waterfall.binding"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &history.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &palette.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params.as_entire_binding(),
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("waterfall.pipeline_layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let format = cx.texture.format();
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("waterfall.pipeline"),
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
        });
        self.palette_upload(cx, &palette);
        Device {
            pipeline,
            binding,
            history,
            params,
            width,
            format,
        }
    }

    fn palette_upload(&self, cx: &SurfaceRenderCx<'_>, texture: &wgpu::Texture) {
        let texels = palette_texels(self.palette.get_untracked());
        cx.queue.write_texture(
            texture.as_image_copy(),
            &texels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(PALETTE_STEPS * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: PALETTE_STEPS,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
    }
}

impl SurfaceRenderer for WaterfallSurface {
    fn render(&mut self, cx: &mut SurfaceRenderCx<'_>) {
        cx.request_animation_frame();
        if cx.size.width == 0 || cx.size.height == 0 {
            return;
        }
        let latest = self.spectrum.get_untracked();
        let bins = latest.as_ref().map_or(0, |spectrum| spectrum.bins.len());
        let width = history_width(bins.max(512));

        let chosen = self.palette.get_untracked();
        let stale = self
            .device
            .as_ref()
            .is_none_or(|held| held.width != width || held.format != cx.texture.format());
        if stale {
            self.device = Some(self.build(cx, width));
            self.drawn_palette = Some(chosen);
            self.cursor = 0;
            self.last = None;
        } else if self.drawn_palette != Some(chosen) {
            self.device = Some(self.build(cx, width));
            self.drawn_palette = Some(chosen);
        }
        let Some(held) = &self.device else {
            return;
        };

        if let Some(spectrum) = &latest {
            let stamp = (spectrum.stream_id, spectrum.seq);
            if self.last != Some(stamp) && !spectrum.bins.is_empty() {
                self.last = Some(stamp);
                let row = padded_row(&spectrum.bins, held.width);
                cx.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &held.history,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: 0,
                            y: self.cursor,
                            z: 0,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &row,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(held.width),
                        rows_per_image: None,
                    },
                    wgpu::Extent3d {
                        width: held.width,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                );
                self.cursor = (self.cursor + 1) % HISTORY_ROWS;
            }
        }

        let columns = latest.as_ref().map_or(512, |spectrum| spectrum.bins.len()) as f32;
        cx.queue.write_buffer(
            &held.params,
            0,
            &params_bytes(
                self.cursor as f32,
                HISTORY_ROWS as f32,
                columns,
                held.width as f32,
            ),
        );

        let mut encoder = cx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("waterfall"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("waterfall.pass"),
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
            pass.set_pipeline(&held.pipeline);
            pass.set_bind_group(0, &held.binding, &[]);
            pass.draw(0..3, 0..1);
        }
        cx.queue.submit([encoder.finish()]);
    }
}

#[must_use]
pub fn params_bytes(cursor: f32, rows: f32, columns: f32, width: f32) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    for (slot, value) in bytes
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip([cursor, rows, columns, width])
    {
        *slot = value.to_le_bytes();
    }
    bytes
}

const SHADER: &str = r#"
struct Params {
    cursor: f32,
    rows: f32,
    columns: f32,
    width: f32,
}

@group(0) @binding(0) var history: texture_2d<f32>;
@group(0) @binding(1) var palette: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: Params;

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

@fragment
fn fragment(in: Varying) -> @location(0) vec4<f32> {
    let rows = i32(params.rows);
    let columns = max(i32(params.columns), 1);
    let column = clamp(i32(in.uv.x * params.columns), 0, columns - 1);
    let age = i32(in.uv.y * params.rows);
    var row = i32(params.cursor) - 1 - age;
    row = ((row % rows) + rows) % rows;
    let level = textureLoad(history, vec2<i32>(column, row), 0).r;
    let step = clamp(i32(level * 255.0), 0, 255);
    let colour = textureLoad(palette, vec2<i32>(step, 0), 0);
    return vec4<f32>(colour.rgb, 1.0);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_history_row_is_padded_out_to_the_copy_alignment() {
        assert_eq!(history_width(512), 512);
        assert_eq!(history_width(300), 512);
        assert_eq!(history_width(1), 256);
        assert_eq!(history_width(0), 256);
        assert_eq!(history_width(1024), 1024);
    }

    #[test]
    fn a_short_spectrum_is_padded_and_a_long_one_is_cut() {
        assert_eq!(padded_row(&[1, 2, 3], 8), vec![1, 2, 3, 0, 0, 0, 0, 0]);
        assert_eq!(padded_row(&[1, 2, 3, 4], 2), vec![1, 2]);
        assert_eq!(padded_row(&[], 3), vec![0, 0, 0]);
    }

    #[test]
    fn the_palette_lookup_runs_opaque_from_one_stop_to_the_other() {
        let texels = palette_texels(Palette::Viridis);
        assert_eq!(texels.len(), 256 * 4);
        assert!(
            texels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|texel| texel[3] == 255)
        );
        assert_ne!(&texels[..3], &texels[texels.len() - 4..texels.len() - 1]);
    }

    #[test]
    fn the_shader_parameters_go_out_as_four_little_endian_floats() {
        let bytes = params_bytes(1.0, 512.0, 512.0, 512.0);
        assert_eq!(&bytes[..4], &1.0f32.to_le_bytes());
        assert_eq!(&bytes[4..8], &512.0f32.to_le_bytes());
    }

    #[test]
    fn the_two_palettes_do_not_produce_the_same_lookup() {
        assert_ne!(
            palette_texels(Palette::Viridis),
            palette_texels(Palette::Classic)
        );
    }
}
