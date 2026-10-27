fn main() {
    println!("cargo:rustc-check-cfg=cfg(ffmpeg_channel_layout_v2)");
    if std::env::var_os("CARGO_FEATURE_AUDIO_OUTPUT").is_some() {
        let avutil = pkg_config::Config::new()
            .cargo_metadata(false)
            .probe("libavutil")
            .expect("audio-output requires FFmpeg development libraries");
        let major: u32 = avutil.version.split('.').next().unwrap().parse().unwrap();
        if major >= 59 {
            println!("cargo:rustc-cfg=ffmpeg_channel_layout_v2");
        }
    }
}
