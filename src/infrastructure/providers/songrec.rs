use super::Candidate;
use crate::infrastructure::provider_cache::{ProviderCache, fingerprint_key};
use anyhow::{Context, Result, anyhow, bail};
use chrono::{Duration as ChronoDuration, Utc};
use serde_json::Value;
use sqlx::SqlitePool;
use std::{
    path::Path,
    process::Stdio,
    sync::OnceLock,
    time::{Duration, UNIX_EPOCH},
};
use tokio::process::Command;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CliKind {
    SongRec,
    SongRecLib,
}

#[derive(Clone, Debug)]
struct CommandSpec {
    executable: String,
    kind: CliKind,
}

static COMMAND_SPEC: OnceLock<Option<CommandSpec>> = OnceLock::new();

/// Reports whether either the maintained SongRec CLI or songrec-lib's headless
/// CLI is available. `UNUNKNOWN_SONGREC_BIN` can point at a non-PATH binary.
pub fn available() -> bool {
    command_spec().is_some()
}

pub async fn recognize(
    pool: &SqlitePool,
    path: &Path,
    fingerprint: &str,
) -> Result<Vec<Candidate>> {
    let cache_key = cache_key(path, fingerprint)?;
    let raw = if let Some(value) = ProviderCache::get(pool, "songrec", &cache_key).await? {
        value
    } else {
        let spec = command_spec().ok_or_else(|| {
            anyhow!(
                "SongRec is not installed; install `songrec` or `songrec-lib-cli`, or set UNUNKNOWN_SONGREC_BIN"
            )
        })?;
        let value = run(&spec, path).await?;
        let matched = parse_result(&value).is_some();
        ProviderCache::put(
            pool,
            "songrec",
            &cache_key,
            &value,
            Utc::now()
                + if matched {
                    ChronoDuration::days(90)
                } else {
                    ChronoDuration::days(7)
                },
        )
        .await?;
        value
    };
    Ok(parse_result(&raw).into_iter().collect())
}

fn command_spec() -> Option<CommandSpec> {
    COMMAND_SPEC.get_or_init(detect_command).clone()
}

fn detect_command() -> Option<CommandSpec> {
    if let Ok(executable) = std::env::var("UNUNKNOWN_SONGREC_BIN") {
        let executable = executable.trim();
        if !executable.is_empty() && command_responds(executable) {
            return Some(CommandSpec {
                kind: kind_from_name(executable),
                executable: executable.to_owned(),
            });
        }
    }
    [
        ("songrec", CliKind::SongRec),
        ("songrec-lib-cli", CliKind::SongRecLib),
    ]
    .into_iter()
    .find_map(|(executable, kind)| {
        command_responds(executable).then(|| CommandSpec {
            executable: executable.to_owned(),
            kind,
        })
    })
}

fn kind_from_name(executable: &str) -> CliKind {
    Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| name.contains("songrec-lib"))
        .map_or(CliKind::SongRec, |_| CliKind::SongRecLib)
}

fn command_responds(executable: &str) -> bool {
    std::process::Command::new(executable)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

async fn run(spec: &CommandSpec, path: &Path) -> Result<Value> {
    let mut command = Command::new(&spec.executable);
    match spec.kind {
        CliKind::SongRec => {
            command.arg("recognize").arg("--json").arg(path);
        }
        CliKind::SongRecLib => {
            command
                .arg("recognize")
                .arg(path)
                .args(["--format", "json", "--quiet"]);
        }
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(45), command.output())
        .await
        .context("SongRec recognition timed out")?
        .with_context(|| format!("could not start {}", spec.executable))?;
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    if is_no_match(&diagnostic) {
        return Ok(serde_json::json!({"matches": []}));
    }
    if !output.status.success() {
        let detail = diagnostic.trim();
        bail!(
            "SongRec recognition failed: {}",
            if detail.is_empty() {
                "the command returned no diagnostic"
            } else {
                detail
            }
        );
    }
    parse_json_output(&output.stdout)
}

fn is_no_match(diagnostic: &str) -> bool {
    let diagnostic = diagnostic.to_ascii_lowercase();
    diagnostic.contains("no match")
        || diagnostic.contains("no track found")
        || diagnostic.contains("not recognized")
}

fn parse_json_output(output: &[u8]) -> Result<Value> {
    if let Ok(value) = serde_json::from_slice(output) {
        return Ok(value);
    }
    let text = String::from_utf8_lossy(output);
    let start = text
        .find('{')
        .ok_or_else(|| anyhow!("SongRec returned no JSON result"))?;
    let end = text
        .rfind('}')
        .ok_or_else(|| anyhow!("SongRec returned incomplete JSON"))?;
    serde_json::from_str(&text[start..=end]).context("SongRec returned invalid JSON")
}

fn cache_key(path: &Path, fingerprint: &str) -> Result<String> {
    if !fingerprint.trim().is_empty() {
        return Ok(fingerprint_key(fingerprint));
    }
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("could not inspect {} for SongRec cache", path.display()))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    Ok(fingerprint_key(&format!(
        "file:{}:{}:{modified}",
        path.display(),
        metadata.len()
    )))
}

