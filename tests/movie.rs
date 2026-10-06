#![cfg(feature = "media")]
use starkit::media::{
    playback::Player,
    proxy,
    tracks::{self, PlaybackOptions, Selection},
    Quality,
};
use std::{
    process::Command,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

fn fixture(root: &std::path::Path) -> std::path::PathBuf {
    let subtitles = root.join("movie.en.srt");
    std::fs::write(
        &subtitles,
        "1\n00:00:00,000 --> 00:00:01,500\nNATIVE SUBTITLE TEST\n",
    )
    .unwrap();
    let path = root.join("movie.mkv");
    let status = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:size=1920x1080:rate=60",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=400:sample_rate=44100",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=800:sample_rate=48000",
        ])
        .arg("-i")
        .arg(&subtitles)
        .args([
            "-t",
            "2",
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-map",
            "2:a",
            "-map",
            "3:s",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-c:a",
            "pcm_s24le",
            "-c:s",
            "srt",
            "-metadata:s:a:0",
            "language=eng",
            "-metadata:s:a:1",
            "language=fra",
            "-metadata:s:s:0",
            "language=eng",
            "-y",
        ])
        .arg(&path)
        .status()
        .expect("FFmpeg fixture generator is required in the Nix shell");
    assert!(status.success());
    path
}
#[test]
fn native_frames_tracks_subtitles_and_paused_seek() {
    let root = tempfile::tempdir().unwrap();
    let path = fixture(root.path());
    let catalog = tracks::discover(&path, Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!(catalog.audio.len(), 2);
    assert_eq!(catalog.subtitles.len(), 2);
    assert!(catalog.audio[0].label.contains("44100"));
    let options = PlaybackOptions {
        audio: Selection::Off,
        subtitle: Selection::Off,
    };
    let player = Player::file_with_options(path.clone(), 0.0, options.clone()).unwrap();
    let first = player.frames.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(first.pixels.dimensions(), (1920, 1080));
    let second = player.frames.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!((second.seconds - first.seconds - 1.0 / 60.0).abs() < 0.0011);
    drop(player);
    let player = Player::file_with_options(path.clone(), 1.0, options).unwrap();
    player
        .controls
        .paused
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let first = player.frames.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(first.seconds >= 0.99);
    assert_eq!(first.pixels.dimensions(), (1920, 1080));
    assert!(player
        .frames
        .recv_timeout(Duration::from_millis(150))
        .is_err());
    drop(player);
    for subtitle in &catalog.subtitles {
        let player = Player::file_with_options(
            path.clone(),
            0.0,
            PlaybackOptions {
                audio: Selection::Off,
                subtitle: subtitle.selection.clone(),
            },
        )
        .unwrap();
        let frame = player.frames.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            frame.pixels.pixels().filter(|p| p.0[0] > 100).count() > 100,
            "Subtitle was not rendered: {}",
            subtitle.label
        );
        drop(player);
    }
    // Host-selected second audio track and subtitles survive the SSH proxy.
    let output = root.path().join("preview.ts");
    proxy::encode_selected_to_fit(
        &path,
        0.0,
        Quality::Low,
        (640, 360),
        std::fs::File::create(&output).unwrap(),
        Arc::new(AtomicBool::new(false)),
        PlaybackOptions {
            audio: catalog.audio[1].selection.clone(),
            subtitle: catalog.subtitles[0].selection.clone(),
        },
    )
    .unwrap();
    let catalog = tracks::discover(&output, Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!(catalog.audio.len(), 1);
    let player = Player::file_with_options(
        output,
        0.0,
        PlaybackOptions {
            audio: Selection::Off,
            subtitle: Selection::Off,
        },
    )
    .unwrap();
    let frame = player.frames.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(frame.pixels.dimensions(), (640, 360));
    assert!(frame.pixels.pixels().filter(|p| p.0[0] > 100).count() > 10);
}

#[test]
fn hdr_is_tone_mapped_and_anamorphic_pixels_are_normalized() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("hdr.mkv");
    assert!(Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=24",
            "-vf",
            "setsar=2/1,format=yuv420p10le",
            "-t",
            "0.2",
            "-c:v",
            "ffv1",
            "-color_primaries",
            "bt2020",
            "-color_trc",
            "smpte2084",
            "-colorspace",
            "bt2020nc",
            "-y"
        ])
        .arg(&path)
        .status()
        .unwrap()
        .success());
    let player = Player::file_with_options(
        path,
        0.0,
        PlaybackOptions {
            audio: Selection::Off,
            subtitle: Selection::Off,
        },
    )
    .unwrap();
    let frame = player
        .frames
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|error| {
            panic!(
                "HDR decoding failed: {error}; {:?}",
                player.notices.try_iter().collect::<Vec<_>>()
            )
        });
    assert_eq!(frame.pixels.dimensions(), (640, 180));
    assert!(frame.pixels.pixels().any(|p| p.0[0] > 0));
}
