//! Drawing target shared by the reference rasterizer and optional GPU backend.
use tiny_skia::{
    Color, FillRule, Mask, Paint, Path, Pixmap as CpuPixmap, PixmapPaint, PixmapRef, Rect, Stroke,
    Transform,
};

pub(super) enum Canvas {
    Cpu(CpuPixmap),
    #[cfg(feature = "gpu-renderer")]
    Gpu(Box<super::gpu::Canvas>),
}
impl Canvas {
    pub fn new(width: u32, height: u32) -> Option<Self> {
        CpuPixmap::new(width, height).map(Self::Cpu)
    }
    pub fn from_vec(bytes: Vec<u8>, size: tiny_skia::IntSize) -> Option<Self> {
        CpuPixmap::from_vec(bytes, size).map(Self::Cpu)
    }
    pub fn is_gpu(&self) -> bool {
        match self {
            Self::Cpu(_) => false,
            #[cfg(feature = "gpu-renderer")]
            Self::Gpu(_) => true,
        }
    }
    pub fn width(&self) -> u32 {
        match self {
            Self::Cpu(p) => p.width(),
            #[cfg(feature = "gpu-renderer")]
            Self::Gpu(p) => p.width,
        }
    }
    pub fn height(&self) -> u32 {
        match self {
            Self::Cpu(p) => p.height(),
            #[cfg(feature = "gpu-renderer")]
            Self::Gpu(p) => p.height,
        }
    }
    pub fn data(&self) -> &[u8] {
        match self {
            Self::Cpu(p) => p.data(),
            #[cfg(feature = "gpu-renderer")]
            Self::Gpu(_) => panic!("GPU pixels require readback"),
        }
    }
    pub fn data_mut(&mut self) -> &mut [u8] {
        match self {
            Self::Cpu(p) => p.data_mut(),
            #[cfg(feature = "gpu-renderer")]
            Self::Gpu(_) => panic!("GPU drawing must not access CPU pixels"),
        }
    }
    pub fn pixel(&self, x: u32, y: u32) -> Option<tiny_skia::PremultipliedColorU8> {
        match self {
            Self::Cpu(p) => p.pixel(x, y),
            #[cfg(feature = "gpu-renderer")]
            Self::Gpu(_) => None,
        }
    }
    pub fn as_ref(&self) -> PixmapRef<'_> {
        match self {
            Self::Cpu(p) => p.as_ref(),
            #[cfg(feature = "gpu-renderer")]
            Self::Gpu(_) => panic!("GPU scenes cannot be used as CPU image resources"),
        }
    }
    pub fn take(self) -> Vec<u8> {
        match self {
            Self::Cpu(p) => p.take(),
            #[cfg(feature = "gpu-renderer")]
            Self::Gpu(_) => panic!("GPU pixels require readback"),
        }
    }
    pub fn fill(&mut self, color: Color) {
        match self {
            Self::Cpu(p) => p.fill(color),
            #[cfg(feature = "gpu-renderer")]
            Self::Gpu(p) => p.fill_color(color),
        }
    }
    pub fn fill_rect(
        &mut self,
        rect: Rect,
        paint: &Paint<'_>,
        transform: Transform,
        mask: Option<&Mask>,
    ) {
        match self {
            Self::Cpu(p) => p.fill_rect(rect, paint, transform, mask),
            #[cfg(feature = "gpu-renderer")]
            Self::Gpu(p) => {
                let _ = p.fill_rect(rect, paint, transform, mask);
            }
        }
    }
    pub fn fill_path(
        &mut self,
        path: &Path,
        paint: &Paint<'_>,
        rule: FillRule,
        transform: Transform,
        mask: Option<&Mask>,
    ) {
        match self {
            Self::Cpu(p) => p.fill_path(path, paint, rule, transform, mask),
            #[cfg(feature = "gpu-renderer")]
            Self::Gpu(p) => {
                let _ = p.fill_path(path, paint, rule, transform, mask);
            }
        }
    }
    pub fn stroke_path(
        &mut self,
        path: &Path,
        paint: &Paint<'_>,
        stroke: &Stroke,
        transform: Transform,
        mask: Option<&Mask>,
    ) {
        match self {
            Self::Cpu(p) => p.stroke_path(path, paint, stroke, transform, mask),
            #[cfg(feature = "gpu-renderer")]
            Self::Gpu(p) => {
                let _ = p.stroke_path(path, paint, stroke, transform, mask);
            }
        }
    }
    pub fn draw_pixmap(
        &mut self,
        x: i32,
        y: i32,
        image: PixmapRef<'_>,
        paint: &PixmapPaint,
        transform: Transform,
        mask: Option<&Mask>,
    ) {
        match self {
            Self::Cpu(p) => p.draw_pixmap(x, y, image, paint, transform, mask),
            #[cfg(feature = "gpu-renderer")]
            Self::Gpu(p) => p.draw_pixmap(x, y, image, paint, transform, mask),
        }
    }
}
