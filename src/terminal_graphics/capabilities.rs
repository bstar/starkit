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
/// input defaults to cells, even when image dimensions are measured pixels.
/// `pixel_mouse` separately reports an enabled SGR pixel mouse mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Actual default background reported by the presenting terminal (OSC 11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_background: Option<[u8; 3]>,
    #[serde(default)]
    pub local_frames: bool,
    /// Bounded encoded live frames, without access to the controller's filesystem.
    #[serde(default)]
    pub streamed_frames: bool,
    /// Low-latency H.264 canvas decoding on the terminal computer.
    #[serde(default)]
    pub canvas_video: bool,
    #[serde(default)]
    pub key_event_types: bool,
    #[serde(default)]
    pub relative_pointer: bool,
    /// Explicit mode; absent for legacy peers that inferred it from images.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell_presentation: Option<bool>,
    #[serde(default)]
    pub presentation_switch: bool,
    #[serde(default)]
    pub original_media: bool,
    #[serde(default)]
    pub video_player: bool,
    #[serde(default)]
    pub audio_relay: bool,
    #[serde(default)]
    pub audio_pcm_v2: bool,
    /// Whole-track PCM caching independent of terminal presentation.
    #[serde(default)]
    pub audio_cache_v1: bool,
    #[serde(default)]
    pub video: bool,
    #[serde(default)]
    pub local_media: bool,
    pub image_transport: ImageTransport,
    pub pixel_geometry: PixelGeometry,
    pub pointer_precision: PointerPrecision,
    /// True only when this frontend has enabled SGR 1016. The baseline enum
    /// remains cell precision so older controllers can deserialize the report.
    #[serde(default)]
    pub pixel_mouse: bool,
    pub keyboard: bool,
    pub paste: bool,
    /// This frontend acknowledges scenes after terminal presentation. Older
    /// version-one clients remain compatible without presentation pacing.
    #[serde(default)]
    pub presentation_ack: bool,
    #[serde(default)]
    pub native_surfaces: bool,
    #[serde(default)]
    pub physical_text: bool,
    #[serde(default)]
    pub native_skins: bool,
    #[serde(default)]
    pub native_paths: bool,
    /// Native surface background hover is painted without controller input.
    #[serde(default)]
    pub surface_hover: bool,
    #[serde(default)]
    pub pixel_layout: bool,
    /// Decode and play animation assets locally without new controller scenes.
    #[serde(default)]
    pub animated_images: bool,
}

impl Capabilities {
    /// Convert a negotiated report to policy without reading environment or IO.
    /// `bridge` means presentation runs on the terminal computer, even when the
    /// application session is remote. `images` comes from the image probe.
    pub fn profile(
        &self,
        env: &crate::presentation::Environment,
        images: &str,
        bridge: bool,
    ) -> crate::presentation::Profile {
        use crate::presentation::{Images, Observed, Profile};
        Profile::resolve(
            env,
            Observed {
                images: match images {
                    "kitty" => Images::Kitty,
                    "sixel" => Images::Sixel,
                    "iterm2" => Images::Iterm2,
                    "halfblocks" => Images::HalfBlocks,
                    _ => Images::None,
                },
                native_renderer: self.native_surfaces,
                measured_pixels: self.pixel_geometry == PixelGeometry::Measured,
                pixel_mouse: self.pixel_mouse,
                audio_frontend: self.audio_relay || self.audio_pcm_v2,
                video_decoder: self.video,
                trusted_paths: self.local_media,
                bridge,
            },
        )
    }
    pub fn detected(graphics: &crate::graphics::Graphics) -> Self {
        let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
        let measured = crate::crossterm::terminal::window_size()
            .is_ok_and(|size| size.width > 0 && size.height > 0);
        let remote_terminal =
            std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some();
        Self::for_terminal(graphics, interactive, measured, remote_terminal)
    }

