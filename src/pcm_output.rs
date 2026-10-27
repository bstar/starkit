//! Native PCM device selection and float64 conversion at the final boundary.
use crate::pcm::{Quantizer, SampleFormat, Spec};
use anyhow::{bail, ensure, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};

pub struct Plan {
    pub device: cpal::Device,
    pub config: cpal::StreamConfig,
    pub format: cpal::SampleFormat,
    pub converted: bool,
}
fn precision(format: cpal::SampleFormat) -> u32 {
    match format {
        cpal::SampleFormat::F64 => 64,
        cpal::SampleFormat::I32 => 32,
        cpal::SampleFormat::F32 => 24,
        cpal::SampleFormat::I16 | cpal::SampleFormat::U16 => 16,
        _ => 0,
    }
}
impl Plan {
    pub fn select(source: Spec, compatibility: bool) -> Result<Self> {
        source.validate()?;
        let device = cpal::default_host()
            .default_output_device()
            .context("No local audio device")?;
        let needed = match source.format {
            SampleFormat::I32 => source.valid_bits.unwrap_or(32),
            SampleFormat::F32 => 24,
            SampleFormat::F64 => source.valid_bits.unwrap_or(64),
        };
        let canonical = ffmpeg_next::ChannelLayout::default(i32::from(source.channels)).bits();
        let layout_ok = source.channel_mask == 0 || source.channel_mask == canonical;
        let mut configs: Vec<_> = device
            .supported_output_configs()?
            .filter(|c| {
                c.channels() == source.channels
                    && c.min_sample_rate().0 <= source.sample_rate
                    && c.max_sample_rate().0 >= source.sample_rate
                    && precision(c.sample_format()) >= needed
                    && (source.format == SampleFormat::I32
                        || matches!(
                            c.sample_format(),
                            cpal::SampleFormat::F32 | cpal::SampleFormat::F64
                        ))
                    && layout_ok
            })
            .collect();
        configs.sort_by_key(|c| std::cmp::Reverse(precision(c.sample_format())));
        if let Some(supported) = configs.first() {
            return Ok(Self {
                config: (*supported)
                    .with_sample_rate(cpal::SampleRate(source.sample_rate))
                    .config(),
                format: supported.sample_format(),
                device,
                converted: false,
            });
        }
        ensure!(compatibility,"Local device cannot preserve {} Hz / {} channels / {:?} {:?}-bit PCM. Enable audio compatibility conversion explicitly.",source.sample_rate,source.channels,source.format,source.valid_bits);
        let supported = device.default_output_config()?;
        ensure!(
            precision(supported.sample_format()) > 0,
            "Unsupported local device sample format"
        );
        Ok(Self {
            config: supported.config(),
            format: supported.sample_format(),
            device,
            converted: true,
        })
    }
    /// The callback only drains the ring; observation must also remain realtime-safe.
    pub fn open(
        self,
        consumer: rtrb::Consumer<crate::pcm::Sample>,
        source: Spec,
        paused: std::sync::Arc<std::sync::atomic::AtomicBool>,
        errors: crossbeam_channel::Sender<String>,
        mut observe: impl FnMut(&[f64], usize) + Send + 'static,
    ) -> Result<cpal::Stream> {
        self.open_with_timing(
            consumer,
            source,
            paused,
            errors,
            move |values, wanted, _| observe(values, wanted),
        )
    }

