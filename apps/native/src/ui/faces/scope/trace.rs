use std::{cell::RefCell, rc::Rc};

use zgui::{
    prelude::*,
    surface::{SurfaceRenderCx, SurfaceRenderer, wgpu},
};

use super::{
    gpu::{
        VERTEX_WGSL, draw, linear_sampler, pipeline, sampler_entry, texture, texture_entry,
        uniform_buffer, uniform_entry, write_rows,
    },
    live::{Live, Shown},
    plot::{AXIS_H, DENSITY_HEIGHT, DENSITY_WIDTH, TracePoints, columns_of, level_y, trace_points},
    traces::{DbWindow, TraceMode, trace_unit},
    view::{SpectrumView, decibel_ticks, frequency_ticks},
};

const MAX_COLUMNS: u32 = 8192;
const COLUMN_ROWS: u32 = 5;
const MAX_X_TICKS: usize = 32;
const MAX_Y_TICKS: usize = 16;
const PLOT_BYTES: usize = 48 + MAX_X_TICKS * 4 + MAX_Y_TICKS * 4;
pub const MISSING: f32 = -1.0e8;

pub const FLAG_DENSITY: u32 = 1;
pub const FLAG_LIVE: u32 = 2;

const TRACE_WGSL: &str = r"
struct Plot {
    width: f32,
    height: f32,
    plot_h: f32,
    scale: f32,
    cursor: f32,
    centre: f32,
    flags: u32,
    x_count: u32,
    y_count: u32,
    overlays: u32,
    spare0: u32,
    spare1: u32,
    xs: array<vec4<f32>, 8>,
    ys: array<vec4<f32>, 4>,
}

@group(0) @binding(0) var columns: texture_2d<f32>;
@group(0) @binding(1) var density: texture_2d<f32>;
@group(0) @binding(2) var linear: sampler;
@group(0) @binding(3) var<uniform> plot: Plot;

const GRID = vec3<f32>(0.19607843, 0.19607843, 0.19607843);
const TRACE = vec3<f32>(0.4, 0.89803922, 1.0);
const HOLD = vec3<f32>(1.0, 1.0, 0.0);
const INK = vec3<f32>(1.0, 1.0, 1.0);
const INK_DIM = vec3<f32>(0.49803922, 0.49803922, 0.49803922);
const MISSING = -1.0e7;

fn column_y(x: i32, row: i32) -> f32 {
    let count = i32(textureDimensions(columns).x);
    if (x < 0 || x >= count) {
        return -1.0e8;
    }
    return textureLoad(columns, vec2<i32>(x, row), 0).r;
}

fn segment_distance(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ab = b - a;
    let t = clamp(dot(p - a, ab) / max(dot(ab, ab), 1.0e-6), 0.0, 1.0);
    return length(p - (a + ab * t));
}

fn stroke(p: vec2<f32>, row: i32, half: f32) -> f32 {
    let c = i32(floor(p.x));
    var nearest = 1.0e9;
    for (var i = -3; i <= 2; i++) {
        let x0 = c + i;
        let y0 = column_y(x0, row);
        let y1 = column_y(x0 + 1, row);
        if (y0 > MISSING && y1 > MISSING) {
            let a = vec2<f32>(f32(x0) + 0.5, y0);
            let b = vec2<f32>(f32(x0) + 1.5, y1);
            nearest = min(nearest, segment_distance(p, a, b));
        }
    }
    return clamp(half + 0.5 - nearest, 0.0, 1.0);
}

fn line(at: f32, value: f32, half: f32) -> f32 {
    return clamp(half + 0.5 - abs(at - value), 0.0, 1.0);
}

fn x_tick(i: u32) -> f32 {
    return plot.xs[i / 4u][i % 4u];
}

fn y_tick(i: u32) -> f32 {
    return plot.ys[i / 4u][i % 4u];
}

fn grid(p: vec2<f32>, colour: vec3<f32>) -> vec3<f32> {
    var out = colour;
    let half = plot.scale * 0.5;
    for (var i = 0u; i < plot.y_count; i++) {
        out = mix(out, GRID, line(p.y, y_tick(i), half));
    }
    for (var i = 0u; i < plot.x_count; i++) {
        out = mix(out, GRID, line(p.x, x_tick(i), half));
    }
    if (plot.centre >= 0.0) {
        let dash = select(0.0, 1.0, fract(p.y / plot.scale / 6.0) < 2.0 / 6.0);
        out = mix(out, INK_DIM, line(p.x, plot.centre, half) * dash * 0.7);
    }
    return out;
}