    fn for_terminal(
        graphics: &crate::graphics::Graphics,
        interactive: bool,
        measured: bool,
        remote_terminal: bool,
    ) -> Self {
        Self {
            terminal_background: None,
            local_frames: false,
            streamed_frames: false,
            canvas_video: cfg!(feature = "media") && !remote_terminal,
            key_event_types: false,
            relative_pointer: false,
            cell_presentation: None,
            presentation_switch: false,
            // Original playback decodes on the presentation machine. An
            // unbridged SSH shell would instead decode on the server and send
            // full-resolution RGBA frames over its TTY. Keep that path on the
            // bounded proxy; local --ssh and Kitty bridge frontends retain it.
            original_media: cfg!(feature = "media") && !remote_terminal,
            video_player: cfg!(feature = "media"),
            // An unbridged remote TTY cannot reach the terminal computer's speakers.
            audio_relay: cfg!(feature = "audio-output") && !remote_terminal,
            audio_pcm_v2: cfg!(feature = "audio-output") && !remote_terminal,
            audio_cache_v1: cfg!(feature = "audio-output") && !remote_terminal,
            video: cfg!(feature = "media"),
            local_media: false,
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
            pixel_mouse: false,
            keyboard: interactive,
            paste: interactive,
            presentation_ack: interactive,
            native_surfaces: true,
            physical_text: true,
            native_skins: true,
            native_paths: true,
            surface_hover: true,
            pixel_layout: true,
            animated_images: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unbridged_ssh_does_not_advertise_local_decode_or_speaker_output() {
        let graphics = crate::graphics::Graphics::disabled();
        let remote = Capabilities::for_terminal(&graphics, true, true, true);
        assert!(!remote.original_media);
        assert!(!remote.audio_relay);
        assert!(!remote.audio_pcm_v2);
        assert!(!remote.audio_cache_v1);
        assert!(remote.keyboard);
        assert_eq!(remote.video, cfg!(feature = "media"));
        assert_eq!(remote.video_player, cfg!(feature = "media"));
        let local_bridge = Capabilities::for_terminal(&graphics, true, true, false);
        assert_eq!(local_bridge.original_media, cfg!(feature = "media"));
        assert_eq!(local_bridge.audio_relay, cfg!(feature = "audio-output"));
        assert_eq!(local_bridge.audio_pcm_v2, cfg!(feature = "audio-output"));
        assert_eq!(local_bridge.audio_cache_v1, cfg!(feature = "audio-output"));
    }

    #[test]
    fn old_frontends_do_not_opt_into_skin_primitives() {
        let mut encoded = serde_json::to_value(Capabilities::detected(
            &crate::graphics::Graphics::disabled(),
        ))
        .unwrap();
        encoded.as_object_mut().unwrap().remove("native_skins");
        encoded.as_object_mut().unwrap().remove("canvas_video");
        encoded.as_object_mut().unwrap().remove("pixel_mouse");
        encoded.as_object_mut().unwrap().remove("surface_hover");
        let old: Capabilities = serde_json::from_value(encoded).unwrap();
        assert!(!old.native_skins);
        assert!(!old.canvas_video);
        assert!(!old.surface_hover);
        assert!(!old.pixel_mouse);
    }
    #[test]
    fn image_support_does_not_claim_pixel_pointer_precision() {
        let capabilities = Capabilities {
            terminal_background: None,
            local_frames: false,
            streamed_frames: false,
            canvas_video: false,
            key_event_types: false,
            relative_pointer: false,
            cell_presentation: None,
            presentation_switch: false,
            original_media: true,
            video_player: cfg!(feature = "media"),
            audio_relay: true,
            audio_pcm_v2: true,
            audio_cache_v1: true,
            video: cfg!(feature = "media"),
            local_media: false,
            image_transport: ImageTransport::Kitty,
            pixel_geometry: PixelGeometry::Measured,
            pointer_precision: PointerPrecision::Cells,
            pixel_mouse: false,
            keyboard: true,
            paste: true,
            presentation_ack: true,
            native_surfaces: true,
            physical_text: true,
            native_skins: true,
            native_paths: true,
            surface_hover: true,
            pixel_layout: true,
            animated_images: true,
        };
        let report = serde_json::to_value(capabilities).unwrap();
        assert_eq!(report["image_transport"], "kitty");
        assert_eq!(report["pixel_geometry"], "measured");
        assert_eq!(report["pointer_precision"], "cells");
        assert_eq!(report["pixel_mouse"], false);
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
