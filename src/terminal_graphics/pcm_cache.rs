//! An anonymous disk-backed PCM spool. Memory holds only block offsets;
//! playback can drain a complete track while the terminal is not updating.
use crate::pcm::SampleFormat;
use anyhow::Result;
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
    file: File,
    entries: VecDeque<Entry>,
    end: u64,
    finished: bool,
}
pub(super) struct Cache {
    contents: Mutex<Contents>,
    pub frames: AtomicU64,
}
pub(super) enum Next {
    Block(Block),
    Waiting,
    End,
}
impl Cache {
    pub fn new() -> Result<Self> {
        Ok(Self {
            contents: Mutex::new(Contents {
                file: tempfile::tempfile()?,
                entries: VecDeque::new(),
                end: 0,
                finished: false,
            }),
            frames: AtomicU64::new(0),
        })
    }
    pub fn append(&self, block: Block, channels: u16) -> Result<()> {
        let (format, processed, bytes) = block;
        let frames = bytes.len() as u64 / (format.bytes() as u64 * u64::from(channels));
        let mut c = self.contents.lock().unwrap();
        let offset = c.end;
        c.file.seek(SeekFrom::Start(offset))?;
        c.file.write_all(&bytes)?;
        c.end += bytes.len() as u64;
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
        c.file.seek(SeekFrom::Start(e.offset))?;
        let mut bytes = vec![0; e.size];
        c.file.read_exact(&mut bytes)?;
        Ok(Next::Block((e.format, e.processed, bytes)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
