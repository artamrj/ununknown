use anyhow::Result;
use lofty::{file::AudioFile, prelude::*, probe::Probe};
use serde::Serialize;
use std::path::Path;

#[derive(Clone, Debug, Default, Serialize)]
pub struct AudioInfo {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub duration: f64,
    pub bitrate: Option<u32>,
    pub format: String,
}

pub fn read(path: &Path) -> Result<AudioInfo> {
    let tagged = Probe::open(path)?.read()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let props = tagged.properties();
    let mut info = AudioInfo {
        title: tag.and_then(|t| t.title().map(|v| v.into_owned())),
        artist: tag.and_then(|t| t.artist().map(|v| v.into_owned())),
        album: tag.and_then(|t| t.album().map(|v| v.into_owned())),
        album_artist: tag.and_then(|t| {
            t.get_string(lofty::tag::ItemKey::AlbumArtist)
                .map(str::to_owned)
        }),
        track_number: tag.and_then(|t| t.track()),
        duration: props.duration().as_secs_f64(),
        bitrate: props.audio_bitrate(),
        format: path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase(),
    };
    if info.title.as_deref().is_none_or(str::is_empty) {
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("Unknown track")
            .replace('_', " ");
        if let Some((artist, title)) = stem.split_once(" - ") {
            if info.artist.as_deref().is_none_or(str::is_empty) {
                info.artist = Some(artist.trim().to_owned());
            }
            info.title = Some(title.trim().to_owned());
        } else {
            info.title = Some(stem.trim().to_owned());
        }
    }
    Ok(info)
}

pub fn is_supported(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("mp3" | "flac" | "m4a" | "ogg" | "opus" | "wav" | "aiff" | "aif")
    )
}
