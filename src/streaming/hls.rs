//! Bounded, just-in-time HLS. Each independently encoded fragment has its own
//! initialization section and an explicit discontinuity, across every rendition.
//! This gives seeking and ABR without background whole-library conversions.
use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

const CACHE_BYTES: u64 = 384 * 1024 * 1024;
const MAX_SEGMENT_BYTES: u64 = 48 * 1024 * 1024;
#[derive(Clone, Copy, Debug)]
enum Encoder {
    Vaapi,
    VideoToolbox,
    SoftwareHevc,
    Software,
}
impl Encoder {
    fn name(self) -> &'static str {
        match self {
            Self::Vaapi => "hevc_vaapi",
            Self::VideoToolbox => "hevc_videotoolbox",
            Self::SoftwareHevc => "libx265",
            Self::Software => "libx264",
        }
    }
    fn hevc(self) -> bool {
        !matches!(self, Self::Software)
    }
    fn default_max_height(self) -> u32 {
        // Software 4K can take several playback intervals per fragment on grainy
        // features. Advertising it lets AVPlayer select by network bandwidth even
        // though the just-in-time encoder cannot replenish that buffer in time.
        match self {
            Self::SoftwareHevc | Self::Software => 1080,
            Self::Vaapi | Self::VideoToolbox => 2160,
        }
    }
    fn input(self, c: &mut Command) {
        if matches!(self, Self::Vaapi) {
            c.args(["-vaapi_device", "/dev/dri/renderD128"]);
        }
    }
}
fn max_height(encoder: Encoder, setting: Option<&str>) -> u32 {
    setting
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|height| [480, 720, 1080, 2160].contains(height))
        .unwrap_or_else(|| encoder.default_max_height())
}
fn ffmpeg() -> String {
    std::env::var("STAR_STREAM_FFMPEG").unwrap_or_else(|_| "ffmpeg".into())
}
fn ffprobe() -> String {
    std::env::var("STAR_STREAM_FFPROBE").unwrap_or_else(|_| "ffprobe".into())
}
fn run(mut command: Command, stop: &AtomicBool, seconds: u64) -> Result<Vec<u8>> {
    let mut stdout = tempfile::tempfile()?;
    let stderr = tempfile::tempfile()?;
    command
        .stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?);
    let mut child = command.spawn().context("Start streaming codec tool")?;
    let began = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                bail!(
                    "Streaming codec failed ({}); check host codec support",
                    status
                );
            }
            ensure!(
                stdout.metadata()?.len() <= 1024 * 1024,
                "Codec metadata exceeds limit"
            );
            stdout.seek(SeekFrom::Start(0))?;
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes)?;
            return Ok(bytes);
        }
        if stop.load(Ordering::Acquire)
            || began.elapsed() > Duration::from_secs(seconds)
            || stderr.metadata()?.len() > 64 * 1024
        {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Streaming codec cancelled or timed out");
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}
fn encoder(stop: &AtomicBool) -> Encoder {
    static SELECTED: OnceLock<Encoder> = OnceLock::new();
    *SELECTED.get_or_init(|| {
        if std::env::var("STAR_STREAM_ENCODER").as_deref() == Ok("software") {
            return Encoder::Software;
        }
        let candidates = if cfg!(target_os = "linux") {
            // The Radeon 8060S can report successful HEVC encoding and decoding
            // while producing visibly corrupt frames, including in CQP mode.
            // A successful codec probe cannot establish picture correctness.
            // Keep VAAPI opt-in until its driver is verified with real footage.
            if std::env::var("STAR_STREAM_ENCODER").as_deref() == Ok("vaapi") {
                vec![Encoder::Vaapi, Encoder::SoftwareHevc]
            } else {
                vec![Encoder::SoftwareHevc]
            }
        } else if cfg!(target_os = "macos") {
            vec![Encoder::VideoToolbox]
        } else {
            vec![]
        };
        for candidate in candidates {
            let mut c = Command::new(ffmpeg());
            c.args(["-hide_banner", "-loglevel", "error"]);
            candidate.input(&mut c);
            c.args([
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=192x128:rate=24",
                "-frames:v",
                "2",
            ]);
            if matches!(candidate, Encoder::Vaapi) {
                c.args(["-vf", "format=nv12,hwupload"]);
            }
            c.args(["-c:v", candidate.name()]);
            if matches!(candidate, Encoder::Vaapi) {
                c.args(["-rc_mode", "CQP", "-qp", "26"]);
            } else {
                c.args(["-b:v", "500k"]);
            }
            if matches!(candidate, Encoder::SoftwareHevc) {
                c.args([
                    "-preset",
                    "veryfast",
                    "-x265-params",
                    "pools=4:frame-threads=2:log-level=error",
                ]);
            }
            c.args(["-f", "null", "-"]);
            if run(c, stop, 10).is_ok() {
                return candidate;
            }
        }
        Encoder::Software
    })
}
#[derive(Clone)]
struct Quality {
    name: String,
    width: u32,
    height: u32,
    rate: u32,
}
struct Segment {
    file: tempfile::NamedTempFile,
    split: u64,
    size: u64,
    used: Instant,
}
struct Cache {
    segments: HashMap<(String, u32), Segment>,
    bytes: u64,
}
pub(super) struct Adaptive {
    source: PathBuf,
    duration: f64,
    step: f64,
    fps: String,
    hdr: bool,
    audio: bool,
    encoder: Encoder,
    qualities: Vec<Quality>,
    cache: Mutex<Cache>,
}
pub(super) struct Part {
    pub file: File,
    pub offset: u64,
    pub length: u64,
    pub mime: &'static str,
}
#[derive(Default)]
pub(super) struct Hls {
    movies: Mutex<HashMap<String, Arc<Adaptive>>>,
}
impl Hls {
    pub fn diagnostics(&self) -> Value {
        let Ok(movies) = self.movies.try_lock() else {
            return serde_json::json!({"busy":true});
        };
        let entries=movies.iter().map(|(id,movie)| {
            let cache=movie.cache.try_lock();
            serde_json::json!({"id":id,"fps":movie.fps,"qualities":movie.qualities.iter().map(|q|serde_json::json!({"name":q.name,"width":q.width,"height":q.height,"bitrate_bps":q.rate})).collect::<Vec<_>>(),"movie":movie.source.file_name().map(|n|n.to_string_lossy()),"encoder":movie.encoder.name(),"segment_seconds":movie.step,
                "cache_bytes":cache.as_ref().ok().map(|c|c.bytes),"cached_segments":cache.as_ref().ok().map(|c|c.segments.len()),"busy":cache.is_err()})
        }).collect::<Vec<_>>();
        serde_json::json!({"movies":entries,"cache_limit_per_movie_bytes":CACHE_BYTES,"segment_limit_bytes":MAX_SEGMENT_BYTES,
            "encoder_override":std::env::var("STAR_STREAM_ENCODER").ok()})
    }
    pub fn clear_idle(&self) -> Value {
        let mut cleared = 0;
        if let Ok(movies) = self.movies.try_lock() {
            for movie in movies.values() {
                if Arc::strong_count(movie) != 1 {
                    continue;
                }
                let Ok(mut cache) = movie.cache.try_lock() else {
                    continue;
                };
                cleared += cache.segments.len();
                cache.segments.clear();
                cache.bytes = 0;
            }
        }
        serde_json::json!({"cleared_segments":cleared})
    }
    pub fn movie(&self, id: &str, source: &Path, stop: &AtomicBool) -> Result<Arc<Adaptive>> {
        let mut movies = self
            .movies
            .lock()
            .map_err(|_| anyhow::anyhow!("HLS manager unavailable"))?;
        if let Some(movie) = movies.get(id) {
            return Ok(movie.clone());
        }
        // At most two movie caches / encoder jobs per SSH service. An active
        // request retains its movie, so eviction never cancels a serving worker.
        if movies.len() >= 2 {
            let victim = movies
                .iter()
                .find(|(_, m)| Arc::strong_count(m) == 1)
                .map(|(id, _)| id.clone());
            if let Some(victim) = victim {
                movies.remove(&victim);
            } else {
                bail!("Two movies are already being prepared");
            }
        }
        let movie = Arc::new(Adaptive::new(source, stop)?);
        movies.insert(id.into(), movie.clone());
        Ok(movie)
    }
}
impl Adaptive {
    fn new(source: &Path, stop: &AtomicBool) -> Result<Self> {
        let mut c = Command::new(ffprobe());
        c.args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=codec_type,width,height,avg_frame_rate,color_transfer",
            "-of",
            "json",
        ])
        .arg(source);
        let info: Value = serde_json::from_slice(&run(c, stop, 15)?)?;
        let streams = info["streams"].as_array().context("No media streams")?;
        let video = streams
            .iter()
            .find(|s| s["codec_type"] == "video")
            .context("No movie video")?;
        let width = video["width"].as_u64().context("Missing movie width")? as u32;
        let height = video["height"].as_u64().context("Missing movie height")? as u32;
        ensure!(
            (2..=8192).contains(&width) && (2..=8192).contains(&height),
            "Unsupported movie dimensions"
        );
        let duration = info["format"]["duration"]
            .as_str()
            .context("Missing movie duration")?
            .parse::<f64>()?;
        ensure!(
            duration.is_finite() && duration > 0.0 && duration <= 24.0 * 3600.0,
            "Unsupported movie duration"
        );
        let fps_text = video["avg_frame_rate"].as_str().unwrap_or("24/1");
        let fps = fps_text
            .split_once('/')
            .and_then(|(a, b)| Some(a.parse::<f64>().ok()? / b.parse::<f64>().ok()?))
            .filter(|f| f.is_finite() && *f >= 1.0 && *f <= 60.0)
            .unwrap_or(24.0);
        let step = (6.0 * fps).round() / fps;
        let encoder = encoder(stop);
        let max_height = max_height(
            encoder,
            std::env::var("STAR_STREAM_MAX_HEIGHT").ok().as_deref(),
        );
        let mut qualities = Vec::new();
        for (name, w, h, rate) in [
            ("480", 854, 480, 1_200_000),
            ("720", 1280, 720, 2_800_000),
            ("1080", 1920, 1080, 6_000_000),
            ("2160", 3840, 2160, 20_000_000),
        ] {
            if h > max_height || (name == "2160" && !encoder.hevc()) {
                continue;
            }
            let scale = (f64::from(w) / f64::from(width))
                .min(f64::from(h) / f64::from(height))
                .min(1.0);
            let out_w = ((f64::from(width) * scale / 2.0).floor() as u32 * 2).max(2);
            let out_h = ((f64::from(height) * scale / 2.0).floor() as u32 * 2).max(2);
            if qualities
                .last()
                .is_some_and(|q: &Quality| q.width == out_w && q.height == out_h)
            {
                continue;
            }
            qualities.push(Quality {
                name: name.into(),
                width: out_w,
                height: out_h,
                rate,
            });
            if scale >= 1.0 {
                break;
            }
        }
        Ok(Self {
            source: source.into(),
            duration,
            step,
            fps: format!("{fps:.9}"),
            hdr: matches!(
                video["color_transfer"].as_str(),
                Some("smpte2084" | "arib-std-b67")
            ),
            audio: streams.iter().any(|s| s["codec_type"] == "audio"),
            encoder,
            qualities,
            cache: Mutex::new(Cache {
                segments: HashMap::new(),
                bytes: 0,
            }),
        })
    }
    pub fn master(&self, id: &str, token: &str) -> Vec<u8> {
        let mut text = String::from("#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-INDEPENDENT-SEGMENTS\n");
        // Start near 1080p rather than asking for the largest encode on startup.
        let mut qualities = self.qualities.iter().collect::<Vec<_>>();
        qualities.sort_by_key(|q| if q.name == "1080" { 0 } else { q.rate });
        for q in qualities {
            let codec = if self.encoder.hevc() {
                "hvc1.1.6.L153.B0"
            } else {
                "avc1.640033"
            };
            let audio = if self.audio { ",mp4a.40.2" } else { "" };
            text.push_str(&format!("#EXT-X-STREAM-INF:BANDWIDTH={},AVERAGE-BANDWIDTH={},RESOLUTION={}x{},FRAME-RATE={},CODECS=\"{codec}{audio}\"\n/v1/hls/{id}/{}/index.m3u8?token={token}\n",q.rate*13/10+256_000,q.rate+192_000,q.width,q.height,self.fps,q.name));
        }
        text.into_bytes()
    }
    pub fn playlist(&self, id: &str, quality: &str, token: &str) -> Result<Vec<u8>> {
        ensure!(
            self.qualities.iter().any(|q| q.name == quality),
            "Unknown quality"
        );
        let mut text=format!("#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:{}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-INDEPENDENT-SEGMENTS\n",self.step.ceil() as u32);
        for index in 0..self.count() {
            if index > 0 {
                text.push_str("#EXT-X-DISCONTINUITY\n");
            }
            let base = format!("/v1/hls/{id}/{quality}/{index}");
            let duration = (self.duration - f64::from(index) * self.step).min(self.step);
            text.push_str(&format!("#EXT-X-MAP:URI=\"{base}/init.mp4?token={token}\"\n#EXTINF:{duration:.6},\n{base}/media.m4s?token={token}\n"));
        }
        text.push_str("#EXT-X-ENDLIST\n");
        Ok(text.into_bytes())
    }
    fn count(&self) -> u32 {
        (self.duration / self.step).ceil() as u32
    }
    pub fn part(&self, quality: &str, index: u32, init: bool, stop: &AtomicBool) -> Result<Part> {
        ensure!(index < self.count(), "Unknown segment");
        let q = self
            .qualities
            .iter()
            .find(|q| q.name == quality)
            .context("Unknown quality")?;
        let key = (quality.to_string(), index);
        // One encode per movie at a time. Cached reads release this lock before
        // socket writes, so a slow client cannot retain the encoder/cache lock.
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| anyhow::anyhow!("HLS cache unavailable"))?;
        if !cache.segments.contains_key(&key) {
            let segment = self.encode(q, index, stop)?;
            while cache.bytes + segment.size > CACHE_BYTES {
                let victim = cache
                    .segments
                    .iter()
                    .min_by_key(|(_, s)| s.used)
                    .map(|(k, _)| k.clone());
                let Some(victim) = victim else {
                    break;
                };
                if let Some(old) = cache.segments.remove(&victim) {
                    cache.bytes -= old.size;
                }
            }
            cache.bytes += segment.size;
            cache.segments.insert(key.clone(), segment);
        }
        let segment = cache
            .segments
            .get_mut(&key)
            .context("Missing HLS segment")?;
        segment.used = Instant::now();
        Ok(Part {
            file: File::open(segment.file.path())?,
            offset: if init { 0 } else { segment.split },
            length: if init {
                segment.split
            } else {
                segment.size - segment.split
            },
            mime: if init {
                "video/mp4"
            } else {
                "video/iso.segment"
            },
        })
    }
    fn encode(&self, q: &Quality, index: u32, stop: &AtomicBool) -> Result<Segment> {
        if !matches!(self.encoder, Encoder::Vaapi) {
            return self.encode_at_qp(q, index, stop, None, 45);
        }
        // Radeon 8060S's VAAPI bitrate-control mode emits invalid HEVC CU QP
        // deltas on real 4K material. CQP decodes cleanly. Enforce the rendition's
        // byte budget ourselves, retrying with coarser quantization if needed.
        let began = Instant::now();
        let initial = match q.name.as_str() {
            "480" => 28,
            "720" => 26,
            _ => 24,
        };
        let duration = (self.duration - f64::from(index) * self.step).min(self.step);
        let budget = ((f64::from(q.rate) * 1.2 + 192_000.0) * duration / 8.0) as u64 + 16 * 1024;
        for qp in (initial..=48).step_by(4) {
            let remaining = 45_u64.saturating_sub(began.elapsed().as_secs());
            ensure!(
                remaining > 0,
                "Hardware segment exceeded its encoding deadline"
            );
            let segment = self.encode_at_qp(q, index, stop, Some(qp), remaining)?;
            if segment.size <= budget {
                return Ok(segment);
            }
        }
        bail!("Hardware segment could not meet the rendition bandwidth budget")
    }
    fn encode_at_qp(
        &self,
        q: &Quality,
        index: u32,
        stop: &AtomicBool,
        qp: Option<u8>,
        deadline: u64,
    ) -> Result<Segment> {
        let file = tempfile::NamedTempFile::new()?;
        let start = f64::from(index) * self.step;
        let duration = (self.duration - start).min(self.step);
        let mut c = Command::new(ffmpeg());
        c.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"]);
        self.encoder.input(&mut c);
        c.args(["-ss", &format!("{start:.9}"), "-threads", "4", "-i"])
            .arg(&self.source);
        c.args(["-t", &format!("{duration:.9}"), "-map", "0:v:0"]);
        if self.audio {
            c.args(["-map", "0:a:0"]);
        }
        let tone = if self.hdr {
            "zscale=t=linear:npl=100,format=gbrpf32le,tonemap=hable:desat=0,zscale=p=bt709:t=bt709:m=bt709:r=tv,"
        } else {
            ""
        };
        let upload = if matches!(self.encoder, Encoder::Vaapi) {
            ",format=nv12,hwupload"
        } else {
            ",format=yuv420p"
        };
        c.args([
            "-vf",
            &format!(
                "{tone}scale={}:{},setsar=1,setpts=PTS-STARTPTS{upload}",
                q.width, q.height
            ),
            "-r",
            &self.fps,
            "-c:v",
            self.encoder.name(),
        ]);
        if let Some(qp) = qp {
            c.args(["-rc_mode", "CQP", "-qp", &qp.to_string()]);
        } else {
            c.args([
                "-b:v",
                &q.rate.to_string(),
                "-maxrate",
                &(q.rate * 12 / 10).to_string(),
                "-bufsize",
                &(q.rate * 2).to_string(),
            ]);
        }
        c.args(["-bf", "0", "-g", "360"]);
        if self.encoder.hevc() {
            c.args(["-tag:v", "hvc1", "-profile:v", "main", "-level:v", "5.1"]);
            if matches!(self.encoder, Encoder::SoftwareHevc) {
                c.args([
                    "-preset",
                    "veryfast",
                    "-x265-params",
                    "pools=4:frame-threads=2:log-level=error",
                ]);
            }
        } else {
            c.args([
                "-preset",
                "veryfast",
                "-profile:v",
                "high",
                "-level:v",
                "5.1",
                "-threads",
                "4",
            ]);
        }
        if self.hdr {
            c.args([
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
                "-colorspace",
                "bt709",
            ]);
        }
        if self.audio {
            c.args([
                "-af",
                "asetpts=PTS-STARTPTS",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
                "-ac",
                "2",
                "-ar",
                "48000",
            ]);
        }
        c.args([
            "-sn",
            "-dn",
            "-map_metadata",
            "-1",
            "-map_chapters",
            "-1",
            "-movflags",
            "+empty_moov+default_base_moof+frag_keyframe",
            "-frag_duration",
            "1000000",
            "-f",
            "mp4",
        ])
        .arg(file.path());
        run(c, stop, deadline)?;
        let size = file.as_file().metadata()?.len();
        ensure!(
            size <= MAX_SEGMENT_BYTES,
            "Encoded segment exceeds cache limit"
        );
        let split = fragment_start(file.as_file().try_clone()?, size)?;
        Ok(Segment {
            file,
            split,
            size,
            used: Instant::now(),
        })
    }
}
fn fragment_start(mut file: File, size: u64) -> Result<u64> {
    let mut at = 0;
    while at + 8 <= size {
        file.seek(SeekFrom::Start(at))?;
        let mut header = [0; 8];
        file.read_exact(&mut header)?;
        if &header[4..] == b"moof" {
            ensure!(at > 0, "Missing MP4 initialization");
            return Ok(at);
        }
        let mut length = u64::from(u32::from_be_bytes(header[..4].try_into()?));
        if length == 1 {
            let mut extended = [0; 8];
            file.read_exact(&mut extended)?;
            length = u64::from_be_bytes(extended);
        }
        ensure!(length >= 8 && length <= size - at, "Invalid MP4 fragment");
        at += length;
    }
    bail!("Encoder produced no movie fragment")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn software_auto_avoids_unsustainable_4k_but_allows_an_explicit_cap() {
        assert_eq!(max_height(Encoder::SoftwareHevc, None), 1080);
        assert_eq!(max_height(Encoder::Software, None), 1080);
        assert_eq!(max_height(Encoder::VideoToolbox, None), 2160);
        assert_eq!(max_height(Encoder::SoftwareHevc, Some("2160")), 2160);
        assert_eq!(max_height(Encoder::SoftwareHevc, Some("720")), 720);
        assert_eq!(max_height(Encoder::SoftwareHevc, Some("0")), 1080);
        assert_eq!(max_height(Encoder::SoftwareHevc, Some("9999")), 1080);
    }
    #[test]
    fn cancelled_codec_is_terminated_promptly() {
        let mut command = Command::new("sleep");
        command.arg("30");
        let began = Instant::now();
        assert!(run(command, &AtomicBool::new(true), 30).is_err());
        assert!(began.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn malformed_mp4_box_lengths_are_rejected() {
        for bytes in [
            &b"\0\0\0\0free"[..],
            &b"\0\0\0\x30free"[..],
            &b"\0\0\0\x08moof"[..],
        ] {
            let mut file = tempfile::tempfile().unwrap();
            std::io::Write::write_all(&mut file, bytes).unwrap();
            assert!(fragment_start(file, bytes.len() as u64).is_err());
        }
    }
}
