//! An anonymous disk-backed PCM spool. Memory holds only block offsets;
//! playback can drain a complete track while the terminal is not updating.
use crate::pcm::SampleFormat;
use anyhow::{ensure, Result};
use std::sync::Arc;

const TRACK_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const PROCESS_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 262_144;

struct Budget {
    used: AtomicU64,
    limit: u64,
}
impl Budget {
    fn reserve(&self, bytes: u64) -> Result<()> {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|sum| *sum <= self.limit)
            })
            .map_err(|_| anyhow::anyhow!("PCM cache process disk budget exceeded"))?;
        Ok(())
    }
}
// A single shared process budget; test budgets are independent.
fn process_budget() -> Arc<Budget> {
    static BUDGET: std::sync::OnceLock<Arc<Budget>> = std::sync::OnceLock::new();
    BUDGET
        .get_or_init(|| {
            Arc::new(Budget {
                used: AtomicU64::new(0),
                limit: PROCESS_BYTES,
            })
        })
        .clone()
}
use std::{
    collections::VecDeque,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

pub(super) type Block = (SampleFormat, bool, Vec<u8>);
struct Entry {
    offset: u64,
    size: usize,
    format: SampleFormat,
    processed: bool,
}
struct Contents {
    file: Option<File>,
    entries: VecDeque<Entry>,
    end: u64,
    finished: bool,
}
pub(super) struct Cache {
    contents: Mutex<Contents>,
    pub frames: AtomicU64,
    budget: Arc<Budget>,
    max_bytes: u64,
    max_entries: usize,
}
pub(super) enum Next {
    Block(Block),
    Waiting,
    End,
}
impl Cache {
    pub fn new() -> Result<Self> {
        Self::with_limits(process_budget(), TRACK_BYTES, MAX_ENTRIES)
    }
    fn with_limits(budget: Arc<Budget>, max_bytes: u64, max_entries: usize) -> Result<Self> {
        Ok(Self {
            budget,
            max_bytes,
            max_entries,
            contents: Mutex::new(Contents {
                file: Some(tempfile::tempfile()?),
                entries: VecDeque::new(),
                end: 0,
                finished: false,
            }),
            frames: AtomicU64::new(0),
        })
    }
    pub fn append(&self, block: Block, channels: u16) -> Result<()> {
        let (format, processed, bytes) = block;
        ensure!(
            (1..=32).contains(&channels)
                && !bytes.is_empty()
                && bytes.len() <= crate::pcm::MAX_BLOCK_BYTES
                && bytes
                    .len()
                    .is_multiple_of(format.bytes() * usize::from(channels)),
            "Invalid PCM cache block"
        );
        let frames = bytes.len() as u64 / (format.bytes() as u64 * u64::from(channels));
        let mut c = self.contents.lock().unwrap();
        ensure!(!c.finished, "PCM cache is already finished");
        ensure!(
            c.entries.len() < self.max_entries,
            "PCM cache metadata budget exceeded"
        );
        let end = c
            .end
            .checked_add(bytes.len() as u64)
            .filter(|end| *end <= self.max_bytes)
            .ok_or_else(|| anyhow::anyhow!("PCM cache track disk budget exceeded"))?;
        self.budget.reserve(bytes.len() as u64)?;
        let offset = c.end;
        let result = c
            .file
            .as_mut()
            .unwrap()
            .seek(SeekFrom::Start(offset))
            .and_then(|_| c.file.as_mut().unwrap().write_all(&bytes));
        if let Err(error) = result {
            // A partial write still consumes space. Retain its reservation if
            // truncation fails; close/unlink on drop releases it reliably.
            if c.file.as_mut().unwrap().set_len(offset).is_err() {
                c.end = end;
                c.finished = true;
            } else {
                self.budget
                    .used
                    .fetch_sub(bytes.len() as u64, Ordering::AcqRel);
            }
            return Err(error.into());
        }
        c.end = end;
        c.entries.push_back(Entry {
            offset,
            size: bytes.len(),
            format,
            processed,
        });
        self.frames.fetch_add(frames, Ordering::Release);
        Ok(())
    }
    pub fn finish(&self) {
        self.contents.lock().unwrap().finished = true;
    }
    pub fn next(&self) -> Result<Next> {
        let mut c = self.contents.lock().unwrap();
        let Some(e) = c.entries.pop_front() else {
            return Ok(if c.finished { Next::End } else { Next::Waiting });
        };
        c.file.as_mut().unwrap().seek(SeekFrom::Start(e.offset))?;
        let mut bytes = vec![0; e.size];
        c.file.as_mut().unwrap().read_exact(&mut bytes)?;
        Ok(Next::Block((e.format, e.processed, bytes)))
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        let c = self
            .contents
            .get_mut()
            .unwrap_or_else(|poison| poison.into_inner());
        // Close/unlink before another stream can use the released disk reservation.
        drop(c.file.take());
        self.budget.used.fetch_sub(c.end, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn budgets_reject_before_writing_and_release_on_drop() {
        let budget = Arc::new(Budget {
            used: AtomicU64::new(0),
            limit: 16,
        });
        let cache = Cache::with_limits(budget.clone(), 12, 2).unwrap();
        let block = || (SampleFormat::I32, false, vec![0; 8]);
        cache.append(block(), 2).unwrap();
        assert!(cache
            .append(block(), 2)
            .unwrap_err()
            .to_string()
            .contains("track"));
        assert_eq!(budget.used.load(Ordering::Acquire), 8);
        let other = Cache::with_limits(budget.clone(), 32, 2).unwrap();
        other.append(block(), 2).unwrap();
        assert!(other
            .append(block(), 2)
            .unwrap_err()
            .to_string()
            .contains("process"));
        assert_eq!(cache.frames.load(Ordering::Acquire), 1);
        drop(cache);
        other.append(block(), 2).unwrap();
        assert!(other
            .append(block(), 2)
            .unwrap_err()
            .to_string()
            .contains("metadata"));
        drop(other);
        assert_eq!(budget.used.load(Ordering::Acquire), 0);
    }
    #[test]
    fn invalid_blocks_and_finished_streams_do_not_consume_budget() {
        let cache = Cache::new().unwrap();
        for channels in [0, 33] {
            assert!(cache
                .append((SampleFormat::I32, false, vec![0; 8]), channels)
                .is_err());
        }
        for size in [0, 3, crate::pcm::MAX_BLOCK_BYTES + 8] {
            assert!(cache
                .append((SampleFormat::I32, false, vec![0; size]), 2)
                .is_err());
        }
        cache.finish();
        assert!(cache
            .append((SampleFormat::I32, false, vec![0; 8]), 2)
            .is_err());
        assert_eq!(cache.frames.load(Ordering::Acquire), 0);
    }
    #[test]
    fn complete_track_is_cached_without_a_playback_consumer() {
        let cache = Cache::new().unwrap();
        for i in 0..300u32 {
            cache
                .append((SampleFormat::I32, false, i.to_le_bytes().repeat(1920)), 2)
                .unwrap();
        }
        cache.finish();
        assert_eq!(cache.frames.load(Ordering::Acquire), 300 * 960);
        for i in 0..300u32 {
            let Next::Block((format, processed, data)) = cache.next().unwrap() else {
                panic!("lost cached audio")
            };
            assert_eq!(format, SampleFormat::I32);
            assert!(!processed);
            assert_eq!(data, i.to_le_bytes().repeat(1920));
        }
        assert!(matches!(cache.next().unwrap(), Next::End));
    }
}
