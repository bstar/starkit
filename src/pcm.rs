//! Precision-preserving PCM metadata and conversion. No audio device is needed.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

pub const MAX_BLOCK_BYTES: usize = 256 * 1024;
pub const WINDOW_BLOCKS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleFormat {
    I32,
    F32,
    F64,
}
impl SampleFormat {
    pub fn bytes(self) -> usize {
        if self == Self::F64 {
            8
        } else {
            4
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spec {
    pub sample_rate: u32,
    pub channels: u16,
    /// Standard speaker-position bitmask; zero means unspecified order.
    pub channel_mask: u64,
    pub format: SampleFormat,
    pub valid_bits: Option<u32>,
}
impl Spec {
    pub fn validate(self) -> Result<()> {
        ensure!(
            (1..=12_288_000).contains(&self.sample_rate),
            "Unsupported PCM sample rate"
        );
        ensure!(
            (1..=32).contains(&self.channels),
            "Invalid PCM channel count"
        );
        ensure!(
            self.channel_mask == 0 || self.channel_mask.count_ones() == u32::from(self.channels),
            "PCM channel layout disagrees with count"
        );
        ensure!(
            self.valid_bits.is_none_or(|b| b > 0
                && b <= if self.format == SampleFormat::F64 {
                    64
                } else {
                    32
                }),
            "Invalid PCM precision"
        );
        Ok(())
    }
    pub fn block_frames(self) -> usize {
        (self.sample_rate as usize / 50)
            .max(1)
            .min(MAX_BLOCK_BYTES / (self.channels as usize * self.format.bytes()).max(1))
    }
    pub fn validate_bytes(self, len: usize) -> Result<()> {
        self.validate()?;
        ensure!(
            len > 0
                && len <= MAX_BLOCK_BYTES
                && len.is_multiple_of(self.channels as usize * self.format.bytes()),
            "Invalid PCM block size"
        );
        ensure!(
            len <= self.block_frames() * self.channels as usize * self.format.bytes(),
            "PCM block exceeds negotiated duration"
        );
        Ok(())
    }
    pub fn encode(self, samples: &[f64]) -> Result<Vec<u8>> {
        let len = samples
            .len()
            .checked_mul(self.format.bytes())
            .ok_or_else(|| anyhow::anyhow!("PCM length overflow"))?;
        self.validate_bytes(len)?;
        let mut bytes = Vec::with_capacity(len);
        for &sample in samples {
            match self.format {
                SampleFormat::F64 => bytes.extend_from_slice(&sample.to_le_bytes()),
                SampleFormat::F32 => bytes.extend_from_slice(&(sample as f32).to_le_bytes()),
                SampleFormat::I32 => bytes.extend_from_slice(
                    &((sample * 2147483648.0)
                        .round()
                        .clamp(i32::MIN as f64, i32::MAX as f64) as i32)
                        .to_le_bytes(),
                ),
            }
        }
        Ok(bytes)
    }
    pub fn decode(self, bytes: &[u8]) -> Result<Vec<f64>> {
        self.validate_bytes(bytes.len())?;
        Ok(bytes
            .chunks_exact(self.format.bytes())
            .map(|b| match self.format {
                SampleFormat::F64 => f64::from_le_bytes(b.try_into().unwrap()),
                SampleFormat::F32 => f32::from_le_bytes(b.try_into().unwrap()) as f64,
                SampleFormat::I32 => {
                    i32::from_le_bytes(b.try_into().unwrap()) as f64 / 2147483648.0
                }
            })
            .collect())
    }
}

/// A playback sample with its processing provenance, retained through buffers.
/// This flag prevents a UI setting change from reinterpreting already queued PCM.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    pub value: f64,
    pub processed: bool,
}
impl From<f64> for Sample {
    fn from(value: f64) -> Self {
        Self {
            value,
            processed: false,
        }
    }
}

/// TPDF at one destination LSB. Use only when samples lose integer precision.
pub struct Quantizer {
    state: u64,
}
impl Default for Quantizer {
    fn default() -> Self {
        Self {
            state: 0x6a09e667f3bcc909,
        }
    }
}
impl Quantizer {
    fn uniform(&mut self) -> f64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        (self.state >> 11) as f64 / 9007199254740992.0
    }
    pub fn integer(&mut self, value: f64, bits: u32, dither: bool) -> i64 {
        let scale = (1u64 << (bits - 1)) as f64;
        let noise = if dither {
            self.uniform() - self.uniform()
        } else {
            0.0
        };
        if !value.is_finite() {
            return 0;
        }
        (value
            .mul_add(scale, noise)
            .round()
            .clamp(-scale, scale - 1.0)) as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spec(format: SampleFormat) -> Spec {
        Spec {
            sample_rate: 192000,
            channels: 1,
            channel_mask: 4,
            format,
            valid_bits: Some(32),
        }
    }
    #[test]
    fn integer_low_bits_survive() {
        let source = [
            i32::MIN,
            i32::MIN + 1,
            -16777217,
            -1,
            0,
            1,
            16777217,
            i32::MAX,
        ];
        let samples: Vec<_> = source.iter().map(|&n| n as f64 / 2147483648.0).collect();
        let bytes = spec(SampleFormat::I32).encode(&samples).unwrap();
        assert_eq!(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| i32::from_le_bytes(*b))
                .collect::<Vec<_>>(),
            source
        );
        assert_eq!(spec(SampleFormat::I32).decode(&bytes).unwrap(), samples);
    }
    #[test]
    fn doubles_survive_bit_for_bit() {
        let samples = [
            -0.0,
            0.0,
            f64::MIN_POSITIVE,
            f64::from_bits(1),
            0.5000000000000001,
            -1.0,
        ];
        let s = spec(SampleFormat::F64);
        let got = s.decode(&s.encode(&samples).unwrap()).unwrap();
        assert_eq!(
            got.iter().map(|n| n.to_bits()).collect::<Vec<_>>(),
            samples.map(f64::to_bits)
        );
    }
    #[test]
    fn rejects_bad_frames_and_layout() {
        assert!(spec(SampleFormat::I32).decode(&[0; 3]).is_err());
        assert!(spec(SampleFormat::F64)
            .decode(&vec![0; MAX_BLOCK_BYTES + 8])
            .is_err());
        assert!(Spec {
            channels: 2,
            ..spec(SampleFormat::F64)
        }
        .validate()
        .is_err());
    }
    #[test]
    fn quantization_is_exact_without_dither_and_clips() {
        let mut q = Quantizer::default();
        for value in [i32::MIN, -1, 0, 1, i32::MAX] {
            assert_eq!(
                q.integer(value as f64 / 2147483648.0, 32, false),
                value as i64
            );
        }
        assert_eq!(q.integer(2.0, 16, false), 32767);
        assert_eq!(q.integer(-2.0, 16, false), -32768);
        assert_eq!(q.integer(f64::NAN, 16, true), 0);
    }
    #[test]
    fn tpdf_is_bounded_and_unbiased() {
        let mut q = Quantizer::default();
        let samples: Vec<_> = (0..100000).map(|_| q.integer(0.0, 16, true)).collect();
        assert!(samples.iter().all(|n| n.abs() <= 1));
        assert!((samples.iter().sum::<i64>() as f64 / samples.len() as f64).abs() < 0.02);
        assert!(samples.contains(&1) && samples.contains(&-1));
    }

    proptest::proptest! {
        #[test]
        fn any_integer_round_trips_exactly(value: i32) {
            let sample = f64::from(value) / 2147483648.0;
            let spec = spec(SampleFormat::I32);
            proptest::prop_assert_eq!(spec.decode(&spec.encode(&[sample]).unwrap()).unwrap(), vec![sample]);
        }

        #[test]
        fn foreign_metadata_and_lengths_never_panic(rate:u32, channels:u16, mask:u64, bits:u32, bytes in proptest::collection::vec(proptest::num::u8::ANY, 0..2048)) {
            let spec=Spec{sample_rate:rate,channels,channel_mask:mask,format:SampleFormat::F64,valid_bits:Some(bits)};
            let _ = spec.decode(&bytes);
        }
    }
}
