# Setup And Use After Deployment

## Version 0.3 Pipeline

Ununknown 0.3 discovers all files first, then fetches metadata for exactly one file at a time. The workspace moves automatically through Scan, Fetch, Preview, Apply, and Finish.

Files are never saved to SQLite while they are being discovered or identified. Unmatched and failed details are discarded after their totals are updated. Only successful matched proposals are kept temporarily so Preview can survive a browser or backend restart.

## Complete Settings

The Settings page exposes Basic, Matching, Metadata, Files and Paths, and Expert tabs. Use settings search to find any supported control. Expert Mode is required before enabling in-place writes, renaming, folder reorganization, overwrite collisions, or existing-cover replacement.

Matching supports safe, aggressive, manual, and custom confidence modes. Files and Paths supports output, compilation, and in-place filename templates with live examples plus fallbacks, padding, collision behavior, and filename limits. Metadata fields are grouped so core tags, numbering, release info, IDs, and extras can be enabled independently.

## First Setup

Open <http://localhost:7331> and select **Settings**.

1. Keep `/music/input` and `/music/output` unless you deliberately mounted different container paths.
2. Start with **Copy to output** and **Safe** matching.
3. Configure provider environment variables in Docker Compose, then restart the container.
4. Save Settings and start with copied music files.

Non-provider settings and unfinished workflow data are saved in SQLite. AcoustID and MusicBrainz configuration are supplied through Docker Compose and are never saved in SQLite.

## MusicBrainz

MusicBrainz’s public API does not need an API key. It requires a meaningful User-Agent/contact so its administrators can contact an application owner if necessary.

Ununknown limits MusicBrainz requests to one request per second. If MusicBrainz rejects the contact or cannot be reached, the affected track shows the provider error.

## AcoustID

Create an application API key at <https://acoustid.org/api-key>. This is an application key, not an account password.

Set `UNUNKNOWN_ACOUSTID_API_KEY` in the Docker Compose environment, then restart the container.

Set the MusicBrainz contact through `UNUNKNOWN_MUSICBRAINZ_USER_AGENT`, then restart the container. MusicBrainz does not use an API key.

AcoustID is optional but produces more reliable fingerprint matches. If it returns no match, Ununknown searches MusicBrainz using existing title and artist tags. Text-search matches are capped below auto-selection thresholds and must be reviewed.

## Repair Music

1. Put copied audio in the mounted input folder.
2. Click **Scan music**.
3. Watch Fetch process each file sequentially.
4. Preview the successful matched proposals and duplicate skips.
5. Check destination paths, actions, and warnings.
6. Confirm **Apply changes**.

Scanning and previewing never modify audio. Apply requires a fresh successful preview.

## Matching Modes

- **Safe:** selects strong fingerprint matches scoring 90 or higher.
- **Aggressive:** selects fingerprint matches scoring 75 or higher.
- **Manual:** selects nothing automatically.

Results below the selected confidence threshold are counted as unmatched and are not saved.

## Duplicate Handling

After Fetch, Ununknown groups duplicate matched recordings by MusicBrainz recording ID or ISRC first. If those IDs are missing, it uses a conservative artist/title/duration fallback.

The Preview keeps the best duplicate match and skips the others. Skipped duplicates are not deleted and are not modified; they are only excluded from output/apply writes.

## When No Match Appears

1. Check the track row for a provider error.
2. Confirm the MusicBrainz contact environment variable contains an email address or website.
3. Confirm the AcoustID key is set in Docker Compose if you want fingerprint lookup.
4. Check that `fpcalc` runs in the container: `docker compose exec ununknown fpcalc -version`.
5. Try files with useful existing title/artist tags. Text fallback cannot search a track with no title.
6. Inspect logs with `docker compose logs -f`.

Not every recording exists in AcoustID or MusicBrainz. Unknown tracks can be skipped safely.

## Writing Safety

Use copy mode first. MP3, FLAC, M4A, OGG, and Opus are primary write formats. WAV and AIFF writes are skipped when unsafe. Ununknown does not provide backups, rollback, or transcoding.
