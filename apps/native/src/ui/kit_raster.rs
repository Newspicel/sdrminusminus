use std::{cell::RefCell, rc::Rc};

use sdrmm_wire::patch::NodeBody;
use zgui::{
    prelude::*,
    surface::{SurfaceRenderCx, wgpu},
};

use crate::{store::Store, ui::params::entry};

pub type Rgba = [u8; 4];

pub const PLOT_BG: Rgba = [8, 9, 11, 255];
pub const PLOT_GRID: Rgba = [50, 50, 50, 255];
pub const PLOT_INK: Rgba = [127, 127, 127, 255];
pub const PLOT_TRACE: Rgba = [102, 229, 255, 255];
pub const PLOT_HOLD: Rgba = [240, 180, 80, 255];
pub const WHITE: Rgba = [255, 255, 255, 255];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub aspect: Option<f32>,
    pub pixels: Vec<u8>,
}

impl Raster {
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.pixels.resize(width as usize * height as usize * 4, 0);
    }

    pub fn fill(&mut self, colour: Rgba) {
        for pixel in self.pixels.as_chunks_mut::<4>().0 {
            *pixel = colour;
        }
    }

    fn index(&self, x: i64, y: i64) -> Option<usize> {
        let inside = x >= 0 && y >= 0 && x < i64::from(self.width) && y < i64::from(self.height);
        inside.then(|| (y as usize * self.width as usize + x as usize) * 4)
    }

    pub fn put(&mut self, x: i64, y: i64, colour: Rgba) {
        if let Some(at) = self.index(x, y)
            && let Some(pixel) = self.pixels.get_mut(at..at + 4)
        {
            pixel.copy_from_slice(&colour);
        }
    }

    pub fn blend(&mut self, x: i64, y: i64, colour: Rgba, alpha: f32) {
        let alpha = alpha.clamp(0.0, 1.0);
        if let Some(at) = self.index(x, y)
            && let Some(pixel) = self.pixels.get_mut(at..at + 3)
        {
            for (channel, target) in pixel.iter_mut().zip(colour) {
                let mixed = f32::from(*channel) + (f32::from(target) - f32::from(*channel)) * alpha;
                *channel = mixed.round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    pub fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, colour: Rgba, alpha: f32) {
        let (left, right) = (x0.min(x1).round() as i64, x0.max(x1).round() as i64);
        let (top, bottom) = (y0.min(y1).round() as i64, y0.max(y1).round() as i64);
        for y in top..bottom.max(top + 1) {
            for x in left..right.max(left + 1) {
                self.blend(x, y, colour, alpha);
            }
        }
    }

    pub fn line(&mut self, from: (f32, f32), to: (f32, f32), pen: Pen) {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let length = dx.hypot(dy);
        let steps = length.ceil().max(1.0) as usize;
        let reach = (pen.width / 2.0).max(0.5);
        let (on, off) = pen.dash.unwrap_or((f32::INFINITY, 0.0));
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            if (t * length) % (on + off) >= on {
                continue;
            }
            self.dot(from.0 + dx * t, from.1 + dy * t, reach, pen);
        }
    }

    fn dot(&mut self, x: f32, y: f32, reach: f32, pen: Pen) {
        let spread = (reach - 0.5).ceil().max(0.0) as i64;
        let (cx, cy) = (x.round() as i64, y.round() as i64);
        for oy in -spread..=spread {
            for ox in -spread..=spread {
                self.blend(cx + ox, cy + oy, pen.colour, pen.alpha);
            }
        }
    }

    pub fn circle(&mut self, centre: (f32, f32), radius: f32, pen: Pen) {
        let steps = (radius * std::f32::consts::TAU).ceil().max(8.0) as usize;
        let mut last = (centre.0 + radius, centre.1);
        let mut run = 0.0f32;
        let (on, off) = pen.dash.unwrap_or((f32::INFINITY, 0.0));
        for step in 1..=steps {
            let angle = step as f32 / steps as f32 * std::f32::consts::TAU;
            let next = (
                centre.0 + radius * angle.cos(),
                centre.1 + radius * angle.sin(),
            );
            if run % (on + off) < on {
                self.line(last, next, Pen { dash: None, ..pen });
            }
            run += (next.0 - last.0).hypot(next.1 - last.1);
            last = next;
        }
    }
}

