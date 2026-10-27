//! Specialized content is bounded by a slot solved by KIT. It cannot replace
//! application chrome, claim terminal input modes or own presentation timing.
use super::Id;
use crate::{native_surface::Surface, ratatui::buffer::Buffer};

pub trait Content {
    /// The buffer is local to the content slot. KIT copies only its bounded
    /// cells into the frame, so a content renderer cannot overwrite chrome.
    fn cells(&mut self, _id: &Id, _buffer: &mut Buffer) -> bool {
        false
    }
    /// The surface must have exactly the slot's physical dimensions. Font/grid
    /// hints remain its own; KIT mounts it without scaling the finished text.
    fn native(&mut self, _id: &Id, _size: [u16; 2]) -> Option<Surface> {
        None
    }
    #[cfg(feature = "terminal-graphics")]
    fn canvas(&mut self, _id: &Id) -> Option<Canvas> {
        None
    }
}
pub struct EmptyContent;
impl Content for EmptyContent {}

#[cfg(feature = "terminal-graphics")]
#[derive(Clone, Debug)]
pub struct Canvas {
    pub stream: String,
    pub source: [u32; 2],
    pub interactive: bool,
}
#[cfg(feature = "terminal-graphics")]
impl Canvas {
    pub(crate) fn component(
        &self,
        area: crate::native_surface::PixelRect,
        viewport: crate::terminal_graphics::Viewport,
    ) -> anyhow::Result<crate::terminal_graphics::Component> {
        anyhow::ensure!(
            !self.stream.is_empty() && self.stream.len() <= 128,
            "Invalid canvas stream identity"
        );
        let view = crate::content_view::ContentView::new(self.source, area)?;
        Ok(crate::terminal_graphics::Component::LiveCanvas {
            rect: crate::native_surface::style::logical_rect(area, viewport),
            id: self.stream.clone(),
            view,
            interactive: self.interactive,
        })
    }
}
