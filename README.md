# Ununknown

Ununknown has one job: correct metadata for a folder of music.

It first checks that each audio stream can be decoded, then tries every configured
automatic source (audio fingerprinting, SongRec/Shazam, MusicBrainz, Apple Music, Deezer, Radio Javan, Navahang, Audiomack, Genius,
Spotify, Discogs, Last.fm, TheAudioDB, Wikidata, YouTube, and Cover Art Archive). Difficult
tracks use SongRec when installed and can optionally use AudD audio recognition. Reliable matches are selected
automatically. Uncertain and unmatched files stay in a short review list where you
can choose a candidate or enter metadata manually. Corrected files are first written
as separate copies with ReplayGain metadata. Damaged audio is reported and blocked
from the write queue.

An optional **Remove input after successful output** setting can turn the copy
workflow into a move-like workflow. It is disabled by default and removes each
source only after its corrected destination has been written successfully.

## Run locally

Requirements: Rust and Node.js. FFmpeg enables integrity checking and ReplayGain;
Chromaprint is recommended for fingerprint matching (`brew install ffmpeg
chromaprint` on macOS). Tag, filename, and web lookup still work without them.

```bash
./dev.sh
```

Open <http://localhost:5173>. Frontend and backend changes reload automatically.

The launcher stores temporary data under `.local/`:

- `.local/input` — default music folder
- `.local/output` — corrected copies
- `.local/cache` — SQLite and fingerprint/provider caches

## Production build

Production is a single-user local deployment. The optimized Rust server serves both the API and the
built React application on `127.0.0.1:7331`; it refuses non-loopback bind addresses because the API
has intentional access to local media paths.

Create a versioned release archive:

```bash
./scripts/build-release.sh
```

Extract the archive from `dist/`, then run `bin/ununknown-run`. Application data is stored under
`~/Library/Application Support/Ununknown` on macOS or
`${XDG_STATE_HOME:-~/.local/state}/ununknown` on Linux. Override that location with
`UNUNKNOWN_DATA_DIR`. FFmpeg and Chromaprint remain recommended runtime dependencies.

The server supports `UNUNKNOWN_DB`, `UNUNKNOWN_INPUT_DIR`, `UNUNKNOWN_OUTPUT_DIR`,
`UNUNKNOWN_REFERENCE_DIRS` (an optional OS path-list), `UNUNKNOWN_STATIC_DIR`, and a loopback-only
`UNUNKNOWN_BIND`. Provider credentials can be supplied
without persisting them in SQLite through `UNUNKNOWN_ACOUSTID_KEY`, `UNUNKNOWN_AUDD_TOKEN`,
`UNUNKNOWN_SPOTIFY_CLIENT_ID`, `UNUNKNOWN_SPOTIFY_CLIENT_SECRET`,
`UNUNKNOWN_SOUNDCLOUD_CLIENT_ID`, `UNUNKNOWN_SOUNDCLOUD_CLIENT_SECRET`,
`UNUNKNOWN_YOUTUBE_API_KEY`, `UNUNKNOWN_DISCOGS_TOKEN`, `UNUNKNOWN_LASTFM_KEY`, and
`UNUNKNOWN_THEAUDIODB_KEY`.

### Docker Compose and GHCR

The included Compose deployment pulls `ghcr.io/artamrj/ununknown:latest`, runs the application as a
non-root user, uses a read-only container filesystem, and publishes the application only on
`127.0.0.1:7331`. Prepare it with:

```bash
cp .env.example .env
chmod 600 .env
# Point UNUNKNOWN_INPUT_PATH at an existing music library. On Linux, set PUID
# and PGID in .env to the output of `id -u` and `id -g`.
docker compose pull
docker compose up -d
docker compose ps
```

