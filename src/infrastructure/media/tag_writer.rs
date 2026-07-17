use crate::infrastructure::{media::replaygain::ReplayGain, providers::Candidate};
use anyhow::{Context, Result, bail};
use lofty::{
    config::WriteOptions,
    file::TaggedFileExt,
    picture::{Picture, PictureType},
    prelude::*,
    probe::Probe,
    tag::{ItemKey, Tag},
};
use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
};

pub fn write(
    path: &Path,
    candidate: &Candidate,
    artwork: Option<Vec<u8>>,
    replay_gain: Option<ReplayGain>,
) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(ext.as_str(), "wav" | "aiff" | "aif") {
        bail!("{ext} writing skipped because it is conditional/unsafe in this MVP");
    }
    let mut file = Probe::open(path)?.guess_file_type()?.read()?;
    let preserved_artwork = valid_embedded_artwork(&file);
    // Rebuild the primary tag instead of mutating it. Real music collections often
    // contain malformed frames that can be read but cannot be saved again.
    file.insert_tag(Tag::new(file.primary_tag_type()));
    let tag = file.primary_tag_mut().expect("primary tag inserted");
    let credits = crate::domain::credits::normalize_featured(&candidate.artist, &candidate.title);
    set(tag, ItemKey::TrackTitle, &credits.title);
    set(tag, ItemKey::TrackArtist, &credits.artist);
    optional(tag, ItemKey::AlbumTitle, &candidate.album);
    optional(tag, ItemKey::AlbumArtist, &candidate.album_artist);
    optional(tag, ItemKey::Isrc, &candidate.isrc);
    optional(tag, ItemKey::Genre, &candidate.genre);
    if let Some(language_code) = candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| value["genre"]["language_code"].as_str().map(str::to_owned))
    {
        set(tag, ItemKey::Language, &language_code);
    }
    optional(tag, ItemKey::Composer, &candidate.composer);
    optional(tag, ItemKey::Label, &candidate.label);
    optional(tag, ItemKey::RecordingDate, &candidate.year);
    if let Some(replay_gain) = replay_gain {
        write_replaygain(tag, replay_gain);
    }
    optional(
        tag,
        ItemKey::MusicBrainzRecordingId,
        &candidate.recording_id,
    );
    optional(tag, ItemKey::MusicBrainzReleaseId, &candidate.release_id);
    optional(tag, ItemKey::MusicBrainzArtistId, &candidate.artist_id);
    optional(
        tag,
        ItemKey::MusicBrainzReleaseArtistId,
        &candidate.album_artist_id,
    );
    if let Some(v) = candidate.track_number {
        tag.set_track(v as u32);
    }
    if let Some(v) = candidate.track_total {
        tag.set_track_total(v as u32);
    }
    if let Some(v) = candidate.disc_number {
        tag.set_disk(v as u32);
    }
    if let Some(v) = candidate.disc_total {
        tag.set_disk_total(v as u32);
    }
    if let Some(data) = artwork.or(preserved_artwork) {
        let mut picture = picture_from_bytes(data)?;
        picture.set_pic_type(PictureType::CoverFront);
        tag.push_picture(picture);
    }
    file.save_to_path(path, WriteOptions::default())?;
    Ok(())
}

pub fn write_resilient(
    path: &Path,
    candidate: &Candidate,
    artwork: Option<Vec<u8>>,
    replay_gain: Option<ReplayGain>,
) -> Result<bool> {
    match write(path, candidate, artwork.clone(), replay_gain) {
        Ok(()) => Ok(false),
        Err(initial_error) => {
            sanitize_legacy_metadata(path).with_context(|| {
                format!("initial tag read failed ({initial_error}); lossless tag cleanup failed")
            })?;
            write(path, candidate, artwork, replay_gain).with_context(|| {
                format!("tag cleanup succeeded, but writing still failed ({initial_error})")
            })?;
            Ok(true)
        }
    }
}

