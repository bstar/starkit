//! Offscreen hardware rasterization. No window system or remote GPU is required.
use anyhow::{ensure, Context, Result};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};
use tiny_skia::{
    Color as SkColor, FillRule, Mask, Paint, Path, PathSegment, PixmapPaint, PixmapRef, Shader,
    Stroke, Transform,
};
use vello::{
    kurbo::{Affine, BezPath, Cap, Join, Rect},
    peniko::{Blob, Color, Fill, ImageAlphaType, ImageBrush, ImageData, ImageFormat, ImageQuality},
    wgpu, AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene,
};

const IMAGE_BYTES: usize = 128 * 1024 * 1024;
const IMAGE_COUNT: usize = 2048;
#[derive(Default)]
struct Images {
    images: HashMap<(u64, u32, u32, bool), ImageData>,
    bytes: usize,
}
impl Images {
    fn get(&mut self, bytes: &[u8], width: u32, height: u32, premultiplied: bool) -> ImageData {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hash);
        let key = (hash.finish(), width, height, premultiplied);
        if let Some(image) = self.images.get(&key) {
            if image.data.data() == bytes {
                return image.clone();
            }
        }
        if let Some(old) = self.images.remove(&key) {
            self.bytes = self.bytes.saturating_sub(old.data.data().len());
        }
        if self.bytes.saturating_add(bytes.len()) > IMAGE_BYTES || self.images.len() >= IMAGE_COUNT
        {
            self.images.clear();
            self.bytes = 0;
        }
        let image = ImageData {
            data: Blob::from(bytes.to_vec()),
            format: ImageFormat::Rgba8,
            alpha_type: if premultiplied {
                ImageAlphaType::AlphaPremultiplied
            } else {
                ImageAlphaType::Alpha
            },
            width,
            height,
        };
        self.bytes += bytes.len();
        self.images.insert(key, image.clone());
        image
    }
}
pub(super) struct Canvas {
    pub width: u32,
    pub height: u32,
    pub scene: Scene,
    images: Arc<Mutex<Images>>,
    error: Option<String>,
}
fn affine(t: Transform) -> Affine {
    Affine::new([
        t.sx as f64,
        t.ky as f64,
        t.kx as f64,
        t.sy as f64,
        t.tx as f64,
        t.ty as f64,
    ])
}
fn color(c: SkColor) -> Color {
    Color::from_rgba8(
        (c.red() * 255.).round() as u8,
        (c.green() * 255.).round() as u8,
        (c.blue() * 255.).round() as u8,
        (c.alpha() * 255.).round() as u8,
    )
}
fn path(p: &Path) -> BezPath {
    let mut b = BezPath::new();
    for s in p.segments() {
        match s {
            PathSegment::MoveTo(p) => b.move_to((p.x as f64, p.y as f64)),
            PathSegment::LineTo(p) => b.line_to((p.x as f64, p.y as f64)),
            PathSegment::QuadTo(a, p) => {
                b.quad_to((a.x as f64, a.y as f64), (p.x as f64, p.y as f64))
            }
            PathSegment::CubicTo(a, b0, p) => b.curve_to(
                (a.x as f64, a.y as f64),
                (b0.x as f64, b0.y as f64),
                (p.x as f64, p.y as f64),
            ),
            PathSegment::Close => b.close_path(),
        }
    }
    b
}
impl Canvas {
    fn paint(&mut self, p: &Paint<'_>) -> Option<Color> {
        match p.shader {
            Shader::SolidColor(c) => Some(color(c)),
            _ => {
                self.error = Some("Unsupported GPU paint shader".into());
                None
            }
        }
    }
    fn mask(&mut self, mask: Option<&Mask>) -> bool {
        if mask.is_some() {
            self.error = Some("Unsupported arbitrary GPU raster mask".into());
            false
        } else {
            true
        }
    }
    pub fn fill_color(&mut self, c: SkColor) {
        self.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            color(c),
            None,
            &Rect::new(0., 0., self.width as f64, self.height as f64),
        );
    }
    pub fn fill_rect(
        &mut self,
        r: tiny_skia::Rect,
        p: &Paint<'_>,
        t: Transform,
        m: Option<&Mask>,
    ) -> Option<()> {
        if !self.mask(m) {
            return None;
        }
        if p.blend_mode == tiny_skia::BlendMode::Clear {
            let rect = Rect::new(
                r.left() as f64,
                r.top() as f64,
                r.right() as f64,
                r.bottom() as f64,
            );
            self.scene.push_layer(
                Fill::NonZero,
                vello::peniko::Compose::Clear,
                1.,
                affine(t),
                &rect,
            );
            self.scene
                .fill(Fill::NonZero, affine(t), Color::BLACK, None, &rect);
            self.scene.pop_layer();
            return Some(());
        }
        let c = self.paint(p)?;
        self.scene.fill(
            Fill::NonZero,
            affine(t),
            c,
            None,
            &Rect::new(
                r.left() as f64,
                r.top() as f64,
                r.right() as f64,
                r.bottom() as f64,
            ),
        );
        Some(())
    }
    pub fn fill_path(
        &mut self,
        p: &Path,
        paint: &Paint<'_>,
        r: FillRule,
        t: Transform,
        m: Option<&Mask>,
    ) -> Option<()> {
        if !self.mask(m) {
            return None;
        }
        let c = self.paint(paint)?;
        self.scene.fill(
            match r {
                FillRule::Winding => Fill::NonZero,
                FillRule::EvenOdd => Fill::EvenOdd,
            },
            affine(t),
            c,
            None,
            &path(p),
        );
        Some(())
    }
    pub fn stroke_path(
        &mut self,
        p: &Path,
        paint: &Paint<'_>,
        s: &Stroke,
        t: Transform,
        m: Option<&Mask>,
    ) -> Option<()> {
        if !self.mask(m) {
            return None;
        }
        let c = self.paint(paint)?;
        if s.dash.is_some() {
            self.error = Some("GPU stroke dashes are not supported".into());
            return None;
        }
        let cap = match s.line_cap {
            tiny_skia::LineCap::Butt => Cap::Butt,
            tiny_skia::LineCap::Round => Cap::Round,
            tiny_skia::LineCap::Square => Cap::Square,
        };
        let join = match s.line_join {
            tiny_skia::LineJoin::Round => Join::Round,
            tiny_skia::LineJoin::Bevel => Join::Bevel,
            _ => Join::Miter,
        };
        let style = vello::kurbo::Stroke::new(s.width as f64)
            .with_caps(cap)
            .with_join(join)
            .with_miter_limit(s.miter_limit as f64);
        self.scene.stroke(&style, affine(t), c, None, &path(p));
        Some(())
    }
    pub fn draw_pixmap(
        &mut self,
        x: i32,
        y: i32,
        p: PixmapRef<'_>,
        paint: &PixmapPaint,
        t: Transform,
        m: Option<&Mask>,
    ) {
        if !self.mask(m) {
            return;
        }
        if paint.blend_mode != tiny_skia::BlendMode::SourceOver {
            self.error = Some("Unsupported GPU image blend mode".into());
            return;
        }
        let image =
            self.images
                .lock()
                .expect("GPU image cache")
                .get(p.data(), p.width(), p.height(), true);
        let brush = ImageBrush::new(image)
            .with_alpha(paint.opacity)
            .with_quality(if paint.quality == tiny_skia::FilterQuality::Nearest {
                ImageQuality::Low
            } else {
                ImageQuality::Medium
            });
        self.scene
            .draw_image(&brush, affine(t) * Affine::translate((x as f64, y as f64)));
    }
    pub fn rgba(
        &mut self,
        image: &crate::image::RgbaImage,
        t: Transform,
        clip: [f32; 4],
        nearest: bool,
    ) {
        let data = self.images.lock().expect("GPU image cache").get(
            image.as_raw(),
            image.width(),
            image.height(),
            false,
        );
        let brush = ImageBrush::new(data).with_quality(if nearest {
            ImageQuality::Low
        } else {
            ImageQuality::Medium
        });
        let [x, y, w, h] = clip;
        self.scene.push_clip_layer(
            Fill::NonZero,
            Affine::IDENTITY,
            &Rect::new(x as f64, y as f64, (x + w) as f64, (y + h) as f64),
        );
        self.scene.draw_image(&brush, affine(t));
        self.scene.pop_layer();
    }
}
struct Target {
    texture: wgpu::Texture,
    readback: wgpu::Buffer,
    width: u32,
    height: u32,
    stride: u32,
}
pub(super) struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    target: Option<Target>,
    images: Arc<Mutex<Images>>,
    pub name: String,
    pub backend: String,
}
impl Gpu {
    pub fn new() -> Result<Self> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        }))
        .context("No hardware GPU adapter")?;
        let info = adapter.get_info();
        ensure!(
            !matches!(info.device_type, wgpu::DeviceType::Cpu),
            "Refusing software adapter {} as hardware acceleration",
            info.name
        );
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("STAR/KIT GPU"),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .context("Open GPU device")?;
        let renderer = Renderer::new(
            &device,
            RendererOptions {
                use_cpu: false,
                antialiasing_support: AaSupport::area_only(),
                num_init_threads: std::num::NonZeroUsize::new(1),
                pipeline_cache: None,
            },
        )
        .context("Initialize Vello compute renderer")?;
        tracing::info!(adapter=%info.name,backend=?info.backend,"Hardware scene rasterizer ready");
        Ok(Self {
            device,
            queue,
            renderer,
            target: None,
            images: Arc::new(Mutex::new(Images::default())),
            name: info.name,
            backend: format!("{:?}", info.backend),
        })
    }
    pub fn canvas(&self, width: u32, height: u32) -> Canvas {
        // Vello 0.11's solid-only resolver returns an empty image atlas, which
        // replaces its persistent atlas with a 1x1 texture without invalidating
        // cached image residency. A live-canvas foreground pass often has no
        // artwork: the next frame would reuse atlas coordinates whose text
        // pixels have been discarded. Keep the image resolver active even for
        // empty/vector-only passes with an invisible, cached 1x1 image.
        let sentinel = self
            .images
            .lock()
            .expect("GPU image cache")
            .get(&[0; 4], 1, 1, true);
        let mut scene = Scene::new();
        scene.draw_image(&sentinel, Affine::IDENTITY);
        Canvas {
            width,
            height,
            scene,
            images: self.images.clone(),
            error: None,
        }
    }
    pub fn render(&mut self, canvas: Canvas) -> Result<Vec<u8>> {
        if let Some(error) = canvas.error {
            anyhow::bail!("{error}")
        }
        let (width, height) = (canvas.width, canvas.height);
        ensure!(
            (1..=8192).contains(&width)
                && (1..=8192).contains(&height)
                && u64::from(width) * u64::from(height) <= 32_000_000,
            "GPU frame exceeds bounds"
        );
        if self
            .target
            .as_ref()
            .is_none_or(|t| t.width != width || t.height != height)
        {
            let stride = (width * 4).div_ceil(256) * 256;
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("STAR/KIT RGBA scene"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("STAR/KIT bounded readback"),
                size: u64::from(stride) * u64::from(height),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            self.target = Some(Target {
                texture,
                readback,
                width,
                height,
                stride,
            });
        }
        let target = self.target.as_ref().unwrap();
        let view = target.texture.create_view(&Default::default());
        self.renderer
            .render_to_texture(
                &self.device,
                &self.queue,
                &canvas.scene,
                &view,
                &RenderParams {
                    base_color: Color::TRANSPARENT,
                    width,
                    height,
                    antialiasing_method: AaConfig::Area,
                },
            )
            .context("GPU scene rasterization")?;
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &target.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(target.stride),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::sync_channel(1);
        target
            .readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(10)),
            })
            .context("Wait for GPU readback")?;
        rx.recv_timeout(Duration::from_secs(10))
            .context("GPU readback timeout")??;
        let mapping = target.readback.slice(..).get_mapped_range()?;
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for row in mapping.chunks_exact(target.stride as usize) {
            pixels.extend_from_slice(&row[..width as usize * 4]);
        }
        drop(mapping);
        target.readback.unmap();
        Ok(pixels)
    }
}