    /// Also reports the device's estimated delay through the end of this callback.
    pub fn open_with_timing(
        self,
        mut consumer: rtrb::Consumer<crate::pcm::Sample>,
        source: Spec,
        paused: std::sync::Arc<std::sync::atomic::AtomicBool>,
        errors: crossbeam_channel::Sender<String>,
        mut observe: impl FnMut(&[f64], usize, std::time::Duration) + Send + 'static,
    ) -> Result<cpal::Stream> {
        let channels = usize::from(self.config.channels);
        let rate = f64::from(self.config.sample_rate.0);
        let mut scratch = vec![0.0; channels * 8192];
        let mut quantizer = Quantizer::default();
        macro_rules! stream {
            ($ty:ty,$convert:expr) => {{
                self.device.build_output_stream(
                    &self.config,
                    move |out: &mut [$ty], info: &cpal::OutputCallbackInfo| {
                        use std::sync::atomic::Ordering;
                        if paused.load(Ordering::Relaxed) {
                            out.fill(<$ty as cpal::Sample>::EQUILIBRIUM);
                            return;
                        }
                        let timestamp = info.timestamp();
                        let delay = timestamp
                            .playback
                            .duration_since(&timestamp.callback)
                            .unwrap_or_default()
                            + std::time::Duration::from_secs_f64(
                                out.len() as f64 / (channels as f64 * rate),
                            );
                        for block in out.chunks_mut(scratch.len()) {
                            let mut got = 0;
                            for frame in block.chunks_exact_mut(channels) {
                                if consumer.slots() >= channels {
                                    for value in frame {
                                        let sample = consumer.pop().expect("complete PCM frame");
                                        scratch[got] = sample.value;
                                        got += 1;
                                        *value = ($convert)(sample, &mut quantizer);
                                    }
                                } else {
                                    frame.fill(<$ty as cpal::Sample>::EQUILIBRIUM);
                                }
                            }
                            observe(&scratch[..got], block.len(), delay);
                        }
                    },
                    move |e| {
                        let _ = errors.try_send(e.to_string());
                    },
                    None,
                )?
            }};
        }
        let converted = self.converted
            || self.config.sample_rate.0 != source.sample_rate
            || self.config.channels != source.channels;
        let dither32 = source.format != SampleFormat::I32 || converted;
        let dither16 =
            source.format != SampleFormat::I32 || source.valid_bits.unwrap_or(32) > 16 || converted;
        Ok(match self.format {
            cpal::SampleFormat::F64 => {
                stream!(f64, |n: crate::pcm::Sample, _: &mut Quantizer| n.value)
            }
            cpal::SampleFormat::F32 => stream!(f32, |n: crate::pcm::Sample, _: &mut Quantizer| n
                .value
                as f32),
            cpal::SampleFormat::I32 => stream!(
                i32,
                move |n: crate::pcm::Sample, q: &mut Quantizer| q.integer(
                    n.value,
                    32,
                    dither32 || n.processed
                ) as i32
            ),
            cpal::SampleFormat::I16 => stream!(
                i16,
                move |n: crate::pcm::Sample, q: &mut Quantizer| q.integer(
                    n.value,
                    16,
                    dither16 || n.processed
                ) as i16
            ),
            cpal::SampleFormat::U16 => stream!(
                u16,
                move |n: crate::pcm::Sample, q: &mut Quantizer| (q.integer(
                    n.value,
                    16,
                    dither16 || n.processed
                ) + 32768) as u16
            ),
            _ => bail!("Unsupported local device sample format"),
        })
    }
}

/// Optional rate/layout conversion in float64, never in the transparent path.
pub struct Converter {
    context: ffmpeg_next::software::resampling::Context,
    source: Spec,
    channels: u16,
    rate: u32,
}
impl Converter {
    pub fn new(source: Spec, channels: u16, rate: u32) -> Result<Self> {
        use ffmpeg_next as av;
        av::init()?;
        let from = if source.channel_mask == 0 {
            av::ChannelLayout::default(i32::from(source.channels))
        } else {
            channel_layout(source.channel_mask, source.channels)
        };
        Ok(Self {
            context: av::software::resampling::Context::get(
                av::format::Sample::F64(av::format::sample::Type::Packed),
                from,
                source.sample_rate,
                av::format::Sample::F64(av::format::sample::Type::Packed),
                av::ChannelLayout::default(i32::from(channels)),
                rate,
            )?,
            source,
            channels,
            rate,
        })
    }
    pub fn convert(&mut self, samples: &[f64]) -> Result<Vec<f64>> {
        use ffmpeg_next as av;
        ensure!(
            samples.len().is_multiple_of(self.source.channels as usize),
            "Partial PCM frame"
        );
        let layout = if self.source.channel_mask == 0 {
            av::ChannelLayout::default(i32::from(self.source.channels))
        } else {
            channel_layout(self.source.channel_mask, self.source.channels)
        };
        let mut input = av::frame::Audio::new(
            av::format::Sample::F64(av::format::sample::Type::Packed),
            samples.len() / self.source.channels as usize,
            layout,
        );
        input.set_rate(self.source.sample_rate);
        for (bytes, sample) in input
            .data_mut(0)
            .as_chunks_mut::<8>()
            .0
            .iter_mut()
            .zip(samples)
        {
            bytes.copy_from_slice(&sample.to_ne_bytes());
        }
        let capacity = (input.samples() as u64 * u64::from(self.rate))
            .div_ceil(u64::from(self.source.sample_rate)) as usize
            + self.context.delay().map_or(0, |d| d.output.max(0) as usize)
            + 32;
        let mut output = av::frame::Audio::new(
            av::format::Sample::F64(av::format::sample::Type::Packed),
            capacity,
            av::ChannelLayout::default(i32::from(self.channels)),
        );
        self.context.run(&input, &mut output)?;
        Ok(
            output.data(0)[..output.samples() * self.channels as usize * 8]
                .as_chunks::<8>()
                .0
                .iter()
                .map(|b| f64::from_ne_bytes(*b))
                .collect(),
        )
    }

