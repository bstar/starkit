//! Presentation capabilities are independent of the application protocol.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageTransport {
    None,
    Kitty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelGeometry {
    Estimated,
    Measured,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerPrecision {
    Cells,
}

/// A Kitty image reply does not imply pixel-precise mouse events. Current
/// crossterm input uses cells, even when image dimensions are measured pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub image_transport: ImageTransport,
    pub pixel_geometry: PixelGeometry,
    pub pointer_precision: PointerPrecision,
    pub keyboard: bool,
    pub paste: bool,
}

impl Capabilities {
    pub fn detected(graphics: &crate::graphics::Graphics) -> Self {
        let measured = crate::crossterm::terminal::window_size()
            .is_ok_and(|size| size.width > 0 && size.height > 0);
        Self {
            image_transport: if graphics.name() == "kitty" {
                ImageTransport::Kitty
            } else {
                ImageTransport::None
            },
            pixel_geometry: if measured {
                PixelGeometry::Measured
            } else {
                PixelGeometry::Estimated
            },
            pointer_precision: PointerPrecision::Cells,
            keyboard: true,
            paste: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_support_does_not_claim_pixel_pointer_precision() {
        let capabilities = Capabilities {
            image_transport: ImageTransport::Kitty,
            pixel_geometry: PixelGeometry::Measured,
            pointer_precision: PointerPrecision::Cells,
            keyboard: true,
            paste: true,
        };
        let report = serde_json::to_value(capabilities).unwrap();
        assert_eq!(report["image_transport"], "kitty");
        assert_eq!(report["pixel_geometry"], "measured");
        assert_eq!(report["pointer_precision"], "cells");
        assert_eq!(
            serde_json::from_value::<Capabilities>(report).unwrap(),
            capabilities
        );
    }
}