fn sanitize_legacy_metadata(path: &Path) -> Result<()> {
    let extension = detected_container_extension(path).unwrap_or_else(|| {
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("audio")
            .to_ascii_lowercase()
    });
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("audio");
    let repaired = path.with_file_name(format!(
        ".{stem}.tag-clean-{}.{}",
        std::process::id(),
        extension
    ));
    let _ = std::fs::remove_file(&repaired);
    let output = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-fflags",
            "+discardcorrupt",
            "-i",
        ])
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-c:a",
            "copy",
            "-map_metadata",
            "-1",
            "-map_chapters",
            "-1",
            "-y",
        ])
        .arg(&repaired)
        .output()
        .context("could not start FFmpeg for lossless tag cleanup")?;
    if !output.status.success() || !repaired.is_file() {
        let _ = std::fs::remove_file(&repaired);
        bail!(
            "FFmpeg could not remove malformed metadata: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    std::fs::rename(&repaired, path).with_context(|| {
        format!(
            "could not replace temporary copy with sanitized audio {}",
            path.display()
        )
    })?;
    Ok(())
}

fn detected_container_extension(path: &Path) -> Option<String> {
    let output = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=format_name",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .ok()?;
    let format = String::from_utf8_lossy(&output.stdout);
    let names = format.trim().split(',').collect::<Vec<_>>();
    if names
        .iter()
        .any(|name| matches!(*name, "mov" | "mp4" | "m4a" | "3gp" | "3g2" | "mj2"))
    {
        Some("m4a".into())
    } else if names.contains(&"mp3") {
        Some("mp3".into())
    } else if names.contains(&"flac") {
        Some("flac".into())
    } else if names.contains(&"ogg") {
        Some("ogg".into())
    } else {
        None
    }
}

fn valid_embedded_artwork(file: &lofty::file::TaggedFile) -> Option<Vec<u8>> {
    let valid_picture = |tag: &Tag| {
        tag.pictures()
            .iter()
            .find(|picture| {
                matches!(
                    picture.pic_type(),
                    PictureType::CoverFront | PictureType::Other
                ) && validate_artwork(picture.data()).is_ok()
            })
            .map(|picture| picture.data().to_vec())
    };
    file.primary_tag().and_then(valid_picture).or_else(|| {
        file.tags()
            .iter()
            .filter(|tag| Some(tag.tag_type()) != file.primary_tag().map(Tag::tag_type))
            .find_map(valid_picture)
    })
}

pub fn read_artwork(path: &Path) -> Result<Option<Vec<u8>>> {
    let file = Probe::open(path)?.guess_file_type()?.read()?;
    Ok(valid_embedded_artwork(&file))
}

/// Verify the on-disk representation players actually consume. FLAC artwork is
/// required to be a native PICTURE metadata block; reading it back through the
/// same generic tag abstraction could otherwise hide a format-specific write bug.
pub fn verify_embedded_artwork(path: &Path, expected: &[u8]) -> Result<()> {
    let file = Probe::open(path)?.guess_file_type()?.read()?;
    if file.file_type() == lofty::file::FileType::Flac {
        let embedded = native_flac_front_cover(path)?
            .ok_or_else(|| anyhow::anyhow!("FLAC contains no native front-cover PICTURE block"))?;
        if embedded != expected {
            bail!("native FLAC cover does not match the validated preview image");
        }
        return Ok(());
    }
    let embedded = valid_embedded_artwork(&file)
        .ok_or_else(|| anyhow::anyhow!("cover verification found no embedded image"))?;
    if embedded != expected {
        bail!("embedded cover does not match the validated preview image");
    }
    Ok(())
}

