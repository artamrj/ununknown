use anyhow::{Context, Result, anyhow};
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
    let tagged = || -> lofty::error::Result<lofty::file::TaggedFile> {
        Probe::open(path)?.guess_file_type()?.read()
    };
    let mut info = match tagged() {
        Ok(tagged) => info_from_tagged(path, &tagged),
        Err(tag_error) => info_from_ffprobe(path).with_context(|| {
            format!("tag reader failed ({tag_error}); FFprobe fallback also failed")
        })?,
    };
    fill_from_filename(path, &mut info);
    clean_search_tags(&mut info);
    info.artist = info
        .artist
        .as_deref()
        .map(crate::domain::credits::prefer_latin_alias);
    info.album_artist = info
        .album_artist
        .as_deref()
        .map(crate::domain::credits::prefer_latin_alias);
    if let (Some(artist), Some(title)) = (info.artist.as_deref(), info.title.as_deref()) {
        let credits = crate::domain::credits::normalize_featured(artist, title);
        info.artist = Some(credits.artist);
        info.title = Some(credits.title);
    }
    Ok(info)
}

fn info_from_tagged(path: &Path, tagged: &lofty::file::TaggedFile) -> AudioInfo {
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let props = tagged.properties();
    AudioInfo {
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
        format: canonical_lofty_format(tagged.file_type(), path),
    }
}

fn info_from_ffprobe(path: &Path) -> Result<AudioInfo> {
    let output = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "format=format_name,duration,bit_rate:format_tags=title,artist,album,album_artist,track,genre:stream=codec_name,duration,bit_rate:stream_tags=title,artist,album,album_artist,track,genre",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .context("could not start ffprobe")?;
    if !output.status.success() {
        return Err(anyhow!(
            "ffprobe could not read audio: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_ffprobe(path, &serde_json::from_slice(&output.stdout)?)
}

fn parse_ffprobe(path: &Path, value: &serde_json::Value) -> Result<AudioInfo> {
    let format = &value["format"];
    let stream = value["streams"]
        .as_array()
        .and_then(|streams| streams.first())
        .unwrap_or(&serde_json::Value::Null);
    let format_tags = &format["tags"];
    let stream_tags = &stream["tags"];
    let tag = |name: &str| {
        format_tags[name]
            .as_str()
            .or_else(|| stream_tags[name].as_str())
            .map(str::to_owned)
    };
    let number = |name: &str| {
        format[name]
            .as_str()
            .or_else(|| stream[name].as_str())
            .and_then(|number| number.parse::<f64>().ok())
    };
    let duration = number("duration").ok_or_else(|| anyhow!("ffprobe returned no duration"))?;
    let bitrate = number("bit_rate").map(|value| (value / 1000.0).round() as u32);
    Ok(AudioInfo {
        title: tag("title"),
        artist: tag("artist"),
        album: tag("album"),
        album_artist: tag("album_artist"),
        track_number: tag("track")
            .as_deref()
            .and_then(|track| track.split('/').next())
            .and_then(|track| track.parse().ok()),
        genre: tag("genre"),
        duration,
        bitrate,
        format: canonical_ffprobe_format(
            format["format_name"].as_str(),
            stream["codec_name"].as_str(),
            path,
        ),
    })
}

fn canonical_lofty_format(file_type: lofty::file::FileType, path: &Path) -> String {
    use lofty::file::FileType;
    match file_type {
        FileType::Aac => "aac",
        FileType::Aiff => "aiff",
        FileType::Ape => "ape",
        FileType::Flac => "flac",
        FileType::Mpeg => "mp3",
        FileType::Mp4 => "m4a",
        FileType::Mpc => "mpc",
        FileType::Opus => "opus",
        FileType::Vorbis => "ogg",
        FileType::Speex => "spx",
        FileType::Wav => "wav",
        FileType::WavPack => "wv",
        _ => path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default(),
    }
    .to_owned()
}

fn canonical_ffprobe_format(
    format_name: Option<&str>,
    codec_name: Option<&str>,
    path: &Path,
) -> String {
    let format = format_name.unwrap_or_default();
    if format
        .split(',')
        .any(|name| matches!(name, "mov" | "mp4" | "m4a" | "3gp" | "3g2" | "mj2"))
    {
        return "m4a".into();
    }
    if format.contains("mp3") {
        return "mp3".into();
    }
    if format.contains("flac") {
        return "flac".into();
    }
    if format.contains("ogg") {
        return if codec_name == Some("opus") {
            "opus"
        } else {
            "ogg"
        }
        .into();
    }
    path.extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn fill_from_filename(path: &Path, info: &mut AudioInfo) {
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

    #[test]
    fn parses_ffprobe_fallback_metadata() {
        let info = parse_ffprobe(
            Path::new("Artist - Song.mp3"),
            &serde_json::json!({
                "streams": [{"duration": "181.25", "bit_rate": "192000", "tags": {}}],
                "format": {"tags": {"title": "Song", "artist": "Artist", "track": "3/10"}}
            }),
        )
        .unwrap();
        assert_eq!(info.title.as_deref(), Some("Song"));
        assert_eq!(info.artist.as_deref(), Some("Artist"));
        assert_eq!(info.track_number, Some(3));
        assert_eq!(info.duration, 181.25);
        assert_eq!(info.bitrate, Some(192));
    }

    #[test]
    fn ffprobe_corrects_an_mp4_file_with_mp3_extension() {
        let info = parse_ffprobe(
            Path::new("wrong.mp3"),
            &serde_json::json!({
                "streams": [{"codec_name": "aac", "duration": "173.6", "tags": {}}],
                "format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2", "tags": {}}
            }),
        )
        .unwrap();
        assert_eq!(info.format, "m4a");
    }
}
