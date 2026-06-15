use crate::{config::Config, providers::Candidate};
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
    cfg: &Config,
    artwork: Option<Vec<u8>>,
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
    if file.primary_tag().is_none() {
        file.insert_tag(Tag::new(file.primary_tag_type()));
    }
    let tag = file.primary_tag_mut().expect("primary tag inserted");
    let f = &cfg.metadata_fields;
    let overwrite = cfg.overwrite_existing_tags;
    set(
        tag,
        ItemKey::TrackTitle,
        &candidate.title,
        f.title,
        overwrite,
    );
    set(
        tag,
        ItemKey::TrackArtist,
        &candidate.artist,
        f.artist,
        overwrite,
    );
    optional(
        tag,
        ItemKey::AlbumTitle,
        &candidate.album,
        f.album,
        overwrite,
    );
    optional(
        tag,
        ItemKey::AlbumArtist,
        &candidate.album_artist,
        f.album_artist,
        overwrite,
    );
    optional(tag, ItemKey::Isrc, &candidate.isrc, f.isrc, overwrite);
    optional(tag, ItemKey::Genre, &candidate.genre, f.genre, overwrite);
    optional(
        tag,
        ItemKey::Composer,
        &candidate.composer,
        f.composer,
        overwrite,
    );
    optional(tag, ItemKey::Label, &candidate.label, f.label, overwrite);
    optional(
        tag,
        ItemKey::RecordingDate,
        &candidate.year,
        f.release_date,
        overwrite,
    );
    if f.comment {
        optional(
            tag,
            ItemKey::Comment,
            &Some("Matched by Ununknown".into()),
            true,
            overwrite,
        );
    }
    optional(
        tag,
        ItemKey::MusicBrainzRecordingId,
        &candidate.recording_id,
        f.musicbrainz_recording_id,
        overwrite,
    );
    optional(
        tag,
        ItemKey::MusicBrainzReleaseId,
        &candidate.release_id,
        f.musicbrainz_release_id,
        overwrite,
    );
    optional(
        tag,
        ItemKey::MusicBrainzArtistId,
        &candidate.artist_id,
        f.musicbrainz_artist_id,
        overwrite,
    );
    optional(
        tag,
        ItemKey::MusicBrainzReleaseArtistId,
        &candidate.album_artist_id,
        f.musicbrainz_album_artist_id,
        overwrite,
    );
    if f.track_number
        && (overwrite || tag.track().is_none())
        && let Some(v) = candidate.track_number
    {
        tag.set_track(v as u32);
    }
    if f.track_total
        && (overwrite || tag.track_total().is_none())
        && let Some(v) = candidate.track_total
    {
        tag.set_track_total(v as u32);
    }
    if f.disc_number
        && (overwrite || tag.disk().is_none())
        && let Some(v) = candidate.disc_number
    {
        tag.set_disk(v as u32);
    }
    if f.disc_total
        && (overwrite || tag.disk_total().is_none())
        && let Some(v) = candidate.disc_total
    {
        tag.set_disk_total(v as u32);
    }
    if f.embed_cover_art
        && let Some(data) = artwork
    {
        if f.replace_existing_cover_art {
            tag.remove_picture_type(PictureType::CoverFront);
        }
        if f.replace_existing_cover_art || tag.pictures().is_empty() {
            tag.push_picture(
                Picture::unchecked(data)
                    .pic_type(PictureType::CoverFront)
                    .build(),
            );
        }
    }
    file.save_to_path(path, WriteOptions::default())?;
    Ok(())
}
fn set(tag: &mut Tag, key: ItemKey, value: &str, enabled: bool, overwrite: bool) {
    if enabled && (overwrite || tag.get_string(key).is_none()) {
        tag.insert_text(key, value.into());
    }
}
fn optional(tag: &mut Tag, key: ItemKey, value: &Option<String>, enabled: bool, overwrite: bool) {
    if let Some(value) = value {
        set(tag, key, value, enabled, overwrite);
    }
}