fn native_flac_front_cover(path: &Path) -> Result<Option<Vec<u8>>> {
    let mut file = std::fs::File::open(path)?;
    let mut prefix = [0_u8; 10];
    file.read_exact(&mut prefix)?;
    if &prefix[..3] == b"ID3" {
        if prefix[6..10].iter().any(|byte| byte & 0x80 != 0) {
            bail!("invalid ID3 size before FLAC stream");
        }
        let id3_size = prefix[6..10]
            .iter()
            .fold(0_u64, |size, byte| (size << 7) | u64::from(*byte));
        let footer_size = if prefix[5] & 0x10 != 0 { 10 } else { 0 };
        file.seek(SeekFrom::Start(10 + id3_size + footer_size))?;
    } else {
        file.seek(SeekFrom::Start(0))?;
    }
    let mut magic = [0_u8; 4];
    file.read_exact(&mut magic)?;
    if &magic != b"fLaC" {
        bail!("file does not contain a FLAC stream");
    }

    loop {
        let mut header = [0_u8; 4];
        file.read_exact(&mut header)?;
        let is_last = header[0] & 0x80 != 0;
        let block_type = header[0] & 0x7f;
        let length =
            (u32::from(header[1]) << 16) | (u32::from(header[2]) << 8) | u32::from(header[3]);
        if block_type == 6 {
            if length > 20 * 1024 * 1024 + 1024 {
                bail!("FLAC PICTURE block exceeds the artwork safety limit");
            }
            let mut block = vec![0_u8; length as usize];
            file.read_exact(&mut block)?;
            if let Some((picture_type, data)) = parse_flac_picture_block(&block)?
                && picture_type == 3
            {
                return Ok(Some(data.to_vec()));
            }
        } else {
            file.seek(SeekFrom::Current(i64::from(length)))?;
        }
        if is_last {
            return Ok(None);
        }
    }
}

fn parse_flac_picture_block(block: &[u8]) -> Result<Option<(u32, &[u8])>> {
    let mut position = 0;
    let picture_type = take_be_u32(block, &mut position)?;
    let mime_length = take_be_u32(block, &mut position)? as usize;
    skip_bytes(block, &mut position, mime_length)?;
    let description_length = take_be_u32(block, &mut position)? as usize;
    skip_bytes(block, &mut position, description_length)?;
    // Width, height, color depth, and indexed-color count.
    skip_bytes(block, &mut position, 16)?;
    let data_length = take_be_u32(block, &mut position)? as usize;
    let end = position
        .checked_add(data_length)
        .ok_or_else(|| anyhow::anyhow!("FLAC PICTURE data length overflow"))?;
    let data = block
        .get(position..end)
        .ok_or_else(|| anyhow::anyhow!("truncated FLAC PICTURE data"))?;
    Ok(Some((picture_type, data)))
}

fn take_be_u32(data: &[u8], position: &mut usize) -> Result<u32> {
    let end = position
        .checked_add(4)
        .ok_or_else(|| anyhow::anyhow!("FLAC PICTURE field offset overflow"))?;
    let bytes: [u8; 4] = data
        .get(*position..end)
        .ok_or_else(|| anyhow::anyhow!("truncated FLAC PICTURE field"))?
        .try_into()?;
    *position = end;
    Ok(u32::from_be_bytes(bytes))
}

fn skip_bytes(data: &[u8], position: &mut usize, count: usize) -> Result<()> {
    let end = position
        .checked_add(count)
        .ok_or_else(|| anyhow::anyhow!("FLAC PICTURE field length overflow"))?;
    if end > data.len() {
        bail!("truncated FLAC PICTURE field");
    }
    *position = end;
    Ok(())
}

fn write_replaygain(tag: &mut Tag, replay_gain: ReplayGain) {
    set(tag, ItemKey::ReplayGainTrackGain, &replay_gain.gain_tag());
    set(tag, ItemKey::ReplayGainTrackPeak, &replay_gain.peak_tag());
}

fn picture_from_bytes(data: Vec<u8>) -> Result<Picture> {
    let mut reader = std::io::Cursor::new(data);
    Ok(Picture::from_reader(&mut reader)?)
}

