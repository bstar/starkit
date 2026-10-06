//! Bounded native playback benchmark; --audio drains the real device at zero volume.
#[cfg(feature = "media")]
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    use starkit::media::{
        playback::Player,
        tracks::{PlaybackOptions, Selection},
    };
    use std::{
        sync::atomic::Ordering,
        time::{Duration, Instant},
    };
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("Usage: movie-bench FILE"))?;
    let seconds = std::env::args()
        .find_map(|s| s.strip_prefix("--seconds=").map(str::to_owned))
        .map_or(Ok(6.0), |s| s.parse::<f64>())?;
    let start = std::env::args()
        .find_map(|s| s.strip_prefix("--start=").map(str::to_owned))
        .map_or(Ok(0.0), |s| s.parse::<f64>())?;
    anyhow::ensure!(
        seconds.is_finite()
            && seconds > 0.0
            && seconds <= 120.0
            && start.is_finite()
            && start >= 0.0,
        "Invalid benchmark duration/start"
    );
    let player = Player::file_with_options(
        path.into(),
        start,
        PlaybackOptions {
            audio: if std::env::args().any(|s| s == "--audio") {
                Selection::Auto
            } else {
                Selection::Off
            },
            subtitle: Selection::Off,
        },
    )?;
    let started = Instant::now();
    let mut frames = 0;
    let mut dimensions = (0, 0);
    let mut first = 0.0;
    let mut last = 0.0;
    while started.elapsed() < Duration::from_secs_f64(seconds) {
        match player.frames.recv_timeout(Duration::from_millis(200)) {
            Ok(frame) => {
                if frames == 0 {
                    first = frame.seconds;
                }
                last = frame.seconds;
                dimensions = frame.pixels.dimensions();
                frames += 1;
            }
            Err(_) if player.controls.finished.load(Ordering::Relaxed) => break,
            Err(_) => {}
        }
    }
    println!(
        "{}x{}: {frames} frames in {:.3}s, media {:.3}s, dropped {}",
        dimensions.0,
        dimensions.1,
        started.elapsed().as_secs_f64(),
        last - first,
        player.controls.dropped.load(Ordering::Relaxed)
    );
    println!(
        "Audio frames {}, underruns {}",
        player.controls.audio_samples.load(Ordering::Relaxed),
        player.controls.underruns.load(Ordering::Relaxed)
    );
    for notice in player.notices.try_iter() {
        println!("{notice}");
    }
    anyhow::ensure!(frames > 0, "No video frames decoded");
    if std::env::args().any(|s| s == "--audio") {
        anyhow::ensure!(
            player.controls.audio_samples.load(Ordering::Relaxed) > 0,
            "No audio frames played"
        );
        anyhow::ensure!(
            player.controls.underruns.load(Ordering::Relaxed) == 0,
            "Audio underrun during playback"
        );
    }
    Ok(())
}
#[cfg(not(feature = "media"))]
fn main() {
    eprintln!("Build with --features media");
}