fn overlays(p: vec2<f32>, colour: vec3<f32>) -> vec3<f32> {
    var out = colour;
    let half = plot.scale * 0.5;
    if ((plot.overlays & 1u) != 0u) {
        out = mix(out, HOLD, stroke(p, 2, half));
    }
    if ((plot.overlays & 2u) != 0u) {
        out = mix(out, INK, stroke(p, 3, half));
    }
    if ((plot.overlays & 4u) != 0u) {
        out = mix(out, INK_DIM, stroke(p, 4, half));
    }
    return out;
}

fn live(p: vec2<f32>, colour: vec3<f32>) -> vec3<f32> {
    var out = colour;
    let c = i32(floor(p.x));
    let high = column_y(c, 0);
    let low = column_y(c, 1);
    if (high <= MISSING) {
        return out;
    }
    let band = clamp(min(p.y - high, low - p.y) + 0.5, 0.0, 1.0);
    out = mix(out, TRACE, band * 0.3);
    out = mix(out, TRACE, stroke(p, 0, plot.scale * 0.625));
    out = mix(out, TRACE, clamp(p.y - high + 0.5, 0.0, 1.0) * 0.2);
    return out;
}

@fragment
fn fragment(in: Varying) -> @location(0) vec4<f32> {
    let p = in.position.xy;
    var colour = vec3<f32>(0.0);
    if (p.y >= plot.plot_h || (plot.flags & 2u) == 0u) {
        return vec4<f32>(colour, 1.0);
    }
    if ((plot.flags & 1u) != 0u) {
        let d = textureSampleLevel(density, linear, vec2<f32>(p.x / plot.width, p.y / plot.plot_h), 0.0);
        colour = mix(colour, d.rgb, d.a);
    }
    colour = grid(p, colour);
    colour = overlays(p, colour);
    colour = live(p, colour);
    if (plot.cursor >= 0.0) {
        colour = mix(colour, INK_DIM, line(p.x, plot.cursor, plot.scale * 0.5) * 0.9);
    }
    return vec4<f32>(colour, 1.0);
}
";

#[must_use]
pub fn trace_source() -> String {
    format!("{VERTEX_WGSL}\n{TRACE_WGSL}")
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub width: f32,
    pub height: f32,
    pub plot_h: f32,
    pub scale: f32,
    pub cursor: Option<f32>,
    pub centre: Option<f32>,
    pub flags: u32,
    pub overlays: u32,
    pub xs: Vec<f32>,
    pub ys: Vec<f32>,
}