#[cfg(test)]
impl Raster {
    #[must_use]
    pub fn sized(width: u32, height: u32) -> Self {
        let mut raster = Self {
            scale: 1.0,
            ..Self::default()
        };
        raster.resize(width, height);
        raster.fill([0, 0, 0, 255]);
        raster
    }

    #[must_use]
    pub fn at(&self, x: i64, y: i64) -> Option<Rgba> {
        let at = self.index(x, y)?;
        self.pixels.get(at..at + 4)?.try_into().ok()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pen {
    pub colour: Rgba,
    pub alpha: f32,
    pub width: f32,
    pub dash: Option<(f32, f32)>,
}

impl Pen {
    #[must_use]
    pub fn new(colour: Rgba, alpha: f32, width: f32) -> Self {
        Self {
            colour,
            alpha,
            width,
            dash: None,
        }
    }

    #[must_use]
    pub fn dashed(self, on: f32, off: f32) -> Self {
        Self {
            dash: Some((on, off)),
            ..self
        }
    }
}

#[must_use]
pub fn letterbox(width: u32, height: u32, aspect: Option<f32>) -> (f32, f32, f32, f32) {
    let (width, height) = (width as f32, height as f32);
    let Some(aspect) = aspect.filter(|aspect| aspect.is_finite() && *aspect > 0.0) else {
        return (0.0, 0.0, width, height);
    };
    if height <= 0.0 || width / height > aspect {
        let fitted = height * aspect;
        ((width - fitted) / 2.0, 0.0, fitted, height)
    } else {
        let fitted = width / aspect;
        (0.0, (height - fitted) / 2.0, width, fitted)
    }
}

pub trait Scene: 'static {
    fn stamp(&self) -> u64;
    fn paint(&mut self, raster: &mut Raster);
}

struct Gpu {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    picture: Option<(wgpu::Texture, wgpu::BindGroup)>,
}

pub struct RasterSurface<S: Scene> {
    scene: Rc<RefCell<S>>,
    raster: Raster,
    gpu: Option<Gpu>,
    drawn: Option<(u64, u32, u32)>,
}

impl<S: Scene> RasterSurface<S> {
    pub fn new(scene: Rc<RefCell<S>>) -> Self {
        Self {
            scene,
            raster: Raster::default(),
            gpu: None,
            drawn: None,
        }
    }

    fn repaint(&mut self, cx: &SurfaceRenderCx<'_>) -> bool {
        let Ok(mut scene) = self.scene.try_borrow_mut() else {
            return false;
        };
        let key = (scene.stamp(), cx.size.width, cx.size.height);
        if self.drawn == Some(key) {
            return false;
        }
        self.drawn = Some(key);
        self.raster.resize(cx.size.width, cx.size.height);
        self.raster.scale = cx.scale;
        self.raster.aspect = None;
        scene.paint(&mut self.raster);
        true
    }

