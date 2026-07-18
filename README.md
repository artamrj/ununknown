# Ununknown

Ununknown has one job: correct metadata for a folder of music.

It first checks that each audio stream can be decoded, then tries every configured
automatic source (audio fingerprinting, MusicBrainz, Apple Music, Deezer, Radio Javan, Audiomack, Genius,
Spotify, Discogs, Last.fm, TheAudioDB, Wikidata, YouTube, and Cover Art Archive). Difficult
tracks can optionally use AudD audio recognition. Reliable matches are selected
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

## Stronger matching with optional services

Open **Optional source keys** in the setup screen to enable these sources. Empty
fields keep a source disabled.

- **AcoustID** identifies audio from its Chromaprint fingerprint. Install
  `fpcalc` with `brew install chromaprint`, then create a free application key at
  <https://acoustid.org/new-application>.
- **AudD** is a fallback for difficult files that the free catalogs and AcoustID
  could not identify. Create a token at <https://dashboard.audd.io/>.
- **Spotify** verifies identified tracks by ISRC and contributes release, artwork,
  track number, and date metadata. Create an app at
  <https://developer.spotify.com/dashboard> and enter its client ID and secret.
- **Genius** contributes song identity, artist credits, album, release date, genre,
  and artwork without a key. Pasted Genius song links can fill the manual editor.
- **YouTube Data API** recovers evidence from filenames that contain an exact
  YouTube video ID, which is common for downloaded Persian and international
  music. Enable YouTube Data API v3 and create a key in
  <https://console.cloud.google.com/apis/library/youtube.googleapis.com>.

The pipeline searches free catalogs first, invokes AudD only for unresolved audio,
then uses identifiers such as ISRC to cross-check Spotify. A provider result is not
automatically written unless it passes the existing confidence and ambiguity rules.
Radio Javan search and song-link lookup require no key and contribute Persian music
metadata, duration, release date, and original-resolution cover art.
Audiomack search and pasted song links also work without user credentials and contribute
track identity, album, duration, genre, release date, credits, label, ISRC when present, and
original-resolution artwork.

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

The browser uses a deliberately small local API: `/api/setup`, `/api/status`,
`/api/identify`, `/api/tracks`, and `/api/write`. There are no Docker,
deployment, authentication, background-job, or production-build layers.