#[must_use]
pub fn plot_bytes(frame: &Frame) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(PLOT_BYTES);
    for value in [
        frame.width,
        frame.height,
        frame.plot_h,
        frame.scale,
        frame.cursor.unwrap_or(-1.0),
        frame.centre.unwrap_or(-1.0),
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let x_count = frame.xs.len().min(MAX_X_TICKS) as u32;
    let y_count = frame.ys.len().min(MAX_Y_TICKS) as u32;
    for value in [frame.flags, x_count, y_count, frame.overlays, 0, 0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for (ticks, max) in [(&frame.xs, MAX_X_TICKS), (&frame.ys, MAX_Y_TICKS)] {
        for at in 0..max {
            let value = ticks.get(at).copied().unwrap_or(0.0);
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub width_css: f64,
    pub plot_h_css: f64,
    pub scale: f64,
}

#[must_use]
pub fn grid_lines(
    geometry: Geometry,
    centre_hz: f64,
    span_hz: f64,
    view: SpectrumView,
    window: DbWindow,
) -> (Vec<f32>, Vec<f32>, Option<f32>) {
    let Geometry {
        width_css,
        plot_h_css,
        scale,
    } = geometry;
    let ys = decibel_ticks(window.min, window.max, 4.0)
        .into_iter()
        .map(|db| (((plot_h_css * (1.0 - trace_unit(db, window))).round() + 0.5) * scale) as f32)
        .collect();
    let target = (width_css / 110.0).floor().max(2.0);
    let xs = frequency_ticks(centre_hz, span_hz, view, target)
        .into_iter()
        .map(|tick| (((tick.at * width_css).round() + 0.5) * scale) as f32)
        .collect();
    let centre_at = view.place(0.5);
    let centre = (0.0..=1.0)
        .contains(&centre_at)
        .then(|| (((centre_at * width_css).round() + 0.5) * scale) as f32);
    (xs, ys, centre)
}

pub struct Inputs {
    pub view: Signal<SpectrumView>,
    pub window: Signal<DbWindow>,
    pub overlays: Signal<Vec<TraceMode>>,
    pub hover: RwSignal<Option<f64>>,
}

struct Resources {
    pipeline: wgpu::RenderPipeline,
    columns: wgpu::Texture,
    density: wgpu::Texture,
    params: wgpu::Buffer,
    binding: wgpu::BindGroup,
    format: wgpu::TextureFormat,
    width: u32,
}

type Drawn = (
    Shown,
    SpectrumView,
    DbWindow,
    Vec<TraceMode>,
    Option<f64>,
    (u32, u32),
);

pub struct TraceSurface {
    live: Rc<RefCell<Live>>,
    inputs: Inputs,
    held: Option<Resources>,
    points: TracePoints,
    high: Vec<f32>,
    low: Vec<f32>,
    staging: Vec<u8>,
    image: Vec<u8>,
    density_revision: Option<u64>,
    last: Option<Drawn>,
}

impl TraceSurface {
    pub fn new(live: Rc<RefCell<Live>>, inputs: Inputs) -> Self {
        Self {
            live,
            inputs,
            held: None,
            points: TracePoints::default(),
            high: Vec::new(),
            low: Vec::new(),
            staging: Vec::new(),
            image: Vec::new(),
            density_revision: None,
            last: None,
        }
    }

    fn ensure(&mut self, cx: &SurfaceRenderCx<'_>, width: u32) {
        let format = cx.texture.format();
        let stale = self
            .held
            .as_ref()
            .is_none_or(|held| held.format != format || held.width != width);
        if !stale {
            return;
        }
        let device = cx.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trace.layout"),
            entries: &[
                texture_entry(0, false),
                texture_entry(1, true),
                sampler_entry(2),
                uniform_entry(3),
            ],
        });
        let pipeline = pipeline(device, "trace", &trace_source(), &layout, format);
        let sampler = linear_sampler(device);
        let columns = texture(
            device,
            "trace.columns",
            width,
            COLUMN_ROWS,
            wgpu::TextureFormat::R32Float,
        );
        let density = texture(
            device,
            "trace.density",
            DENSITY_WIDTH as u32,
            DENSITY_HEIGHT as u32,
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let params = uniform_buffer(device, "trace.params", PLOT_BYTES as u64);
        let view =
            |texture: &wgpu::Texture| texture.create_view(&wgpu::TextureViewDescriptor::default());
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trace.binding"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view(&columns)),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view(&density)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params.as_entire_binding(),
                },
            ],
        });
        self.held = Some(Resources {
            pipeline,
            columns,
            density,
            params,
            binding,
            format,
            width,
        });
        self.density_revision = None;
        self.last = None;
    }

    fn fill_row(
        &mut self,
        row: usize,
        width: u32,
        plot_h: f64,
        window: DbWindow,
        db: &[f32],
        view: SpectrumView,
    ) {
        trace_points(db, view, f64::from(width), &mut self.points);
        columns_of(&self.points, width as usize, &mut self.high, &mut self.low);
        let start = row * width as usize * 4;
        let column = |value: f32| {
            if value.is_nan() {
                MISSING
            } else {
                level_y(value, plot_h, window)
            }
        };
        for (at, value) in self.high.iter().enumerate() {
            let offset = start + at * 4;
            self.staging[offset..offset + 4].copy_from_slice(&column(*value).to_le_bytes());
        }
        if row == 0 {
            let low_start = width as usize * 4;
            for (at, value) in self.low.iter().enumerate() {
                let offset = low_start + at * 4;
                self.staging[offset..offset + 4].copy_from_slice(&column(*value).to_le_bytes());
            }
        }
    }

    fn upload_density(&mut self, cx: &SurfaceRenderCx<'_>, live: &Live) -> bool {
        let Some(layer) = live.density.as_ref() else {
            return false;
        };
        if self.density_revision != Some(layer.revision) {
            layer.image(&mut self.image);
            if let Some(held) = &self.held {
                write_rows(
                    cx.queue,
                    &held.density,
                    0,
                    &self.image,
                    DENSITY_WIDTH as u32,
                    DENSITY_HEIGHT as u32,
                    4,
                );
            }
            self.density_revision = Some(layer.revision);
        }
        true
    }
}

