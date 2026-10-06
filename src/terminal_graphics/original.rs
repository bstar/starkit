//! Bounded, seekable original media reads. Only the host owns a filesystem path.
use super::{media::ToHost, protocol::ClientMessage};
use crate::media::playback::Controls;
use crossbeam_channel::Sender;
use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    io::{self, Read, Seek, SeekFrom},
    sync::{atomic::Ordering, Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

pub const BLOCK: usize = 256 * 1024;
const CACHE_BLOCKS: usize = 256;
const LOOKAHEAD: u64 = 32;
const IN_FLIGHT: usize = 8;

#[derive(Default)]
struct State {
    blocks: BTreeMap<u64, Arc<Vec<u8>>>,
    order: VecDeque<u64>,
    pending: HashSet<u64>,
    focus: u64,
}
pub struct Source {
    pub size: u64,
    session: u64,
    generation: u64,
    out: Sender<ClientMessage>,
    state: Mutex<State>,
    ready: Condvar,
}
impl Source {
    pub fn new(size: u64, session: u64, generation: u64, out: Sender<ClientMessage>) -> Arc<Self> {
        Arc::new(Self {
            size,
            session,
            generation,
            out,
            state: Mutex::new(State::default()),
            ready: Condvar::new(),
        })
    }
    fn request(&self, state: &mut State, offset: u64) {
        if offset >= self.size
            || state.blocks.contains_key(&offset)
            || state.pending.contains(&offset)
            || state.pending.len() >= IN_FLIGHT
        {
            return;
        }
        if self
            .out
            .try_send(ClientMessage::Media {
                message: ToHost::Read {
                    session: self.session,
                    generation: self.generation,
                    offset,
                },
            })
            .is_ok()
        {
            state.pending.insert(offset);
        }
    }
    fn fill(&self, state: &mut State) {
        for index in 0..LOOKAHEAD {
            self.request(state, state.focus.saturating_add(index * BLOCK as u64));
        }
    }
    pub fn accept(&self, offset: u64, bytes: Vec<u8>) -> anyhow::Result<()> {
        anyhow::ensure!(
            offset < self.size
                && offset.is_multiple_of(BLOCK as u64)
                && bytes.len() == (self.size - offset).min(BLOCK as u64) as usize,
            "Invalid original media range"
        );
        let mut state = self.state.lock().unwrap();
        anyhow::ensure!(
            state.pending.remove(&offset),
            "Unrequested original media range"
        );
        state.blocks.insert(offset, Arc::new(bytes));
        state.order.push_back(offset);
        while state.blocks.len() > CACHE_BLOCKS {
            if let Some(old) = state.order.pop_front() {
                state.blocks.remove(&old);
            }
        }
        self.fill(&mut state);
        self.ready.notify_all();
        Ok(())
    }
    pub fn buffered_bytes(&self) -> u64 {
        let state = self.state.lock().unwrap();
        let mut offset = state.focus;
        let mut bytes = 0;
        while let Some(block) = state.blocks.get(&offset) {
            bytes += block.len() as u64;
            offset += BLOCK as u64;
        }
        bytes
    }
    pub fn reader(self: &Arc<Self>, controls: Arc<Controls>) -> Reader {
        Reader {
            source: self.clone(),
            controls,
            position: 0,
            primed: false,
        }
    }
}
pub struct Reader {
    source: Arc<Source>,
    controls: Arc<Controls>,
    position: u64,
    primed: bool,
}
impl Read for Reader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() || self.position >= self.source.size {
            return Ok(0);
        }
        let offset = self.position / BLOCK as u64 * BLOCK as u64;
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut state = self.source.state.lock().unwrap();
        loop {
            if self.controls.cancelled.load(Ordering::Relaxed) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            state.focus = if self.primed { offset } else { 0 };
            self.source.request(&mut state, offset);
            self.source.fill(&mut state);
            if !self.primed {
                let mut available = 0u64;
                let mut next = 0;
                while let Some(block) = state.blocks.get(&next) {
                    available += block.len() as u64;
                    next += BLOCK as u64;
                }
                if available >= self.source.size.min(4 * 1024 * 1024) {
                    self.primed = true;
                }
            }
            if let Some(block) = state.blocks.get(&offset).cloned().filter(|_| self.primed) {
                let start = (self.position - offset) as usize;
                let n = out.len().min(block.len() - start);
                out[..n].copy_from_slice(&block[start..start + n]);
                self.position += n as u64;
                return Ok(n);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Original media read timed out",
                ));
            }
            state = self
                .source
                .ready
                .wait_timeout(state, Duration::from_millis(20))
                .unwrap()
                .0;
        }
    }
}
impl Seek for Reader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::Current(n) => i128::from(self.position) + i128::from(n),
            SeekFrom::End(n) => i128::from(self.source.size) + i128::from(n),
        };
        if position < 0 || position > i128::from(self.source.size) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        self.position = position as u64;
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::bounded;
    #[test]
    fn independent_readers_seek_and_receive_exact_original_bytes() {
        let bytes: Vec<u8> = (0..BLOCK * 3 + 19).map(|i| (i * 17 + 9) as u8).collect();
        let (out, requests) = bounded(32);
        let source = Source::new(bytes.len() as u64, 7, 2, out);
        let serve = source.clone();
        let content = bytes.clone();
        let controls = Controls::new();
        let stop = controls.clone();
        let worker = std::thread::spawn(move || {
            while !stop.cancelled.load(Ordering::Relaxed) {
                if let Ok(ClientMessage::Media {
                    message:
                        ToHost::Read {
                            offset,
                            session: 7,
                            generation: 2,
                        },
                }) = requests.recv_timeout(Duration::from_millis(20))
                {
                    let start = offset as usize;
                    serve
                        .accept(
                            offset,
                            content[start..(start + BLOCK).min(content.len())].to_vec(),
                        )
                        .unwrap();
                }
            }
        });
        let mut video = source.reader(controls.clone());
        let mut audio = source.reader(controls.clone());
        video.seek(SeekFrom::End(-17)).unwrap();
        let mut end = [0; 17];
        video.read_exact(&mut end).unwrap();
        assert_eq!(end, bytes[bytes.len() - 17..]);
        audio.seek(SeekFrom::Start(BLOCK as u64 - 3)).unwrap();
        let mut span = [0; 11];
        audio.read_exact(&mut span).unwrap();
        assert_eq!(span, bytes[BLOCK - 3..BLOCK + 8]);
        assert_eq!(video.read(&mut span).unwrap(), 0);
        assert!(audio.seek(SeekFrom::End(1)).is_err());
        controls.cancelled.store(true, Ordering::Relaxed);
        worker.join().unwrap();
    }
    #[test]
    fn cancellation_interrupts_a_missing_range_without_waiting_for_network() {
        let (out, _requests) = bounded(32);
        let source = Source::new(BLOCK as u64, 1, 1, out);
        let controls = Controls::new();
        let stop = controls.clone();
        let worker = std::thread::spawn(move || source.reader(controls).read(&mut [0; 8]));
        stop.cancelled.store(true, Ordering::Relaxed);
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
    }
    proptest::proptest! {
        #[test]
        fn foreign_ranges_are_bounded(offset in proptest::num::u64::ANY, length in 0usize..BLOCK*2) {
            let (out,_)=bounded(1);
            let source=Source::new(BLOCK as u64*2+1,1,1,out);
            source.state.lock().unwrap().pending.insert(offset);
            let result=source.accept(offset,vec![0;length]);
            proptest::prop_assert_eq!(result.is_ok(),offset < source.size && offset.is_multiple_of(BLOCK as u64) && length==(source.size-offset).min(BLOCK as u64) as usize);
        }
    }
}