fn parse_result(value: &Value) -> Option<Candidate> {
    let raw = value.get("raw_response").unwrap_or(value);
    let track = raw.get("track").or_else(|| raw.pointer("/matches/0/track"));
    let title = track
        .and_then(|track| track.get("title"))
        .and_then(Value::as_str)
        .or_else(|| value.get("song_name").and_then(Value::as_str))?
        .trim();
    let artist = track
        .and_then(|track| track.get("subtitle"))
        .and_then(Value::as_str)
        .or_else(|| value.get("artist_name").and_then(Value::as_str))?
        .trim();
    if title.is_empty()
        || artist.is_empty()
        || title.eq_ignore_ascii_case("unknown")
        || artist.eq_ignore_ascii_case("unknown")
    {
        return None;
    }
    let album = song_metadata(raw, "Album")
        .or_else(|| value.get("album_name").and_then(Value::as_str))
        .map(str::to_owned);
    let released = song_metadata(raw, "Released")
        .or_else(|| value.get("release_year").and_then(Value::as_str))
        .map(str::to_owned);
    let genre = track
        .and_then(|track| track.pointer("/genres/primary"))
        .and_then(Value::as_str)
        .or_else(|| value.get("genre").and_then(Value::as_str))
        .map(str::to_owned);
    let label = song_metadata(raw, "Label").map(str::to_owned);
    let isrc = track
        .and_then(|track| track.get("isrc"))
        .and_then(Value::as_str)
        .or_else(|| raw.get("isrc").and_then(Value::as_str))
        .or_else(|| song_metadata(raw, "ISRC"))
        .map(|value| value.trim().to_ascii_uppercase())
        .filter(|value| !value.is_empty());
    let cover_url = track
        .and_then(|track| track.pointer("/images/coverarthq"))
        .and_then(Value::as_str)
        .or_else(|| {
            track
                .and_then(|track| track.pointer("/images/coverart"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            track
                .and_then(|track| track.pointer("/share/image"))
                .and_then(Value::as_str)
        })
        .map(str::to_owned);
    let track_key = track
        .and_then(|track| track.get("key"))
        .and_then(Value::as_str)
        .or_else(|| value.get("track_key").and_then(Value::as_str));

    Some(Candidate {
        provider: "songrec".into(),
        title: title.to_owned(),
        artist: artist.to_owned(),
        album,
        year: released.as_deref().and_then(extract_year),
        release_date: released,
        genre,
        label,
        isrc,
        cover_url,
        score: 96.0,
        score_breakdown: Some(
            serde_json::json!({
                "audio_recognition": true,
                "source": "songrec_shazam_recognition",
                "sources": ["SongRec / Shazam"],
                "shazam_track_key": track_key,
                "final_score": 96.0
            })
            .to_string(),
        ),
        raw_json: raw.to_string(),
        ..Default::default()
    })
}

fn extract_year(value: &str) -> Option<String> {
    value
        .split(|character: char| !character.is_ascii_digit())
        .find(|part| part.len() == 4 && (part.starts_with('1') || part.starts_with('2')))
        .map(str::to_owned)
}

fn song_metadata<'a>(raw: &'a Value, wanted: &str) -> Option<&'a str> {
    raw.pointer("/track/sections")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|section| {
            section
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.eq_ignore_ascii_case("SONG"))
        })
        .filter_map(|section| section.get("metadata").and_then(Value::as_array))
        .flatten()
        .find(|item| {
            item.get("title")
                .and_then(Value::as_str)
                .is_some_and(|title| title.eq_ignore_ascii_case(wanted))
        })
        .and_then(|item| item.get("text"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_response() -> Value {
        serde_json::json!({
            "matches": [{"id": "match"}],
            "track": {
                "key": "12345",
                "title": "Tehran Kenaret",
                "subtitle": "Saaren",
                "isrc": "IRMZ12345678",
                "genres": {"primary": "Pop"},
                "images": {"coverarthq": "https://example.test/cover.jpg"},
                "sections": [{
                    "type": "SONG",
                    "metadata": [
                        {"title": "Album", "text": "Tehran Kenaret"},
                        {"title": "Label", "text": "Radio Javan"},
                        {"title": "Released", "text": "2021"}
                    ]
                }]
            }
        })
    }

    #[test]
    fn parses_raw_songrec_response() {
        let candidate = parse_result(&raw_response()).unwrap();
        assert_eq!(candidate.title, "Tehran Kenaret");
        assert_eq!(candidate.artist, "Saaren");
        assert_eq!(candidate.album.as_deref(), Some("Tehran Kenaret"));
        assert_eq!(candidate.label.as_deref(), Some("Radio Javan"));
        assert_eq!(candidate.year.as_deref(), Some("2021"));
        assert_eq!(candidate.isrc.as_deref(), Some("IRMZ12345678"));
        assert_eq!(candidate.provider, "songrec");
    }

    #[test]
    fn parses_songrec_lib_wrapper() {
        let wrapped = serde_json::json!({
            "song_name": "Fallback title",
            "artist_name": "Fallback artist",
            "album_name": "Fallback album",
            "release_year": "2020",
            "genre": "Rock",
            "track_key": "key",
            "raw_response": raw_response()
        });
        let candidate = parse_result(&wrapped).unwrap();
        assert_eq!(candidate.title, "Tehran Kenaret");
        assert_eq!(candidate.genre.as_deref(), Some("Pop"));
        assert!(candidate.raw_json.contains("coverarthq"));
    }

    #[test]
    fn ignores_no_match_response() {
        assert!(parse_result(&serde_json::json!({"matches": []})).is_none());
        assert!(is_no_match("Error: No track found in response"));
    }

    #[test]
    fn extracts_json_surrounded_by_cli_noise() {
        let value = parse_json_output(b"starting\n{\"track\":{\"title\":\"Song\"}}\ndone").unwrap();
        assert_eq!(value["track"]["title"], "Song");
    }
}
