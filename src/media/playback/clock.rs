//! Lock-free device clock. Interpolate between callbacks instead of advancing
//! video in audio-period-sized jumps. No allocation or lock on the callback.
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

pub(super) struct AudioClock {
    epoch: Instant,
    sequence: AtomicU64,
    playback_ns: AtomicU64,
    first_sample: AtomicU64,
    end_sample: AtomicU64,
}
impl Default for AudioClock {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            sequence: AtomicU64::new(0),
            playback_ns: AtomicU64::new(0),
            first_sample: AtomicU64::new(0),
            end_sample: AtomicU64::new(0),
        }
    }
}
impl AudioClock {
    pub fn elapsed_ns(&self) -> u64 {
        self.epoch.elapsed().as_nanos() as u64
    }
    // Single writer: the device callback. The sequence keeps the timestamp
    // and sample range consistent even if a reader races the next callback.
    pub fn publish(&self, playback_ns: u64, first_sample: u64, end_sample: u64) {
        self.sequence.fetch_add(1, Ordering::SeqCst);
        self.playback_ns.store(playback_ns, Ordering::SeqCst);
        self.first_sample.store(first_sample, Ordering::SeqCst);
        self.end_sample.store(end_sample, Ordering::SeqCst);
        self.sequence.fetch_add(1, Ordering::SeqCst);
    }
    pub fn seconds(&self, rate: u32) -> Option<f64> {
        self.seconds_at(self.elapsed_ns(), rate)
    }
    fn seconds_at(&self, now_ns: u64, rate: u32) -> Option<f64> {
        if rate == 0 {
            return None;
        }
        for _ in 0..3 {
            let before = self.sequence.load(Ordering::SeqCst);
            if before == 0 || before & 1 != 0 {
                continue;
            }
            let time = self.playback_ns.load(Ordering::SeqCst);
            let first = self.first_sample.load(Ordering::SeqCst);
            let end = self.end_sample.load(Ordering::SeqCst);
            if before == self.sequence.load(Ordering::SeqCst) {
                // A future device timestamp accounts for queued output latency.
                // Clamp only the end: underruns must not extrapolate into audio
                // that was never delivered to the device.
                let elapsed = (i128::from(now_ns) - i128::from(time)) as f64 / 1_000_000_000.0;
                return Some(
                    (first as f64 / f64::from(rate) + elapsed).min(end as f64 / f64::from(rate)),
                );
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audio_time_advances_between_callbacks_and_accounts_for_device_latency() {
        let c = AudioClock::default();
        assert_eq!(c.seconds_at(0, 48_000), None);
        c.publish(10_000_000, 0, 960);
        assert!((c.seconds_at(5_000_000, 48_000).unwrap() + 0.005).abs() < 1e-9);
        assert!((c.seconds_at(15_000_000, 48_000).unwrap() - 0.005).abs() < 1e-9);
        assert!((c.seconds_at(25_000_000, 48_000).unwrap() - 0.015).abs() < 1e-9);
        // Publishing the next 20 ms callback does not make a 20 ms jump.
        let before = c.seconds_at(25_000_000, 48_000).unwrap();
        c.publish(30_000_000, 960, 1920);
        assert!((c.seconds_at(25_000_000, 48_000).unwrap() - before).abs() < 1e-9);
    }
    #[test]
    fn clock_stops_at_delivered_audio_and_reanchors_after_pause_or_starvation() {
        let c = AudioClock::default();
        c.publish(0, 0, 960);
        assert_eq!(c.seconds_at(1_000_000_000, 48_000), Some(0.02));
        c.publish(1_000_000_000, 960, 1920);
        assert_eq!(c.seconds_at(1_000_000_000, 48_000), Some(0.02));
        assert!((c.seconds_at(1_010_000_000, 48_000).unwrap() - 0.03).abs() < 1e-9);
    }
}