impl SurfaceRenderer for TraceSurface {
    fn render(&mut self, cx: &mut SurfaceRenderCx<'_>) {
        cx.request_animation_frame();
        if cx.size.width == 0 || cx.size.height == 0 {
            return;
        }
        let width = cx.size.width.min(MAX_COLUMNS);
        self.ensure(cx, width);
        let live_cell = Rc::clone(&self.live);
        let mut live = live_cell.borrow_mut();
        let now = live.now_ms();
        let shown = live.shown(now);
        let view = self.inputs.view.get_untracked();
        let window = self.inputs.window.get_untracked();
        let modes = self.inputs.overlays.get_untracked();
        let hover = self.inputs.hover.get_untracked();
        let size = (cx.size.width, cx.size.height);
        let state = (shown, view, window, modes.clone(), hover, size);
        let settled = live.tween.settled(now);
        if settled && self.last.as_ref() == Some(&state) {
            return;
        }
        self.last = Some(state);
        let scale = f64::from(cx.scale.max(0.01));
        let plot_h = (f64::from(cx.size.height) - AXIS_H * scale).max(1.0);
        let mut frame = Frame {
            width: width as f32,
            height: cx.size.height as f32,
            plot_h: plot_h as f32,
            scale: scale as f32,
            ..Frame::default()
        };
        self.staging.clear();
        self.staging
            .resize(width as usize * COLUMN_ROWS as usize * 4, 0);
        if let Some(meta) = live.frame.filter(|_| window.max > window.min) {
            let db = live.tween.sample(now).to_vec();
            if db.len() >= 2 {
                frame.flags |= FLAG_LIVE;
                self.fill_row(0, width, plot_h, window, &db, view);
                for (slot, mode) in [TraceMode::Peak, TraceMode::Average, TraceMode::Min]
                    .into_iter()
                    .enumerate()
                {
                    let overlay = live
                        .traces
                        .as_ref()
                        .map(|traces| traces.trace(mode).to_vec())
                        .filter(|trace| modes.contains(&mode) && trace.len() == db.len());
                    if let Some(trace) = overlay {
                        frame.overlays |= 1 << slot;
                        self.fill_row(slot + 2, width, plot_h, window, &trace, view);
                    }
                }
                let geometry = Geometry {
                    width_css: f64::from(width) / scale,
                    plot_h_css: plot_h / scale,
                    scale,
                };
                let (xs, ys, centre) =
                    grid_lines(geometry, meta.centre_hz, meta.span_hz, view, window);
                frame.xs = xs;
                frame.ys = ys;
                frame.centre = centre;
                frame.cursor =
                    hover.map(|at| (((at * geometry.width_css).round() + 0.5) * scale) as f32);
                if self.upload_density(cx, &live) {
                    frame.flags |= FLAG_DENSITY;
                }
            }
        }
        drop(live);
        let Some(held) = &self.held else {
            return;
        };
        write_rows(
            cx.queue,
            &held.columns,
            0,
            &self.staging,
            width,
            COLUMN_ROWS,
            4,
        );
        cx.queue.write_buffer(&held.params, 0, &plot_bytes(&frame));
        draw(cx, &held.pipeline, &held.binding);
    }
}

#[cfg(test)]
mod tests {
    use super::{super::view::FULL_VIEW, *};

    #[test]
    fn the_plot_uniform_has_the_layout_the_shader_reads() {
        let frame = Frame {
            width: 640.0,
            cursor: Some(12.0),
            flags: FLAG_LIVE,
            xs: vec![1.0, 2.0],
            ys: vec![3.0],
            ..Frame::default()
        };
        let bytes = plot_bytes(&frame);
        assert_eq!(bytes.len(), PLOT_BYTES);
        assert_eq!(&bytes[..4], &640.0f32.to_le_bytes());
        assert_eq!(&bytes[16..20], &12.0f32.to_le_bytes());
        assert_eq!(&bytes[20..24], &(-1.0f32).to_le_bytes());
        assert_eq!(&bytes[24..28], &FLAG_LIVE.to_le_bytes());
        assert_eq!(&bytes[28..32], &2u32.to_le_bytes());
        assert_eq!(&bytes[48..52], &1.0f32.to_le_bytes());
        assert_eq!(&bytes[48 + 128..48 + 132], &3.0f32.to_le_bytes());
    }

    #[test]
    fn grid_lines_sit_on_whole_css_pixels_in_device_space() {
        let window = DbWindow {
            min: -100.0,
            max: -20.0,
        };
        let geometry = Geometry {
            width_css: 440.0,
            plot_h_css: 80.0,
            scale: 2.0,
        };
        let (xs, ys, centre) = grid_lines(geometry, 100e6, 2e6, FULL_VIEW, window);
        assert!(!xs.is_empty());
        assert_eq!(centre, Some(441.0));
        assert!(ys.iter().all(|y| (y / 2.0 - 0.5).fract() == 0.0));
        let zoomed = FULL_VIEW.zoom(0.0, 4.0);
        assert_eq!(grid_lines(geometry, 100e6, 2e6, zoomed, window).2, None);
    }

    #[test]
    fn the_trace_shader_validates() {
        use wgpu::naga::{front::wgsl, valid};
        let module = wgsl::parse_str(&trace_source()).expect("the trace shader parses");
        let checked =
            valid::Validator::new(valid::ValidationFlags::all(), valid::Capabilities::all())
                .validate(&module);
        assert!(checked.is_ok(), "{checked:?}");
    }
}