Then open <http://127.0.0.1:7331>. Put input files in `music/`; corrected copies are written to
`output/`; existing music for duplicate checks goes in `reference/`; SQLite and caches are stored
in `data/`. The `/data/reference` folder is detected automatically and its mount is read-only, so
no extra container environment setting is needed. On startup, the container creates the writable
mounts when necessary, assigns them to `PUID:PGID`, and drops root privileges before starting the
application. These locations, the image tag, port, log level, and optional provider credentials are
documented in `.env.example`. The input mount is read-only, so **Remove input after successful
output** cannot delete source music in this deployment. If that behavior is intentionally required,
back up the library and set `UNUNKNOWN_INPUT_MODE=rw` in `.env`. On startup, writable input
directories are assigned to `PUID:PGID` automatically so originals can be removed; ownership of the
music files themselves is not changed.

For a reproducible NAS deployment, set `UNUNKNOWN_TAG` in `.env` to an exact release such as
`0.6.0` instead of `latest`. The image contains FFmpeg/ffprobe, Chromaprint/fpcalc, the headless
SongRec CLI, CA certificates, and tini; no media tools need to be installed on the Docker host.

Normal pushes and pull requests only run the fast source checks. Pushing a stable semantic version
tag such as `v0.6.0` starts the separate release workflow. It builds `linux/amd64` and
`linux/arm64` images once, in parallel on native GitHub runners, scans both images, publishes a
multi-architecture manifest to GHCR, and creates the matching GitHub Release. That example creates
image tags `0.6.0`, `v0.6.0`, `0.6`, `0`, and `latest`. No registry password is required in the
workflow: it uses the repository's short-lived `GITHUB_TOKEN`. GHCR packages are private on first
publication unless their package visibility is changed in GitHub. For a private package, log in on
the deployment host with a personal access token that has `read:packages` before running
`docker compose pull`; otherwise, change the package visibility to public in GitHub's package
settings.

Before publication, Trivy blocks critical and high image vulnerabilities. The exact FFmpeg
dependency findings that currently have no Alpine 3.24 fix are documented as time-limited,
package-specific exceptions in `.trivyignore.yaml`; CI will fail when those exceptions expire.

For a one-off local image instead of GHCR:

```bash
docker build -t ununknown:local .
UNUNKNOWN_IMAGE=ununknown UNUNKNOWN_TAG=local docker compose up -d
```

Do not change `UNUNKNOWN_HOST` to `0.0.0.0`. The container's internal non-loopback bind is an
explicit Docker-only exception; the published host port must remain loopback-only because the API
accepts local filesystem paths and has no multi-user authentication.

Send `SIGTERM` or press Ctrl+C for a graceful stop. The server asks an active workflow to stop at a
safe boundary, waits up to 30 seconds, drains HTTP requests, and closes SQLite. Back up the database
and keep **Remove input after successful output** disabled until the output has been verified for a
new library.

Disposable provider responses and downloaded artwork are cleared automatically at
local midnight while the app is idle. A missed cleanup runs on the next startup.
Fingerprint, integrity-check, and ReplayGain caches share a 100 MiB limit, with
the oldest entries removed first; the limit is checked at startup and hourly while
the app is idle. Saved settings and configuration are preserved.

Automatic cleaning is enabled by default every five minutes and can be changed or disabled in
**Settings → Automatic cleaning**. Each cycle scans only new or changed source files, writes
strictly matched tracks that are already safe to apply, and leaves every ambiguous or incomplete
track in Review. The scheduler sleeps while any frontend page is open and resumes in the backend
after the page closes, keeping automatic work away from active review sessions. Between deadlines
it uses an event-driven sleep instead of polling; folder discovery runs off the async request
runtime, and unchanged files are compared by size and modification time without decoding audio or
contacting metadata providers.

## Stronger matching with optional services

Open **Optional source keys** in the setup screen to enable these sources. Empty
fields keep a source disabled.

- **AcoustID** identifies audio from its Chromaprint fingerprint. Install
  `fpcalc` with `brew install chromaprint`, then create a free application key at
  <https://acoustid.org/new-application>.
- **AudD** is a fallback for difficult files that the free catalogs and AcoustID
  could not identify. Create a token at <https://dashboard.audd.io/>.
- **SongRec / Shazam** recognizes difficult audio from a local fingerprint and then
  feeds the recognized artist and title back through the metadata catalogs to find
  album data, artwork, credits, release information, and ISRC. Ununknown supports
  both the maintained `songrec` executable and `songrec-lib-cli`; `songrec` is
  preferred. Install it with `cargo install songrec --no-default-features --features
  ffmpeg`, or set `UNUNKNOWN_SONGREC_BIN` to the executable path. Recognition is
  optional, cached for repeat scans, and limited to one request at a time.