    fn upload(&mut self, cx: &SurfaceRenderCx<'_>) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let (width, height) = (self.raster.width.max(1), self.raster.height.max(1));
        let fits = gpu
            .picture
            .as_ref()
            .is_some_and(|(texture, _)| texture.width() == width && texture.height() == height);
        if !fits {
            gpu.picture = Some(picture(cx.device, &gpu.layout, width, height));
        }
        let Some((texture, _)) = &gpu.picture else {
            return;
        };
        if self.raster.pixels.len() < width as usize * height as usize * 4 {
            return;
        }
        cx.queue.write_texture(
            texture.as_image_copy(),
            &self.raster.pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    }
}

impl<S: Scene> SurfaceRenderer for RasterSurface<S> {
    fn render(&mut self, cx: &mut SurfaceRenderCx<'_>) {
        cx.request_animation_frame();
        if cx.size.width == 0 || cx.size.height == 0 {
            return;
        }
        let format = cx.texture.format();
        if self.gpu.as_ref().is_none_or(|gpu| gpu.format != format) {
            self.gpu = Some(build(cx.device, format));
            self.drawn = None;
        }
        if self.repaint(cx) {
            self.upload(cx);
        }
        let Some((_, binding)) = self.gpu.as_ref().and_then(|gpu| gpu.picture.as_ref()) else {
            return;
        };
        let Some(gpu) = &self.gpu else {
            return;
        };
        let (x, y, w, h) = letterbox(cx.size.width, cx.size.height, self.raster.aspect);
        let mut encoder = cx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("raster"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("raster.pass"),
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
            if w >= 1.0 && h >= 1.0 {
                pass.set_viewport(x, y, w, h, 0.0, 1.0);
                pass.set_pipeline(&gpu.pipeline);
                pass.set_bind_group(0, binding, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        cx.queue.submit([encoder.finish()]);
    }
}

fn build(device: &wgpu::Device, format: wgpu::TextureFormat) -> Gpu {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("raster.shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("raster.layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("raster.pipeline_layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("raster.pipeline"),
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
    Gpu {
        pipeline,
        layout,
        format,
        picture: None,
    }
}

fn picture(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::BindGroup) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("raster.picture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("raster.binding"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(
                &texture.create_view(&wgpu::TextureViewDescriptor::default()),
            ),
        }],
    });
    (texture, binding)
}

const SHADER: &str = r#"
@group(0) @binding(0) var picture: texture_2d<f32>;

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
    let size = vec2<i32>(textureDimensions(picture));
    let at = clamp(vec2<i32>(in.uv * vec2<f32>(size)), vec2<i32>(0, 0), size - vec2<i32>(1, 1));
    return vec4<f32>(textureLoad(picture, at, 0).rgb, 1.0);
}
"#;

pub fn raster_view<S: Scene>(class: &'static str, scene: Rc<RefCell<S>>) -> impl IntoView {
    zgui::elements::surface()
        .class(class)
        .renderer(RasterSurface::new(scene))
        .into_view()
}

pub fn edit_body(store: Store, node: &str, edit: impl FnOnce(&mut NodeBody)) {
    let node = node.to_owned();
    store.edit_graph(move |graph| {
        if let Some(found) = graph.nodes.iter_mut().find(|found| found.id == node) {
            edit(&mut found.body);
        }
    });
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: f64,
    pub max: f64,
    pub whole: bool,
}

impl Bounds {
    #[must_use]
    pub fn new(min: f64, max: f64) -> Self {
        Self {
            min,
            max,
            whole: false,
        }
    }

    #[must_use]
    pub fn whole(min: f64, max: f64) -> Self {
        Self {
            min,
            max,
            whole: true,
        }
    }

