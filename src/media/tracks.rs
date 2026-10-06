//! Source track discovery and explicit selection, shared by local and SSH players.
use super::*;
use av::{format, media};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Selection {
    #[default]
    Auto,
    Off,
    Stream(usize),
    External(PathBuf),
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackOptions {
    pub audio: Selection,
    pub subtitle: Selection,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub selection: Selection,
    pub label: String,
    pub default: bool,
    pub forced: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Tracks {
    pub audio: Vec<Track>,
    pub subtitles: Vec<Track>,
}
impl Tracks {
    pub fn contains_audio(&self, selection: &Selection) -> bool {
        matches!(selection, Selection::Auto | Selection::Off)
            || self.audio.iter().any(|t| &t.selection == selection)
    }
    pub fn contains_subtitle(&self, selection: &Selection) -> bool {
        matches!(selection, Selection::Auto | Selection::Off)
            || self.subtitles.iter().any(|t| &t.selection == selection)
    }
}
pub fn audio_index(input: &format::context::Input, selection: &Selection) -> Option<usize> {
    match selection {
        Selection::Off | Selection::External(_) => None,
        Selection::Stream(index) => input
            .stream(*index)
            .filter(|s| s.parameters().medium() == media::Type::Audio)
            .map(|s| s.index()),
        Selection::Auto => input
            .streams()
            .find(|s| {
                s.parameters().medium() == media::Type::Audio
                    && s.disposition()
                        .contains(format::stream::Disposition::DEFAULT)
            })
            .or_else(|| input.streams().best(media::Type::Audio))
            .map(|s| s.index()),
    }
}
pub fn subtitle_selection(input: &format::context::Input, selection: &Selection) -> Selection {
    if *selection != Selection::Auto {
        return selection.clone();
    }
    input
        .streams()
        .find(|s| {
            s.parameters().medium() == media::Type::Subtitle
                && s.disposition()
                    .contains(format::stream::Disposition::FORCED)
        })
        .map_or(Selection::Off, |s| Selection::Stream(s.index()))
}
pub fn discover(path: &Path, cancel: Arc<AtomicBool>) -> Result<Tracks> {
    init()?;
    let start = std::time::Instant::now();
    let input = format::input_with_interrupt(path, move || {
        cancel.load(Ordering::Relaxed) || start.elapsed() > std::time::Duration::from_secs(5)
    })?;
    let mut tracks = Tracks::default();
    for s in input.streams() {
        let medium = s.parameters().medium();
        if !matches!(medium, media::Type::Audio | media::Type::Subtitle) {
            continue;
        }
        let m = s.metadata();
        let mut parts = vec![format!("Track {}", s.index() + 1)];
        if let Some(value) = m.get("language") {
            parts.push(value.into());
        }
        if let Some(value) = m.get("title") {
            parts.push(value.into());
        }
        parts.push(s.parameters().id().name().into());
        if medium == media::Type::Audio {
            if let Ok(d) = av::codec::context::Context::from_parameters(s.parameters())?
                .decoder()
                .audio()
            {
                parts.push(format!("{} ch · {} Hz", d.channels(), d.rate()));
            }
        }
        let default = s
            .disposition()
            .contains(format::stream::Disposition::DEFAULT);
        let forced = s
            .disposition()
            .contains(format::stream::Disposition::FORCED);
        if default {
            parts.push("default".into());
        }
        if forced {
            parts.push("forced".into());
        }
        let track = Track {
            selection: Selection::Stream(s.index()),
            label: parts.join(" · "),
            default,
            forced,
        };
        if medium == media::Type::Audio {
            tracks.audio.push(track);
        } else {
            tracks.subtitles.push(track);
        }
    }
    if let (Some(parent), Some(stem)) = (path.parent(), path.file_stem().and_then(|s| s.to_str())) {
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.take(4096).flatten() {
                let p = entry.path();
                let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if (name == stem || name.starts_with(&format!("{stem}.")))
                    && p.extension().and_then(|s| s.to_str()).is_some_and(|s| {
                        matches!(
                            s.to_ascii_lowercase().as_str(),
                            "srt" | "ass" | "ssa" | "vtt"
                        )
                    })
                {
                    tracks.subtitles.push(Track {
                        selection: Selection::External(p.clone()),
                        label: p
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        default: false,
                        forced: false,
                    });
                }
            }
        }
    }
    tracks.subtitles.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(tracks)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selections_are_explicit_and_catalog_checked() {
        let tracks = Tracks::default();
        assert!(tracks.contains_audio(&Selection::Off));
        assert!(!tracks.contains_audio(&Selection::Stream(17)));
        let options = PlaybackOptions {
            audio: Selection::Off,
            subtitle: Selection::External("movie.en.srt".into()),
        };
        assert_eq!(
            options,
            serde_json::from_str(&serde_json::to_string(&options).unwrap()).unwrap()
        );
    }
}
