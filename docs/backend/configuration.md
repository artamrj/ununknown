# Backend Configuration

Configuration comes from two places:

1. Environment variables used at process startup.
2. Saved editable settings stored as JSON in the SQLite `settings` table.

The startup environment values are treated as deployment-owned values. Saved UI
settings are loaded from SQLite and merged with those deployment values.

## Environment Variables

| Variable | Purpose | Default |
| --- | --- | --- |
| `UNUNKNOWN_DB` | SQLite database path | `/cache/ununknown.sqlite` |
| `UNUNKNOWN_INPUT_DIR` | Folder scanned for music | `/music/input` |
| `UNUNKNOWN_OUTPUT_DIR` | Copy-mode output folder | `/music/output` |
| `UNUNKNOWN_ACOUSTID_API_KEY` | Optional AcoustID API key | empty |
| `UNUNKNOWN_MUSICBRAINZ_USER_AGENT` | MusicBrainz contact user agent | `Ununknown/0.5.0 (https://github.com/artamrj/ununknown)` |

MusicBrainz requires a useful user agent. The backend validation expects a
product/version plus contact details in parentheses, with either an email
address or a website.

Example:

```text
Ununknown/0.5.0 (https://github.com/artamrj/ununknown)
```

## Saved Settings

`src/config.rs` defines the `Config` struct. Editable settings include:

- Paths and mode: `input_dir`, `output_dir`, `output_mode`.
- Matching: `automation_mode`, `confidence_threshold`, `track_attempts`.
- Worker limits: scan, metadata, fingerprint, AcoustID, artwork, tag write, and
  DB batch sizes.
- Metadata behavior: cover art, tag overwrite, selected metadata fields.
- Safety: `expert_mode`.
- Retention: workspace and job cleanup windows.
- File naming: `path_templates` and `in_place`.

Secrets are not returned as normal settings. `Config::public()` clears the
AcoustID key and returns booleans that say whether AcoustID and MusicBrainz are
configured.

## Defaults

Important defaults:

- `output_mode`: `copy`
- `automation_mode`: `safe`
- `confidence_threshold`: `90.0`
- `track_attempts`: `3`
- `scan_worker_concurrency`: `8`
- `metadata_read_concurrency`: `6`
- `fingerprint_concurrency`: `3`
- `acoustid_concurrency`: `3`
- `artwork_download_concurrency`: `3`
- `tag_write_concurrency`: `2`
- `db_write_batch_size`: `25`
- `cover_art_enabled`: `true`
- `overwrite_existing_tags`: `true`
- `expert_mode`: `false`

Default output templates:

```text
$albumartist/$album/$track - $title
Compilations/$album/$track - $artist - $title
```

Default in-place filename template:

```text
$track - $title
```

## Validation Rules

`Config::validate()` blocks invalid or unsafe settings before they are saved.
The main rules are:

- Input folder is required.
- Output folder is required in copy mode.
- Confidence threshold must be between `0` and `100`.
- Track attempts must be between `1` and `10`.
- Worker and batch limits must stay in documented ranges.
- MusicBrainz user agent must include contact details.
- Output templates cannot be empty.
- Number padding cannot exceed `8`.
- Filename limit must be between `32` and `255`.
- Workspace and job retention must be between `1` and `365` days.

## Expert Mode Safety

Some settings can overwrite, rename, or modify existing files. They require
`expert_mode = true`:

- `output_mode = in_place`
- in-place file or folder renaming
- collision strategy `overwrite`
- replacing existing cover art

Copy mode is safer because the backend copies files into the output folder
before writing tags. In-place mode edits the source files directly.

## Path Templates

Path templates are rendered by `src/domain/path_templates.rs`.

Supported variables include:

```text
$artist $albumartist $album $title $track $tracktotal $disc $disctotal
$year $genre $composer $isrc $label $format $bitrate $ext
```

The renderer:

- Replaces missing artist, album, and title with configured unknown labels.
- Pads track and disc numbers.
- Sanitizes illegal filename characters.
- Blocks absolute paths and `..` traversal.
- Preserves or adds the original extension.
- Resolves destination collisions according to `skip`, `overwrite`, or
  `rename`.
