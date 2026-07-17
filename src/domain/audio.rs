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
    pub genre: Option<String>,
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
        genre: tag.and_then(|t| t.genre().map(|v| v.into_owned())),
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
    clean_search_tags(&mut info);
    Ok(info)
}

fn clean_search_tags(info: &mut AudioInfo) {
    let bilingual = info.album.as_deref().and_then(|album| {
        let (_, english) = album.split_once('|')?;
        let (artist, title) = english.trim().split_once(" - ")?;
        (!artist.trim().is_empty() && !title.trim().is_empty())
            .then(|| (artist.trim().to_owned(), title.trim().to_owned()))
    });
    if let Some((artist, title)) = bilingual {
        info.artist = Some(artist);
        info.title = Some(title);
        info.album = None;
    }
    let suspicious_artist = info
        .artist
        .as_deref()
        .is_none_or(|artist| artist.trim().is_empty() || artist.trim().starts_with('@'));
    let parsed = info.title.as_deref().and_then(|value| {
        let (title, artist) = value.rsplit_once('|')?;
        (!title.trim().is_empty() && !artist.trim().is_empty())
            .then(|| (title.trim().to_owned(), artist.trim().to_owned()))
    });
    if suspicious_artist && let Some((title, artist)) = parsed {
        info.title = Some(title);
        info.artist = Some(artist);
    }
    let wide_space_parts = info.title.as_deref().and_then(|value| {
        let (title, artist) = value.rsplit_once("   ")?;
        (!title.trim().is_empty() && !artist.trim().is_empty())
            .then(|| (title.trim().to_owned(), artist.trim().to_owned()))
    });
    if info.artist.as_deref().is_none_or(str::is_empty)
        && let Some((title, artist)) = wide_space_parts
    {
        info.title = Some(title);
        info.artist = Some(artist);
    }
    if let Some(artist) = info.artist.as_mut() {
        *artist = artist
            .trim()
            .trim_matches(|ch: char| matches!(ch, ',' | '،' | ';' | '؛'))
            .trim()
            .to_owned();
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_social_spam_artist_from_pipe_title() {
        let mut info = AudioInfo {
            title: Some("Колыбельная | Jah Khalib".into()),
            artist: Some("@RadioP0l".into()),
            ..Default::default()
        };
        clean_search_tags(&mut info);
        assert_eq!(info.title.as_deref(), Some("Колыбельная"));
        assert_eq!(info.artist.as_deref(), Some("Jah Khalib"));
    }

    #[test]
    fn extracts_artist_after_wide_space_separator() {
        let mut info = AudioInfo {
            title: Some("Season 3 Netflix Trailer)   X-Ray Dog".into()),
            ..Default::default()
        };
        clean_search_tags(&mut info);
        assert_eq!(info.title.as_deref(), Some("Season 3 Netflix Trailer)"));
        assert_eq!(info.artist.as_deref(), Some("X-Ray Dog"));
    }

    #[test]
    fn cleans_bilingual_scraper_tags() {
        let mut info = AudioInfo {
            title: Some("محمدرضا علیمردانی - موزیک ویدیو".into()),
            artist: Some("Mohammadreza Alimardani - Joker".into()),
            album: Some("محمدرضا علیمردانی - موزیک ویدیو | Mohammadreza Alimardani - Joker".into()),
            ..Default::default()
        };
        clean_search_tags(&mut info);
        assert_eq!(info.title.as_deref(), Some("Joker"));
        assert_eq!(info.artist.as_deref(), Some("Mohammadreza Alimardani"));
        assert_eq!(info.album, None);
    }
}