    /// Emit the resampler's delayed tail before reporting end of stream.
    pub fn finish(&mut self) -> Result<Vec<f64>> {
        use ffmpeg_next as av;
        let mut samples = Vec::new();
        loop {
            let mut output = av::frame::Audio::new(
                av::format::Sample::F64(av::format::sample::Type::Packed),
                4096,
                av::ChannelLayout::default(i32::from(self.channels)),
            );
            self.context.flush(&mut output)?;
            if output.samples() == 0 {
                break;
            }
            samples.extend(
                output.data(0)[..output.samples() * self.channels as usize * 8]
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|b| f64::from_ne_bytes(*b)),
            );
        }
        Ok(samples)
    }
}

/// Construct the declared speaker layout, using canonical order when unspecified.
pub fn channel_layout(mask: u64, channels: u16) -> ffmpeg_next::ChannelLayout {
    if mask == 0 {
        return ffmpeg_next::ChannelLayout::default(i32::from(channels));
    }
    #[cfg(not(ffmpeg_channel_layout_v2))]
    {
        ffmpeg_next::ChannelLayout::from_bits_truncate(mask)
    }
    #[cfg(ffmpeg_channel_layout_v2)]
    {
        // A native mask has no owned custom channel array. Use the public
        // conversion rather than a representation that differs before FFmpeg 7.
        let mut value = unsafe { std::mem::zeroed() };
        unsafe { ffmpeg_next::ffi::av_channel_layout_from_mask(&mut value, mask) };
        ffmpeg_next::ChannelLayout::from(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn local_device_preserves_partial_frame_channel_alignment() {
        use cpal::traits::StreamTrait;
        use std::sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            Arc,
        };
        assert!(
            std::env::var_os("STAR_PCM_DEVICE_TEST").is_some(),
            "Use an isolated test audio device"
        );
        let source = Spec {
            sample_rate: 48000,
            channels: 2,
            channel_mask: 3,
            format: SampleFormat::F64,
            valid_bits: Some(64),
        };
        let plan = Plan::select(source, false).unwrap();
        let (mut producer, consumer) = rtrb::RingBuffer::new(64);
        producer.push(0.25.into()).unwrap();
        let observed = Arc::new([AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)]);
        let callback = observed.clone();
        let (errors, _) = crossbeam_channel::bounded(1);
        let stream = plan
            .open(
                consumer,
                source,
                Arc::new(AtomicBool::new(false)),
                errors,
                move |values, _| {
                    if values.len() >= 2 {
                        callback[0].store(values[0].to_bits(), Ordering::Relaxed);
                        callback[1].store(values[1].to_bits(), Ordering::Relaxed);
                        callback[2].fetch_add(values.len() as u64, Ordering::Release);
                    }
                },
            )
            .unwrap();
        stream.play().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(producer.slots(), 63, "A partial frame was consumed");
        producer.push(0.5.into()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while observed[2].load(Ordering::Acquire) == 0 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(observed[2].load(Ordering::Acquire), 2);
        assert_eq!(observed[0].load(Ordering::Relaxed), 0.25f64.to_bits());
        assert_eq!(observed[1].load(Ordering::Relaxed), 0.5f64.to_bits());
    }

    #[test]
    fn compatibility_uses_speaker_layout_even_when_channel_counts_match() {
        // Two channels containing centre and LFE are not a stereo L/R pair.
        let source = Spec {
            sample_rate: 48000,
            channels: 2,
            channel_mask: 12,
            format: SampleFormat::F64,
            valid_bits: Some(64),
        };
        let mut converter = Converter::new(source, 2, 48000).unwrap();
        let mut converted = converter.convert(&[0.5, 0.0].repeat(256)).unwrap();
        converted.extend(converter.finish().unwrap());
        assert_eq!(converted.len(), 512);
        for frame in converted.as_chunks::<2>().0 {
            assert!(frame[0] > 0.0);
            assert!(
                (frame[0] - frame[1]).abs() < 1e-12,
                "Centre speaker must reach both stereo channels"
            );
        }
    }

    #[test]
    fn compatibility_resampling_drains_the_final_frames() {
        for (from, to) in [(44100, 48000), (192000, 44100), (48000, 96000)] {
            let spec = Spec {
                sample_rate: from,
                channels: 2,
                channel_mask: 3,
                format: SampleFormat::F64,
                valid_bits: Some(64),
            };
            let mut converter = Converter::new(spec, 2, to).unwrap();
            let mut converted = Vec::new();
            for _ in 0..50 {
                converted.extend(
                    converter
                        .convert(&vec![0.25; from as usize / 50 * 2])
                        .unwrap(),
                );
            }
            let before = converted.len();
            converted.extend(converter.finish().unwrap());
            assert!(converted.len() > before, "Resampler tail was not exercised");
            assert_eq!(converted.len(), to as usize * 2);
            assert!(converted.iter().all(|v| (*v - 0.25).abs() < 0.001));
        }
    }
}