- **Spotify** verifies identified tracks by ISRC and contributes release, artwork,
  track number, and date metadata. Create an app at
  <https://developer.spotify.com/dashboard> and enter its client ID and secret.
- **Genius** contributes song identity, artist credits, album, release date, genre,
  and artwork without a key. Pasted Genius song links can fill the manual editor.
- **YouTube Data API** recovers evidence from filenames that contain an exact
  YouTube video ID, which is common for downloaded Persian and international
  music. Enable YouTube Data API v3 and create a key in
  <https://console.cloud.google.com/apis/library/youtube.googleapis.com>.

The pipeline searches free catalogs first, invokes SongRec/Shazam for unresolved
audio, uses AudD only if SongRec did not identify it, then uses identifiers such as
ISRC to cross-check Spotify. A provider result is not
automatically written unless it passes the existing confidence and ambiguity rules.
Radio Javan search and song-link lookup require no key and contribute Persian music
metadata, duration, release date, and original-resolution cover art.
Navahang search and pasted song links require no key and contribute Persian song
identity, duration, release date, credits, label, and original-resolution artwork.
Audiomack search and pasted song links also work without user credentials and contribute
track identity, album, duration, genre, release date, credits, label, ISRC when present, and
original-resolution artwork.
Pasted Shazam song links require no key and import the recognized title, artist, ISRC, album,
release date, track position, duration, genre, composer, label, and high-resolution artwork.
Pasted Spotify track links use the full Spotify catalog metadata when client credentials are
configured, including ISRC, album, release date, track position, duration, and cover; without
credentials they continue to provide title and cover through Spotify oEmbed.

## Product flow

1. Enter an input and output folder.
   Optionally add one or more read-only reference libraries containing music you already own.
2. Optionally add source API keys.
3. Select **Scan and identify**.
4. Resolve files that need help individually, or use **Smart auto-select** to analyze review
   candidates in bulk. It combines recording identity, version markers, duration, independent
   source agreement, existing album context, release type, and compilation status instead of
   blindly taking the highest score. A completion worker then merges missing metadata only from
   agreeing recordings and compatible releases, verifies the actual cover image, and keeps tracks
   without a title, artist, album, or usable cover in review. Ambiguous, unmatched, damaged, and
   incomplete tracks remain in review. Any identified track can be returned to review with
   **Undo identification** without discarding its candidate choices.
5. Select **Write corrected files**.

Steps 3 and 5 can run automatically at the configured interval. Automatic runs never choose a
Review candidate; they only write tracks that passed the normal strict matching and completeness
checks.

Before writing, the output planner removes duplicate recordings from the batch. It uses a
compatible ISRC first, then cached Chromaprint audio fingerprints, with a whole-file SHA-256
fallback. Only one corrected output is written for a duplicate recording. Different audio,
remixes, live versions, and materially different durations remain separate and receive numbered
filenames when their corrected names collide. Existing output files are never bulk-deleted.

Reference libraries are indexed locally and are never modified. On the first scan, Ununknown
stores each reference file's size, modification time, duration, and Chromaprint fingerprint in
SQLite. Later scans fingerprint only new or changed files. Input recordings already present in a
reference library are marked **Skipped** with the matching path before any online catalog lookup or
output write. When fingerprinting is unavailable, an exact SHA-256 file hash is used as a safe
fallback. Reference folders may not overlap the input or output folder. When **Remove processed
inputs and duplicates** is enabled, a matched input duplicate is removed only after Ununknown
rechecks that its reference copy is accessible and resolves to a different file. The input mount
must be writable; reference mounts remain read-only.

The browser uses a deliberately small local API: `/api/setup`, `/api/status`,
`/api/identify`, `/api/tracks`, and `/api/write`. Production packages keep that API on loopback,
serve the frontend from the same process, apply versioned SQLite migrations, and publish corrected
outputs atomically without replacing existing files.
