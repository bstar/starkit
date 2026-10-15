//! A graphics-only frontend reports missing engines rather than opening devices.
use super::{ToClient, ToHost};
use crate::terminal_graphics::protocol::ClientMessage;
use crossbeam_channel::Sender;

pub struct Frontend {
    #[cfg(feature = "audio-output")]
    audio: super::super::audio::Frontend,
}
impl Frontend {
    pub fn new(_local: bool) -> Self {
        Self {
            #[cfg(feature = "audio-output")]
            audio: Default::default(),
        }
    }
    pub fn reset(&mut self) {
        #[cfg(feature = "audio-output")]
        self.audio.reset();
    }
    pub fn video_playing(&self) -> bool {
        false
    }
    pub(crate) fn canvas_tick(
        &mut self,
        _out: &Sender<ClientMessage>,
    ) -> Option<(String, u64, std::sync::Arc<crate::image::RgbaImage>)> {
        None
    }
    pub fn tick(
        &mut self,
        _out: &Sender<ClientMessage>,
    ) -> Option<(String, std::sync::Arc<crate::image::RgbaImage>)> {
        #[cfg(feature = "audio-output")]
        self.audio.tick(_out);
        None
    }
    pub fn receive(
        &mut self,
        message: ToClient,
        out: &Sender<ClientMessage>,
    ) -> anyhow::Result<()> {
        #[cfg(feature = "audio-output")]
        if self.audio.receive(&message)? {
            return Ok(());
        }
        let reply = match message {
            ToClient::CanvasVideoOpen {
                session,
                generation,
                ..
            } => Some(ToHost::CanvasVideoError {
                session,
                generation,
                message: "This frontend was built without live video decoding".into(),
            }),
            ToClient::Open {
                session,
                generation,
                ..
            }
            | ToClient::OriginalOpen {
                session,
                generation,
                ..
            } => Some(ToHost::Status {
                session,
                generation,
                position: 0.,
                buffering: false,
                finished: true,
                warning: Some("This frontend was built without media support".into()),
                bandwidth_bps: 0,
                buffered_bytes: 0,
                dropped_frames: 0,
                source_bitrate: 0,
            }),
            ToClient::AudioOpen { session, epoch } | ToClient::PcmOpen { session, epoch, .. } => {
                Some(ToHost::AudioError {
                    session,
                    epoch,
                    message: "This frontend was built without media support".into(),
                })
            }
            _ => None,
        };
        if let Some(message) = reply {
            out.try_send(ClientMessage::Media { message })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unavailable_video_returns_a_terminal_status_without_credit_or_device_work() {
        let mut frontend = Frontend::new(false);
        let (out, rx) = crossbeam_channel::bounded(2);
        frontend
            .receive(
                ToClient::Open {
                    session: 1,
                    generation: 2,
                    image: "frame".into(),
                    local: Some("untrusted".into()),
                    start: 0.,
                    quality: crate::media_types::Quality::Balanced,
                    options: Default::default(),
                },
                &out,
            )
            .unwrap();
        assert!(matches!(
            rx.try_recv().unwrap(),
            ClientMessage::Media {
                message: ToHost::Status {
                    session: 1,
                    generation: 2,
                    finished: true,
                    warning: Some(_),
                    ..
                }
            }
        ));
        assert!(frontend.tick(&out).is_none() && !frontend.video_playing());
    }
}