    pub fn read(self, text: &str) -> Result<f64, String> {
        let value: f64 = text
            .trim()
            .parse()
            .map_err(|_| format!("{} is not a number", text.trim()))?;
        if !value.is_finite() || value < self.min || value > self.max {
            return Err(format!("{} to {}", shown(self.min), shown(self.max)));
        }
        Ok(if self.whole { value.round() } else { value })
    }
}

#[must_use]
pub fn shown(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        let text = format!("{value:.6}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

pub fn number(
    label: &str,
    value: Signal<f64>,
    bounds: Bounds,
    commit: impl Fn(f64) + Clone + 'static,
) -> impl IntoView {
    let shown_value = Signal::derive(move || shown(value.get()));
    entry(shown_value, label.to_owned(), false, move |text| {
        commit(bounds.read(&text)?);
        Ok(())
    })
}

pub fn readout(rows: impl Fn() -> Vec<(String, String)> + 'static) -> impl IntoView {
    install_stylesheet("kit-raster", SHEET);
    view! {
        column(class = "rdo") {
            {move || rows()
                .into_iter()
                .map(|(label, value)| view! {
                    row(class = "rdo__row") {
                        text(class = "rdo__label") {{label}}
                        text(class = "rdo__value") {{value}}
                    }
                })
                .collect::<Vec<_>>()}
        }
    }
}

pub fn button(
    label: &'static str,
    disabled: Signal<bool>,
    on_press: impl Fn() + 'static,
) -> impl IntoView {
    view! {
        control(
            class = "btn",
            tabindex = Focus::Sequential,
            a11y:role = Role::Button,
            state:disabled = disabled,
            on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation(),
            on:click = move |_| on_press()
        ) {
            {label}
        }
    }
}

const SHEET: &str = css!(
    r#"
.rdo { gap: 2px; }
.rdo__row { gap: 10px; align-items: baseline; }
.rdo__label {
    flex: 0 0 auto;
    width: 84px;
    font-family: var(--mono);
    font-size: 9px;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--ink-faint);
}
.rdo__value { flex: 1 1 auto; min-width: 0; font-family: var(--mono); font-size: 11px; color: var(--ink); }
"#
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_raster_is_filled_and_read_back_by_pixel() {
        let mut raster = Raster::sized(3, 2);
        raster.fill(PLOT_BG);
        raster.put(2, 1, WHITE);
        assert_eq!(raster.at(2, 1), Some(WHITE));
        assert_eq!(raster.at(0, 0), Some(PLOT_BG));
        assert_eq!(raster.at(3, 0), None);
        assert_eq!(raster.at(-1, 0), None);
    }

    #[test]
    fn blending_moves_part_of_the_way_to_the_ink() {
        let mut raster = Raster::sized(1, 1);
        raster.fill([0, 0, 0, 255]);
        raster.blend(0, 0, [200, 100, 0, 255], 0.5);
        assert_eq!(raster.at(0, 0), Some([100, 50, 0, 255]));
    }

    #[test]
    fn a_line_reaches_both_ends_and_a_dash_leaves_gaps() {
        let mut raster = Raster::sized(10, 1);
        raster.line((0.0, 0.0), (9.0, 0.0), Pen::new(WHITE, 1.0, 1.0));
        assert!((0..10).all(|x| raster.at(x, 0) == Some(WHITE)));
        let mut dashed = Raster::sized(10, 1);
        dashed.line(
            (0.0, 0.0),
            (9.0, 0.0),
            Pen::new(WHITE, 1.0, 1.0).dashed(2.0, 2.0),
        );
        let lit = (0..10).filter(|x| dashed.at(*x, 0) == Some(WHITE)).count();
        assert!(lit > 2 && lit < 10);
    }

    #[test]
    fn drawing_off_the_raster_is_ignored() {
        let mut raster = Raster::sized(2, 2);
        raster.line((-50.0, -50.0), (-10.0, -10.0), Pen::new(WHITE, 1.0, 3.0));
        raster.rect(5.0, 5.0, 9.0, 9.0, WHITE, 1.0);
        assert!(
            raster
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| *pixel == [0, 0, 0, 255])
        );
    }

    #[test]
    fn a_picture_is_letterboxed_to_its_aspect() {
        assert_eq!(letterbox(400, 200, Some(1.0)), (100.0, 0.0, 200.0, 200.0));
        assert_eq!(letterbox(200, 400, Some(2.0)), (0.0, 150.0, 200.0, 100.0));
        assert_eq!(letterbox(300, 100, None), (0.0, 0.0, 300.0, 100.0));
        assert_eq!(letterbox(300, 100, Some(0.0)), (0.0, 0.0, 300.0, 100.0));
    }

    #[test]
    fn a_number_is_read_inside_its_bounds() {
        assert_eq!(Bounds::new(0.01, 100.0).read(" 0.5 "), Ok(0.5));
        assert_eq!(Bounds::whole(2.0, 16.0).read("4.4"), Ok(4.0));
        assert_eq!(
            Bounds::whole(2.0, 16.0).read("17"),
            Err("2 to 16".to_owned())
        );
        assert!(Bounds::new(0.0, 1.0).read("x").is_err());
        assert!(Bounds::new(0.0, 1.0).read("NaN").is_err());
    }

    #[test]
    fn a_number_is_shown_without_trailing_zeros() {
        assert_eq!(shown(4.0), "4");
        assert_eq!(shown(0.35), "0.35");
        assert_eq!(shown(-12.5), "-12.5");
    }
}
