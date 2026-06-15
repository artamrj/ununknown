# Setup And Use After Deployment

## Version 0.2 Workspace

Ununknown 0.2 shows progress on each file instead of using one global progress bar. Automatic modes hide candidate controls for confident matches; review rows expose candidate choices. Successfully applied files are removed from the temporary workspace database and can be rediscovered by a later scan.

## Complete Settings

The Settings page exposes Basic, Matching, Metadata, Files and Paths, Providers, and Expert tabs. Use settings search to find any supported control. Expert Mode is required before enabling in-place writes, renaming, folder reorganization, overwrite collisions, or existing-cover replacement.

Matching supports safe, aggressive, manual, and custom confidence modes. Files and Paths supports output, compilation, and in-place filename templates plus fallbacks, padding, collision behavior, and filename limits. Metadata allows every supported field to be enabled independently.

## First Setup

Open <http://localhost:7331> and select **Settings**.

1. Keep `/music/input` and `/music/output` unless you deliberately mounted different container paths.
2. Start with **Copy to output** and **Safe** matching.
3. Enter a MusicBrainz contact, for example `Ununknown/0.1 (you@example.com)`.
4. Save Settings, then use **Test MusicBrainz**.
5. Optionally enter an AcoustID application key, save Settings, then use **Test AcoustID**.

Non-provider settings and unfinished workflow data are saved in SQLite. AcoustID and MusicBrainz configuration are supplied through Docker Compose and are never saved in SQLite.

## MusicBrainz

MusicBrainz’s public API does not need an API key. It requires a meaningful User-Agent/contact so its administrators can contact an application owner if necessary.

Ununknown limits MusicBrainz requests to one request per second. If MusicBrainz rejects the contact or cannot be reached, the affected track shows the provider error.

## AcoustID

Create an application API key at <https://acoustid.org/api-key>. This is an application key, not an account password.

Set `UNUNKNOWN_ACOUSTID_API_KEY` in the Docker Compose environment, restart the container, then click **Test AcoustID** in Settings.

Set the MusicBrainz contact through `UNUNKNOWN_MUSICBRAINZ_USER_AGENT`, restart the container, then click **Test MusicBrainz**. MusicBrainz does not use an API key.

AcoustID is optional but produces more reliable fingerprint matches. If it returns no match, Ununknown searches MusicBrainz using existing title and artist tags. Text-search matches are capped below auto-selection thresholds and must be reviewed.

## Repair Music

1. Put copied audio in the mounted input folder.
2. Click **Scan music**.
3. Review each proposed match. Tracks with provider problems show the exact error.
4. Select or skip uncertain candidates.
5. Click **Preview changes**.
6. Check destination paths, actions, and warnings.
7. Confirm **Apply changes**.

Scanning and previewing never modify audio. Apply requires a fresh successful preview.

## Matching Modes

- **Safe:** selects strong fingerprint matches scoring 90 or higher.
- **Aggressive:** selects fingerprint matches scoring 75 or higher.
- **Manual:** selects nothing automatically.

MusicBrainz text-search fallback results always require review.

## When No Match Appears

1. Check the track row for a provider error.
2. Test both providers in Settings.
3. Confirm the MusicBrainz contact contains an email address or website.
4. Confirm the AcoustID key was saved before testing.
5. Check that `fpcalc` runs in the container: `docker compose exec ununknown fpcalc -version`.
6. Try files with useful existing title/artist tags. Text fallback cannot search a track with no title.
7. Inspect logs with `docker compose logs -f`.

Not every recording exists in AcoustID or MusicBrainz. Unknown tracks can be skipped safely.

## Writing Safety

Use copy mode first. MP3, FLAC, M4A, OGG, and Opus are primary write formats. WAV and AIFF writes are skipped when unsafe. Ununknown does not provide backups, rollback, or transcoding.