pub fn validate_artwork(data: &[u8]) -> Result<()> {
    if data.len() > 20 * 1024 * 1024 {
        bail!("cover image exceeds the 20 MB safety limit");
    }
    picture_from_bytes(data.to_vec()).map(|_| ())
}
fn set(tag: &mut Tag, key: ItemKey, value: &str) {
    tag.insert_text(key, value.into());
}
fn optional(tag: &mut Tag, key: ItemKey, value: &Option<String>) {
    if let Some(value) = value {
        set(tag, key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use lofty::picture::MimeType;

    fn one_pixel_png() -> Vec<u8> {
        STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
            .unwrap()
    }

    #[test]
    fn cover_image_type_is_detected_before_embedding() {
        let png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0];
        let picture = picture_from_bytes(png).unwrap();
        assert_eq!(picture.mime_type(), Some(&MimeType::Png));
    }

    #[test]
    fn invalid_cover_data_is_rejected() {
        assert!(picture_from_bytes(vec![0; 12]).is_err());
    }

    #[test]
    fn existing_valid_cover_survives_when_catalog_has_no_artwork() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preserve-cover.mp3");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=0.1")
            .args(["-q:a", "9"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let candidate = Candidate {
            title: "Song".into(),
            artist: "Artist".into(),
            ..Default::default()
        };
        let artwork = one_pixel_png();
        write(&path, &candidate, Some(artwork.clone()), None).unwrap();
        write(&path, &candidate, None, None).unwrap();

        let file = Probe::open(&path).unwrap().read().unwrap();
        assert_eq!(file.primary_tag().unwrap().pictures()[0].data(), artwork);
    }

    #[test]
    fn flac_cover_is_a_native_picture_block_after_disk_round_trip() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native-cover.flac");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=0.1")
            .args(["-c:a", "flac"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());

        let candidate = Candidate {
            title: "FLAC artwork test".into(),
            artist: "Test artist".into(),
            ..Default::default()
        };
        let artwork = one_pixel_png();
        write(&path, &candidate, Some(artwork.clone()), None).unwrap();

        assert_eq!(
            native_flac_front_cover(&path).unwrap().as_deref(),
            Some(artwork.as_slice())
        );
        verify_embedded_artwork(&path, &artwork).unwrap();
    }

    #[test]
    fn replaygain_uses_portable_item_keys() {
        let mut tag = Tag::new(lofty::tag::TagType::Id3v2);
        write_replaygain(
            &mut tag,
            ReplayGain {
                track_gain_db: -7.235,
                track_peak: 0.987_654_3,
            },
        );
        assert_eq!(
            tag.get_string(ItemKey::ReplayGainTrackGain),
            Some("-7.24 dB")
        );
        assert_eq!(
            tag.get_string(ItemKey::ReplayGainTrackPeak),
            Some("0.987654")
        );
    }

    #[test]
    fn replaygain_survives_an_mp3_disk_round_trip() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("round-trip.mp3");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=0.1")
            .args(["-q:a", "9"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());

        let candidate = Candidate {
            title: "Round trip".into(),
            artist: "Test artist".into(),
            ..Candidate::default()
        };
        write(
            &path,
            &candidate,
            None,
            Some(ReplayGain {
                track_gain_db: -6.25,
                track_peak: 0.75,
            }),
        )
        .unwrap();

        let file = Probe::open(&path).unwrap().read().unwrap();
        let tag = file.primary_tag().unwrap();
        assert_eq!(
            tag.get_string(ItemKey::ReplayGainTrackGain),
            Some("-6.25 dB")
        );
        assert_eq!(
            tag.get_string(ItemKey::ReplayGainTrackPeak),
            Some("0.750000")
        );
    }

    #[test]
    fn detects_mp4_container_hidden_behind_mp3_extension() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let m4a = directory.path().join("actual.m4a");
        let disguised = directory.path().join("wrong.mp3");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=0.1")
            .args(["-c:a", "aac"])
            .arg(&m4a)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::rename(m4a, &disguised).unwrap();

        assert_eq!(
            detected_container_extension(&disguised).as_deref(),
            Some("m4a")
        );
        let file = Probe::open(&disguised)
            .unwrap()
            .guess_file_type()
            .unwrap()
            .read()
            .unwrap();
        assert_eq!(file.file_type(), lofty::file::FileType::Mp4);
    }
}
