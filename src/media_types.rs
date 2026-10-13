//! Media descriptions and track choices without codecs or audio devices.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Quality {
    Low,
    Balanced,
    High,
}
impl Quality {
    pub fn parameters(self) -> (u32, u32, u32) {
        match self {
            Self::Low => (360, 15, 750_000),
            Self::Balanced => (480, 24, 1_500_000),
            Self::High => (720, 30, 3_000_000),
        }
    }
    pub fn down(self) -> Self {
        match self {
            Self::High => Self::Balanced,
            _ => Self::Low,
        }
    }
    pub fn up(self) -> Self {
        match self {
            Self::Low => Self::Balanced,
            _ => Self::High,
        }
    }
}
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
