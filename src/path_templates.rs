use crate::config::PathTemplateConfig;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TemplateValues {
    pub artist: Option<String>,
    pub albumartist: Option<String>,
    pub album: Option<String>,
    pub title: Option<String>,
    pub track: Option<i64>,
    pub tracktotal: Option<i64>,
    pub disc: Option<i64>,
    pub disctotal: Option<i64>,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub composer: Option<String>,
    pub isrc: Option<String>,
    pub label: Option<String>,
    pub format: Option<String>,
    pub bitrate: Option<i64>,
    pub ext: String,
}

pub fn render(
    template: &str,
    values: &TemplateValues,
    cfg: &PathTemplateConfig,
) -> Result<PathBuf> {
    let padded = |v: Option<i64>, width| v.map(|n| format!("{n:0width$}")).unwrap_or_default();
    let mut variables = vec![
        (
            "artist",
            values
                .artist
                .clone()
                .unwrap_or_else(|| cfg.unknown_artist.clone()),
        ),
        (
            "albumartist",
            values
                .albumartist
                .clone()
                .or(values.artist.clone())
                .unwrap_or_else(|| cfg.unknown_artist.clone()),
        ),
        (
            "album",
            values
                .album
                .clone()
                .unwrap_or_else(|| cfg.unknown_album.clone()),
        ),
        (
            "title",
            values
                .title
                .clone()
                .unwrap_or_else(|| cfg.unknown_title.clone()),
        ),
        ("track", padded(values.track, cfg.track_padding)),
        ("tracktotal", padded(values.tracktotal, cfg.track_padding)),
        ("disc", padded(values.disc, cfg.disc_padding)),
        ("disctotal", padded(values.disctotal, cfg.disc_padding)),
        ("year", values.year.clone().unwrap_or_default()),
        ("genre", values.genre.clone().unwrap_or_default()),
        ("composer", values.composer.clone().unwrap_or_default()),
        ("isrc", values.isrc.clone().unwrap_or_default()),
        ("label", values.label.clone().unwrap_or_default()),
        ("format", values.format.clone().unwrap_or_default()),
        (
            "bitrate",
            values.bitrate.map(|v| v.to_string()).unwrap_or_default(),
        ),
        ("ext", values.ext.clone()),
    ];
    variables.sort_by_key(|(key, _)| std::cmp::Reverse(key.len()));
    let mut output = template.to_string();
    for (key, value) in variables {
        output = output.replace(
            &format!("${key}"),
            &sanitize(&value, cfg.max_filename_length),
        );
    }
    let mut path = PathBuf::from(output);
    if path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("template escapes configured root");
    }
    path = path
        .components()
        .filter_map(|part| match part {
            Component::Normal(v) => Some(sanitize(&v.to_string_lossy(), cfg.max_filename_length)),
            _ => None,
        })
        .collect();
    let ext = values.ext.trim_start_matches('.');
    let same_ext = path
        .extension()
        .and_then(|v| v.to_str())
        .is_some_and(|v| v.eq_ignore_ascii_case(ext));
    if !same_ext {
        if path.extension().is_some() {
            path.set_extension(ext);
        } else {
            let name = format!(
                "{}.{}",
                path.file_name().and_then(|v| v.to_str()).unwrap_or("file"),
                ext
            );
            path.set_file_name(name);
        }
    }
    Ok(path)
}

pub fn resolve_collision(path: &Path, strategy: &str) -> Result<PathBuf> {
    if !path.exists() {
        return Ok(path.to_owned());
    }
    match strategy {
        "overwrite" => Ok(path.to_owned()),
        "rename" => {
            for n in 1..10_000 {
                let stem = path.file_stem().and_then(|v| v.to_str()).unwrap_or("file");
                let ext = path.extension().and_then(|v| v.to_str()).unwrap_or("");
                let candidate = path.with_file_name(if ext.is_empty() {
                    format!("{stem} ({n})")
                } else {
                    format!("{stem} ({n}).{ext}")
                });
                if !candidate.exists() {
                    return Ok(candidate);
                }
            }
            bail!("could not resolve collision")
        }
        _ => bail!("destination exists"),
    }
}

fn sanitize(value: &str, max: usize) -> String {
    let replaced: String = value
        .nfc()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .trim_end_matches(['.', ' '])
        .chars()
        .take(max)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn values() -> TemplateValues {
        TemplateValues {
            artist: Some("A/B".into()),
            albumartist: None,
            album: Some("Album".into()),
            title: Some("Song".into()),
            track: Some(1),
            tracktotal: None,
            disc: None,
            disctotal: None,
            year: None,
            genre: None,
            composer: None,
            isrc: None,
            label: None,
            format: Some("FLAC".into()),
            bitrate: None,
            ext: "flac".into(),
        }
    }
    #[test]
    fn renders_and_preserves_extension() {
        assert_eq!(
            render("$artist/$track - $title", &values(), &Default::default()).unwrap(),
            PathBuf::from("A_B/01 - Song.flac")
        );
    }
    #[test]
    fn album_artist_does_not_partially_replace_artist() {
        let mut values = values();
        values.albumartist = Some("Album Artist".into());
        assert_eq!(
            render("$albumartist/$artist", &values, &Default::default()).unwrap(),
            PathBuf::from("Album Artist/A_B.flac")
        );
    }
    #[test]
    fn blocks_traversal() {
        assert!(render("../$title", &values(), &Default::default()).is_err());
    }
}
