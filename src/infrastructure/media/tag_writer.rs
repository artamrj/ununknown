use crate::infrastructure::{media::replaygain::ReplayGain, providers::Candidate};
use anyhow::{Result, bail};
use lofty::{
    config::WriteOptions,
    file::TaggedFileExt,
    picture::{Picture, PictureType},
    prelude::*,
    probe::Probe,
    tag::{ItemKey, Tag},
};
use std::path::Path;

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
    let mut file = Probe::open(path)?.read()?;
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
    let file = Probe::open(path)?.read()?;
    Ok(valid_embedded_artwork(&file))
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
}
