//! Bounded stereo PCM playback on the presentation machine.
use super::{
    media::{ToClient, ToHost},
    protocol::ClientMessage,
};
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{bounded, Receiver, Sender};
use ffmpeg_next as av;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

pub const BLOCK_SAMPLES: usize = 1920;
pub const WINDOW_BLOCKS: usize = 8;
struct Active {
    session: u64,
    epoch: u64,
    blocks: Sender<Vec<i16>>,
    credits: Receiver<()>,
    warnings: Receiver<String>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    pending: usize,
    failure: Option<String>,
}
impl Drop for Active {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[derive(Default)]
pub struct Frontend {
    active: Option<Active>,
}
impl Frontend {
    pub fn reset(&mut self) {
        self.active = None;
    }
    pub fn receive(&mut self, message: &ToClient) -> Result<bool> {
        match message {
            ToClient::AudioOpen { session, epoch } => {
                self.active = None;
                let (blocks, receive) = bounded(WINDOW_BLOCKS);
                let (credit, credits) = bounded(WINDOW_BLOCKS);
                let (warning, warnings) = bounded(1);
                let stop = Arc::new(AtomicBool::new(false));
                let cancel = stop.clone();
                let worker = std::thread::Builder::new()
                    .name("star-audio-relay".into())
                    .spawn(move || {
                        if let Err(error) = play(receive, credit, cancel.clone()) {
                            if !cancel.load(Ordering::Relaxed) {
                                let _ = warning.try_send(format!("Local audio: {error:#}"));
                            }
                        }
                    })?;
                self.active = Some(Active {
                    session: *session,
                    epoch: *epoch,
                    blocks,
                    credits,
                    warnings,
                    stop,
                    worker: Some(worker),
                    pending: WINDOW_BLOCKS,
                    failure: None,
                });
            }
            ToClient::AudioChunk {
                session,
                epoch,
                samples,
            } => {
                anyhow::ensure!(valid_samples(samples), "Invalid audio block");
                if let Some(active) = self
                    .active
                    .as_mut()
                    .filter(|a| a.session == *session && a.epoch == *epoch)
                {
                    active.blocks.try_send(samples.clone()).map_err(|_| {
                        anyhow::anyhow!("Audio receiver exceeded its credit window")
                    })?;
                }
            }
            ToClient::AudioClose { session } => {
                if self.active.as_ref().is_some_and(|a| a.session == *session) {
                    self.active = None;
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    pub fn tick(&mut self, out: &Sender<ClientMessage>) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if let Ok(message) = active.warnings.try_recv() {
            active.failure = Some(message);
            active.stop.store(true, Ordering::Relaxed);
        }
        if active.stop.load(Ordering::Relaxed) {
            if let Some(message) = &active.failure {
                if out
                    .try_send(ClientMessage::Media {
                        message: ToHost::AudioError {
                            session: active.session,
                            epoch: active.epoch,
                            message: message.clone(),
                        },
                    })
                    .is_ok()
                {
                    active.failure = None;
                }
            }
            return;
        }
        active.pending += active.credits.try_iter().count();
        if active.pending > 0
            && out
                .try_send(ClientMessage::Media {
                    message: ToHost::AudioCredit {
                        session: active.session,
                        epoch: active.epoch,
                        blocks: active.pending.min(WINDOW_BLOCKS),
                    },
                })
                .is_ok()
        {
            active.pending = 0;
        }
    }
}
fn valid_samples(samples: &[i16]) -> bool {
    !samples.is_empty() && samples.len() <= BLOCK_SAMPLES && samples.len().is_multiple_of(2)
}
fn play(blocks: Receiver<Vec<i16>>, credits: Sender<()>, stop: Arc<AtomicBool>) -> Result<()> {
    let device = cpal::default_host()
        .default_output_device()
        .context("No local audio device")?;
    let supported = device.default_output_config()?;
    let config = supported.config();
    let channels = usize::from(config.channels);
    let rate = config.sample_rate.0;
    anyhow::ensure!(
        (1..=8).contains(&channels) && (8000..=192000).contains(&rate),
        "Unsupported local audio format"
    );
    let (mut producer, consumer) = rtrb::RingBuffer::<f32>::new(rate as usize * channels / 10);
    let mut consumer = Some(consumer);
    let (errors, error) = bounded::<String>(1);
    macro_rules! stream {
        ($ty:ty) => {{
            let mut ring = consumer.take().unwrap();
            let errors = errors.clone();
            device.build_output_stream(
                &config,
                move |out: &mut [$ty], _| {
                    for value in out {
                        *value =
                            <$ty as cpal::FromSample<f32>>::from_sample_(ring.pop().unwrap_or(0.0));
                    }
                },
                move |e| {
                    let _ = errors.try_send(e.to_string());
                },
                None,
            )?
        }};
    }
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => stream!(f32),
        cpal::SampleFormat::I16 => stream!(i16),
        cpal::SampleFormat::U16 => stream!(u16),
        _ => anyhow::bail!("Unsupported local audio sample format"),
    };
    stream.play()?;
    let mut resample = PcmResampler::new(channels as u16, rate)?;
    while !stop.load(Ordering::Relaxed) {
        if let Ok(message) = error.try_recv() {
            anyhow::bail!("{message}");
        }
        let samples = match blocks.recv_timeout(Duration::from_millis(10)) {
            Ok(samples) => samples,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };
        for mut value in resample.convert(&samples)? {
            loop {
                if stop.load(Ordering::Relaxed) {
                    return Ok(());
                }
                if let Ok(message) = error.try_recv() {
                    anyhow::bail!("{message}");
                }
                match producer.push(value) {
                    Ok(()) => break,
                    Err(rtrb::PushError::Full(v)) => {
                        value = v;
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
            }
        }
        let _ = credits.try_send(());
    }
    Ok(())
}
struct PcmResampler {
    context: av::software::resampling::Context,
}
impl PcmResampler {
    fn new(channels: u16, rate: u32) -> Result<Self> {
        av::init()?;
        Ok(Self {
            context: av::software::resampling::Context::get(
                av::format::Sample::I16(av::format::sample::Type::Packed),
                av::ChannelLayout::STEREO,
                48000,
                av::format::Sample::F32(av::format::sample::Type::Packed),
                av::ChannelLayout::default(i32::from(channels)),
                rate,
            )?,
        })
    }
    fn convert(&mut self, samples: &[i16]) -> Result<Vec<f32>> {
        anyhow::ensure!(valid_samples(samples), "Invalid PCM block");
        let mut input = av::frame::Audio::new(
            av::format::Sample::I16(av::format::sample::Type::Packed),
            samples.len() / 2,
            av::ChannelLayout::STEREO,
        );
        input.set_rate(48000);
        for (bytes, sample) in input
            .data_mut(0)
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(samples)
        {
            bytes.copy_from_slice(&sample.to_ne_bytes());
        }
        let mut output = av::frame::Audio::empty();
        self.context.run(&input, &mut output)?;
        let bytes = &output.data(0)[..output.samples() * usize::from(output.channels()) * 4];
        Ok(bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f32::from_ne_bytes(*bytes))
            .collect())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pcm_conversion_preserves_volume_and_resamples_continuously() {
        let mut native = PcmResampler::new(2, 48000).unwrap();
        let output = native.convert(&vec![16384; BLOCK_SAMPLES]).unwrap();
        assert_eq!(output.len(), BLOCK_SAMPLES);
        assert!(output.iter().all(|sample| (*sample - 0.5).abs() < 0.0001));
        let mut mono = PcmResampler::new(1, 44100).unwrap();
        let count: usize = (0..100)
            .map(|_| mono.convert(&vec![16384; BLOCK_SAMPLES]).unwrap().len())
            .sum();
        assert!((88136..=88200).contains(&count));
    }
    #[test]
    fn device_error_is_retried_when_the_control_queue_is_full() {
        let (blocks, _) = bounded(8);
        let (_, credits) = bounded(8);
        let (_, warnings) = bounded(1);
        let mut frontend = Frontend {
            active: Some(Active {
                session: 1,
                epoch: 2,
                blocks,
                credits,
                warnings,
                stop: Arc::new(AtomicBool::new(true)),
                worker: None,
                pending: 8,
                failure: Some("device unavailable".into()),
            }),
        };
        let (out, receive) = bounded(1);
        out.send(ClientMessage::Media {
            message: ToHost::AudioCredit {
                session: 0,
                epoch: 0,
                blocks: 1,
            },
        })
        .unwrap();
        frontend.tick(&out);
        assert!(frontend.active.as_ref().unwrap().failure.is_some());
        receive.recv().unwrap();
        frontend.tick(&out);
        assert!(matches!(
            receive.recv().unwrap(),
            ClientMessage::Media {
                message: ToHost::AudioError {
                    session: 1,
                    epoch: 2,
                    ..
                }
            }
        ));
        assert!(frontend.active.as_ref().unwrap().failure.is_none());
    }
    proptest::proptest! {
    #[test]
        fn pcm_bounds_hold_for_arbitrary_lengths(samples in proptest::collection::vec(proptest::prelude::any::<i16>(), 0..4096)) {
            proptest::prop_assert_eq!(valid_samples(&samples), !samples.is_empty() && samples.len() <= BLOCK_SAMPLES && samples.len() % 2 == 0);
        }
    }
    #[test]
    fn reject_unbounded_and_partial_stereo_blocks() {
        assert!(valid_samples(&vec![0; BLOCK_SAMPLES]));
        assert!(!valid_samples(&[]));
        assert!(!valid_samples(&[0]));
        assert!(!valid_samples(&vec![0; BLOCK_SAMPLES + 2]));
    }
}
