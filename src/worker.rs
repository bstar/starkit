//! Cancellable application jobs. Progress coalesces; completion is never dropped.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
    Arc, Mutex,
};
use std::thread::JoinHandle;

#[derive(Clone, Debug)]
pub struct Progress {
    pub fraction: f64,
    pub message: String,
}
#[derive(Clone)]
pub struct Context {
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<Option<Progress>>>,
}
impl Context {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
    pub fn progress(&self, fraction: f64, message: impl Into<String>) {
        let mut message = message.into();
        message.truncate(
            message
                .char_indices()
                .nth(4096)
                .map_or(message.len(), |(i, _)| i),
        );
        *self.progress.lock().unwrap() = Some(Progress {
            fraction: if fraction.is_finite() {
                fraction.clamp(0., 1.)
            } else {
                0.
            },
            message,
        });
    }
}
pub struct Worker<T> {
    context: Context,
    result: Receiver<Result<T, String>>,
    join: Option<JoinHandle<()>>,
}
impl<T: Send + 'static> Worker<T> {
    pub fn spawn(
        name: &str,
        job: impl FnOnce(Context) -> Result<T, String> + Send + 'static,
    ) -> std::io::Result<Self> {
        let context = Context {
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(Mutex::new(None)),
        };
        let (tx, result) = mpsc::sync_channel(1);
        let ctx = context.clone();
        let join = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(ctx)))
                    .unwrap_or_else(|_| Err("Background worker panicked".into()));
                let _ = tx.send(outcome);
            })?;
        Ok(Self {
            context,
            result,
            join: Some(join),
        })
    }
    pub fn progress(&self) -> Option<Progress> {
        self.context.progress.lock().unwrap().take()
    }
    pub fn result(&mut self) -> Option<Result<T, String>> {
        match self.result.try_recv() {
            Ok(value) => {
                if let Some(j) = self.join.take() {
                    let _ = j.join();
                }
                Some(value)
            }
            Err(mpsc::TryRecvError::Disconnected) => self.join.take().map(|j| {
                let _ = j.join();
                Err("Background worker disconnected".into())
            }),
            Err(_) => None,
        }
    }
    pub fn cancel(&self) {
        self.context.cancel.store(true, Ordering::Release);
    }
}
impl<T> Drop for Worker<T> {
    fn drop(&mut self) {
        self.context.cancel.store(true, Ordering::Release);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_cannot_hide_completion() {
        let mut worker = Worker::spawn("test", |ctx| {
            for i in 0..10000 {
                ctx.progress(i as f64 / 10000., "Working");
            }
            Ok(42)
        })
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(result) = worker.result() {
                assert_eq!(result.unwrap(), 42);
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
}
