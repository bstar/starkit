//! Presentation capabilities are independent of the application protocol.
use serde::{Deserialize, Serialize};
use std::io::IsTerminal as _;

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
    None,
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
    /// This frontend acknowledges scenes after terminal presentation. Older
    /// version-one clients remain compatible without presentation pacing.
    #[serde(default)]
    pub presentation_ack: bool,
    #[serde(default)]
    pub native_surfaces: bool,
}

impl Capabilities {
    pub fn detected(graphics: &crate::graphics::Graphics) -> Self {
        let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
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
            pointer_precision: if interactive {
                PointerPrecision::Cells
            } else {
                PointerPrecision::None
            },
            keyboard: interactive,
            paste: interactive,
            presentation_ack: interactive,
            native_surfaces: true,
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
            presentation_ack: true,
            native_surfaces: true,
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

    #[test]
    fn version_one_clients_without_a_capability_report_still_attach() {
        let message: super::super::protocol::ClientMessage = serde_json::from_str(
            r#"{"type":"hello","version":1,"viewport":{"columns":80,"rows":24,"width":800,"height":480,"generation":1},"client":"legacy"}"#,
        ).unwrap();
        assert!(matches!(
            message,
            super::super::protocol::ClientMessage::Hello {
                capabilities: None,
                ..
            }
        ));
    }
}
