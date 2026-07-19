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
`UNUNKNOWN_STATIC_DIR`, and a loopback-only `UNUNKNOWN_BIND`. Provider credentials can be supplied
without persisting them in SQLite through `UNUNKNOWN_ACOUSTID_KEY`, `UNUNKNOWN_AUDD_TOKEN`,
`UNUNKNOWN_SPOTIFY_CLIENT_ID`, `UNUNKNOWN_SPOTIFY_CLIENT_SECRET`,
`UNUNKNOWN_SOUNDCLOUD_CLIENT_ID`, `UNUNKNOWN_SOUNDCLOUD_CLIENT_SECRET`,
`UNUNKNOWN_YOUTUBE_API_KEY`, `UNUNKNOWN_DISCOGS_TOKEN`, `UNUNKNOWN_LASTFM_KEY`, and
`UNUNKNOWN_THEAUDIODB_KEY`.

Send `SIGTERM` or press Ctrl+C for a graceful stop. The server asks an active workflow to stop at a
safe boundary, waits up to 30 seconds, drains HTTP requests, and closes SQLite. Back up the database
and keep **Remove input after successful output** disabled until the output has been verified for a
new library.

Disposable provider responses and downloaded artwork are cleared automatically at
local midnight while the app is idle. A missed cleanup runs on the next startup.
Fingerprint, integrity-check, and ReplayGain caches share a 100 MiB limit, with
the oldest entries removed first; the limit is checked at startup and hourly while
the app is idle. Saved settings and configuration are preserved.

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

Before writing, the output planner removes duplicate recordings from the batch. It uses a
compatible ISRC first, then cached Chromaprint audio fingerprints, with a whole-file SHA-256
fallback. Only one corrected output is written for a duplicate recording. Different audio,
remixes, live versions, and materially different durations remain separate and receive numbered
filenames when their corrected names collide. Existing output files are never bulk-deleted.

The browser uses a deliberately small local API: `/api/setup`, `/api/status`,
`/api/identify`, `/api/tracks`, and `/api/write`. Production packages keep that API on loopback,
serve the frontend from the same process, apply versioned SQLite migrations, and publish corrected
outputs atomically without replacing existing files.
