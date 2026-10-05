//! Lossless compressed animation frames with a bounded decoded working set.
use crate::image::{AnimationDecoder, DynamicImage, ImageFormat, RgbaImage};
use std::{
    io::Cursor,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Debug)]
struct Frame {
    png: Vec<u8>,
    delay: Duration,
}

#[derive(Debug)]
pub struct Animation {
    frames: Vec<Frame>,
    pub total: Duration,
    pub plays: u32,
    pub truncated: bool,
    dimensions: (u32, u32),
    first: Arc<RgbaImage>,
    cache: Mutex<Option<(usize, Arc<RgbaImage>)>>,
}
impl Animation {
    /// Consume composited frames on a worker. Memory is charged to their lossless
    /// compressed size, rather than width × height × the entire frame count.
    pub fn collect(
        frames: crate::image::Frames<'_>,
        plays: u32,
        max_bytes: usize,
        mut stop: impl FnMut() -> bool,
    ) -> anyhow::Result<Self> {
        let mut result = Self {
            frames: Vec::new(),
            total: Duration::ZERO,
            plays,
            truncated: false,
            dimensions: (0, 0),
            first: Arc::new(RgbaImage::new(0, 0)),
            cache: Mutex::new(None),
        };
        let mut bytes = 0usize;
        for frame in frames {
            anyhow::ensure!(!stop(), "Animation cancelled or timed out");
            let frame = frame?;
            let (n, d) = frame.delay().numer_denom_ms();
            let delay = Duration::from_millis(u64::from(n.checked_div(d).unwrap_or(0)))
                .max(crate::anim::MIN_DELAY);
            let image = frame.into_buffer();
            anyhow::ensure!(
                u64::from(image.width()) * u64::from(image.height()) <= 8_000_000,
                "Animation dimensions exceed limit"
            );
            if result.frames.is_empty() {
                result.dimensions = image.dimensions();
                result.first = Arc::new(image.clone());
            }
            anyhow::ensure!(
                image.dimensions() == result.dimensions,
                "Animation dimensions differ"
            );
            let mut png = Cursor::new(Vec::new());
            DynamicImage::ImageRgba8(image).write_to(&mut png, ImageFormat::Png)?;
            let png = png.into_inner();
            if bytes.saturating_add(png.len()) > max_bytes || result.frames.len() >= 4096 {
                result.truncated = true;
                break;
            }
            bytes += png.len();
            result.total += delay;
            result.frames.push(Frame { png, delay });
        }
        anyhow::ensure!(
            !result.frames.is_empty(),
            "Animation has no frames within budget"
        );
        Ok(result)
    }
    pub fn gif(bytes: &[u8], max_bytes: usize, stop: impl FnMut() -> bool) -> anyhow::Result<Self> {
        use crate::image::ImageDecoder as _;
        let mut decoder = crate::image::codecs::gif::GifDecoder::new(Cursor::new(bytes))?;
        let (w, h) = decoder.dimensions();
        anyhow::ensure!(
            u64::from(w) * u64::from(h) <= 8_000_000,
            "Animation dimensions exceed limit"
        );
        let mut limits = crate::image::Limits::default();
        limits.max_alloc = Some(64_000_000);
        decoder.set_limits(limits)?;
        let plays = match decoder.loop_count() {
            crate::image::metadata::LoopCount::Infinite => 0,
            crate::image::metadata::LoopCount::Finite(n) => n.get(),
        };
        Self::collect(decoder.into_frames(), plays, max_bytes, stop)
    }
    pub fn len(&self) -> usize {
        self.frames.len()
    }
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
    pub fn dimensions(&self) -> (u32, u32) {
        self.dimensions
    }
    pub fn bytes(&self) -> usize {
        self.frames.iter().map(|f| f.png.len()).sum()
    }
    pub fn delay(&self, index: usize) -> Duration {
        self.frames[index].delay
    }
    pub fn finished(&self, elapsed: Duration) -> bool {
        self.plays > 0 && elapsed >= self.total.saturating_mul(self.plays)
    }
    pub fn index(&self, elapsed: Duration) -> usize {
        if self.finished(elapsed) {
            return self.len() - 1;
        }
        let mut t = Duration::from_nanos((elapsed.as_nanos() % self.total.as_nanos()) as u64);
        for (index, frame) in self.frames.iter().enumerate() {
            if t < frame.delay {
                return index;
            }
            t -= frame.delay;
        }
        self.len() - 1
    }
    /// Only the current frame stays decoded. All stored PNGs were produced here.
    pub fn frame(&self, index: usize) -> anyhow::Result<Arc<RgbaImage>> {
        if index == 0 {
            return Ok(Arc::clone(&self.first));
        }
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| anyhow::anyhow!("Animation cache unavailable"))?;
        if let Some((old, image)) = &*cache {
            if *old == index {
                return Ok(Arc::clone(image));
            }
        }
        let bytes = &self.frames[index].png;
        let image = Arc::new(
            crate::image::load_from_memory_with_format(bytes, ImageFormat::Png)?.into_rgba8(),
        );
        *cache = Some((index, Arc::clone(&image)));
        Ok(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::{Delay, Frame as ImageFrame, Rgba};
    fn frames(count: usize) -> crate::image::Frames<'static> {
        crate::image::Frames::new(Box::new((0..count).map(|index| {
            let image = RgbaImage::from_fn(128, 128, |x, _| {
                if x == index as u32 {
                    Rgba([255, 255, 255, 255])
                } else {
                    Rgba([0, 0, 0, 255])
                }
            });
            Ok(ImageFrame::from_parts(
                image,
                0,
                0,
                Delay::from_numer_denom_ms(20, 1),
            ))
        })))
    }
    #[test]
    fn long_animation_keeps_every_pixel_and_frame_within_compressed_budget() {
        let animation = Animation::collect(frames(80), 0, 64_000, || false).unwrap();
        assert_eq!(animation.len(), 80);
        assert!(!animation.truncated);
        assert!(animation.bytes() <= 64_000);
        assert_eq!(animation.total, Duration::from_millis(1600));
        assert_eq!(
            *animation.frame(79).unwrap().get_pixel(79, 0),
            Rgba([255, 255, 255, 255])
        );
        assert_eq!(animation.index(Duration::from_millis(1590)), 79);
        let first = animation.frame(0).unwrap();
        animation.frame(70).unwrap();
        assert!(
            Arc::ptr_eq(&first, &animation.frame(0).unwrap()),
            "first frame identity must survive thumbnail encoding"
        );
    }
    #[test]
    fn cancellation_and_budget_are_enforced() {
        assert!(Animation::collect(frames(80), 0, 64_000, || true).is_err());
        let animation = Animation::collect(frames(80), 0, 2000, || false).unwrap();
        assert!(animation.truncated);
        assert!(animation.len() < 80);
        assert!(animation.bytes() <= 2000);
    }
}
