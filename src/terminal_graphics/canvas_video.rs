//! Movie-style local decoding for interactive canvases, without a playback clock.
//! One access unit in flight bounds work; interdependent packets are never dropped.
use super::{
    media::{
        ToClient, ToHost, CANVAS_VIDEO_MAX_HEIGHT, CANVAS_VIDEO_MAX_PACKET, CANVAS_VIDEO_MAX_WIDTH,
    },
    protocol::ClientMessage,
};
use anyhow::{ensure, Context, Result};
use base64::Engine as _;
use crossbeam_channel::{bounded, Receiver, Sender};
use std::sync::Arc;

const MAX_PACKET: usize = CANVAS_VIDEO_MAX_PACKET;
type Decoded = Result<Arc<crate::image::RgbaImage>, String>;
struct Active {
    session: u64,
    generation: u64,
    stream: String,
    input: Sender<Vec<u8>>,
    output: Receiver<Decoded>,
    outstanding: bool,
    credit: bool,
    failure: Option<String>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Active {
    fn drop(&mut self) {
        let (closed, _) = bounded(1);
        drop(std::mem::replace(&mut self.input, closed));
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[derive(Default)]
pub(super) struct Frontend {
    active: Option<Active>,
}
impl Frontend {
    pub fn reset(&mut self) {
        self.active = None;
    }
    pub fn receive(&mut self, message: &ToClient) -> Result<bool> {
        match message {
            ToClient::CanvasVideoOpen {
                session,
                generation,
                stream,
                width,
                height,
            } => {
                ensure!(
                    (2..=CANVAS_VIDEO_MAX_WIDTH).contains(width)
                        && (2..=CANVAS_VIDEO_MAX_HEIGHT).contains(height)
                        && !stream.is_empty()
                        && stream.len() <= 96
                        && stream
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b)),
                    "Invalid live video bounds or stream"
                );
                self.reset();
                let (input, packets) = bounded::<Vec<u8>>(1);
                let (frames, output) = bounded(1);
                let (width, height) = (*width, *height);
                let worker = std::thread::Builder::new().name("star-canvas-decoder".into()).spawn(move || {
                    let run = || -> Result<()> {
                        crate::media::init()?;
                        use ffmpeg_next as av;
                        av::decoder::find(av::codec::Id::H264).context("H.264 decoder unavailable")?;
                        let mut parameters = av::codec::Parameters::new();
                        parameters.set_id(av::codec::Id::H264);
                        parameters.set_medium(av::media::Type::Video);
                        let mut decoder = crate::media::hardware::decoder(parameters.clone())?;
                        let mut scale = None;
                        let mut first = true;
                        for bytes in packets {
                            let packet = av::Packet::copy(&bytes);
                            let decode = |decoder: &mut av::codec::decoder::Video| -> Result<_> {
                                // This worker owns the decoder. Bound allocations before
                                // submitting any untrusted access unit (including SPS data).
                                unsafe { (*decoder.as_mut_ptr()).max_pixels = i64::from(width.div_ceil(16) * 16) * i64::from(height.div_ceil(16) * 16); }
                                decoder.send_packet(&packet)?;
                                let mut frame = av::frame::Video::empty();
                                decoder.receive_frame(&mut frame)?;
                                crate::media::hardware::download(&frame)
                            };
                            let frame = match decode(&mut decoder) {
                                Ok(frame) => frame,
                                Err(error) if first => {
                                    tracing::warn!(%error, "Live video device rejected first frame; trying software");
                                    decoder = crate::media::video_decoder(parameters.clone())?;
                                    decode(&mut decoder)?
                                }
                                Err(error) => return Err(error),
                            };
                            first = false;
                            ensure!(frame.width() == width && frame.height() == height, "Live video changed dimensions without reopening");
                            let pixels = crate::media::rgba(&frame, &mut scale, (width, height))?;
                            if frames.send(Ok(Arc::new(pixels))).is_err() { break; }
                        }
                        Ok(())
                    };
                    if let Err(error) = run() { let _ = frames.try_send(Err(format!("{error:#}"))); }
                })?;
                self.active = Some(Active {
                    session: *session,
                    generation: *generation,
                    stream: stream.clone(),
                    input,
                    output,
                    outstanding: false,
                    credit: true,
                    failure: None,
                    worker: Some(worker),
                });
            }
            ToClient::CanvasVideoPacket {
                session,
                generation,
                data,
            } => {
                if let Some(a) = self
                    .active
                    .as_mut()
                    .filter(|a| a.session == *session && a.generation == *generation)
                {
                    ensure!(
                        !a.outstanding && !a.credit,
                        "Live video exceeded its frame credit"
                    );
                    ensure!(
                        data.len() <= MAX_PACKET.div_ceil(3) * 4,
                        "Live video packet exceeds limit"
                    );
                    let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
                    ensure!(
                        !bytes.is_empty() && bytes.len() <= MAX_PACKET,
                        "Invalid live video packet"
                    );
                    a.input
                        .try_send(bytes)
                        .context("Live video decoder unavailable")?;
                    a.outstanding = true;
                }
            }
            ToClient::CanvasVideoClose {
                session,
                generation,
                ..
            } => {
                if self
                    .active
                    .as_ref()
                    .is_some_and(|a| a.session == *session && a.generation == *generation)
                {
                    self.reset();
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    pub fn tick(
        &mut self,
        out: &Sender<ClientMessage>,
    ) -> Option<(String, u64, Arc<crate::image::RgbaImage>)> {
        let a = self.active.as_mut()?;
        let mut frame = None;
        match a.output.try_recv() {
            Ok(Ok(pixels)) => {
                a.outstanding = false;
                a.credit = true;
                frame = Some((a.stream.clone(), a.generation, pixels));
            }
            Ok(Err(message)) => {
                a.failure = Some(message);
            }
            Err(_) => {}
        }
        if let Some(message) = &a.failure {
            if out
                .try_send(ClientMessage::Media {
                    message: ToHost::CanvasVideoError {
                        session: a.session,
                        generation: a.generation,
                        message: message.clone(),
                    },
                })
                .is_ok()
            {
                self.reset();
            }
            return None;
        }
        if a.credit
            && out
                .try_send(ClientMessage::Media {
                    message: ToHost::CanvasVideoCredit {
                        session: a.session,
                        generation: a.generation,
                    },
                })
                .is_ok()
        {
            a.credit = false;
        }
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn open(session: u64) -> ToClient {
        ToClient::CanvasVideoOpen {
            session,
            generation: 7,
            stream: "desktop".into(),
            width: 64,
            height: 32,
        }
    }
    fn packets() -> Vec<String> {
        packets_at(64, 32, ffmpeg_next::format::Pixel::YUV420P)
    }
    fn packets_at(width: u32, height: u32, format: ffmpeg_next::format::Pixel) -> Vec<String> {
        use ffmpeg_next as av;
        crate::media::init().unwrap();
        let codec = av::encoder::find_by_name("libx264").unwrap();
        let mut context = av::codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()
            .unwrap();
        context.set_width(width);
        context.set_height(height);
        context.set_format(format);
        context.set_time_base((1, 60));
        context.set_max_b_frames(0);
        let mut options = av::Dictionary::new();
        options.set("preset", "ultrafast");
        options.set("tune", "zerolatency");
        options.set("x264-params", "repeat-headers=1");
        let mut encoder = context.open_with(options).unwrap();
        let mut packets = vec![];
        for i in 0..3 {
            let mut frame = av::frame::Video::new(format, width, height);
            frame.data_mut(0).fill(60 + i * 30);
            frame.data_mut(1).fill(128);
            frame.data_mut(2).fill(128);
            if format == av::format::Pixel::YUV444P {
                for plane in [1, 2] {
                    let stride = frame.stride(plane);
                    for row in frame
                        .data_mut(plane)
                        .chunks_mut(stride)
                        .take(height as usize)
                    {
                        for (x, value) in row.iter_mut().take(width as usize).enumerate() {
                            *value = if (x + plane) % 2 == 0 { 40 } else { 220 };
                        }
                    }
                }
            }
            frame.set_pts(Some(i64::from(i)));
            encoder.send_frame(&frame).unwrap();
            let mut packet = av::Packet::empty();
            encoder.receive_packet(&mut packet).unwrap();
            packets.push(base64::engine::general_purpose::STANDARD.encode(packet.data().unwrap()));
        }
        packets
    }
    #[test]
    fn live_video_decodes_interdependent_frames_and_credits_without_a_movie_clock() {
        let mut client = Frontend::default();
        let (out, replies) = bounded(4);
        client.receive(&open(1)).unwrap();
        assert!(client.tick(&out).is_none());
        assert!(matches!(
            replies.try_recv().unwrap(),
            ClientMessage::Media {
                message: ToHost::CanvasVideoCredit {
                    session: 1,
                    generation: 7
                }
            }
        ));
        let mut brightness = 0;
        for data in packets() {
            client
                .receive(&ToClient::CanvasVideoPacket {
                    session: 1,
                    generation: 7,
                    data,
                })
                .unwrap();
            // A second access unit cannot slip into a queued decoder backlog.
            assert!(client
                .receive(&ToClient::CanvasVideoPacket {
                    session: 1,
                    generation: 7,
                    data: "AA==".into()
                })
                .is_err());
            let deadline = Instant::now() + Duration::from_secs(2);
            let (stream, generation, pixels) = loop {
                if let Some(frame) = client.tick(&out) {
                    break frame;
                }
                assert!(
                    Instant::now() < deadline,
                    "no decoded canvas frame: {:?}",
                    replies.try_iter().collect::<Vec<_>>()
                );
                std::thread::sleep(Duration::from_millis(1));
            };
            assert_eq!(
                (stream.as_str(), generation, pixels.dimensions()),
                ("desktop", 7, (64, 32))
            );
            assert!(pixels.get_pixel(32, 16)[0] > brightness);
            brightness = pixels.get_pixel(32, 16)[0];
            assert!(matches!(
                replies.try_recv().unwrap(),
                ClientMessage::Media {
                    message: ToHost::CanvasVideoCredit { .. }
                }
            ));
        }
        client.receive(&open(2)).unwrap();
        // A packet/close from a retired epoch must not touch the new decoder.
        client
            .receive(&ToClient::CanvasVideoPacket {
                session: 1,
                generation: 7,
                data: "invalid".into(),
            })
            .unwrap();
        client
            .receive(&ToClient::CanvasVideoClose {
                session: 1,
                generation: 7,
                stream: "desktop".into(),
            })
            .unwrap();
        assert_eq!(client.active.as_ref().unwrap().session, 2);
        client.reset();
        assert!(client.active.is_none());
    }
    #[test]
    fn full_hd_444_decode_preserves_single_pixel_color_edges() {
        let mut client = Frontend::default();
        let (out, replies) = bounded(4);
        client
            .receive(&ToClient::CanvasVideoOpen {
                session: 42,
                generation: 7,
                stream: "desktop".into(),
                width: 1920,
                height: 1080,
            })
            .unwrap();
        client.tick(&out);
        let _ = replies.try_recv();
        client
            .receive(&ToClient::CanvasVideoPacket {
                session: 42,
                generation: 7,
                data: packets_at(1920, 1080, ffmpeg_next::format::Pixel::YUV444P).remove(0),
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (_, _, pixels) = loop {
            if let Some(frame) = client.tick(&out) {
                break frame;
            }
            assert!(
                Instant::now() < deadline,
                "Full-HD 4:4:4 decode failed: {:?}",
                replies.try_iter().collect::<Vec<_>>()
            );
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(pixels.dimensions(), (1920, 1080));
        let a = pixels.get_pixel(960, 540);
        let b = pixels.get_pixel(961, 540);
        assert!(
            a[0].abs_diff(b[0]) > 100 && a[2].abs_diff(b[2]) > 100,
            "One-pixel chroma edges were averaged: {a:?}, {b:?}"
        );
    }

    #[test]
    fn native_canvas_accepts_full_hd_and_4k_without_resizing() {
        for (width, height) in [(1920, 1080), (3840, 2160)] {
            let mut client = Frontend::default();
            client
                .receive(&ToClient::CanvasVideoOpen {
                    session: 1,
                    generation: 1,
                    stream: "desktop".into(),
                    width,
                    height,
                })
                .unwrap();
            assert!(client.active.is_some());
            client.reset();
        }
    }

    #[test]
    fn live_video_rejects_bad_bounds_and_oversized_packets_before_decode() {
        let mut client = Frontend::default();
        let mut message = open(1);
        if let ToClient::CanvasVideoOpen { width, .. } = &mut message {
            *width = 8192;
        }
        assert!(client.receive(&message).is_err());
        client.receive(&open(1)).unwrap();
        let (out, _replies) = bounded(2);
        client.tick(&out);
        assert!(client
            .receive(&ToClient::CanvasVideoPacket {
                session: 1,
                generation: 7,
                data: "A".repeat(MAX_PACKET * 2)
            })
            .is_err());
    }
}
