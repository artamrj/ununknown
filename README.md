# Ununknown

Ununknown has one job: correct metadata for a folder of music.

It first checks that each audio stream can be decoded, then tries every configured
automatic source (audio fingerprinting, MusicBrainz, Apple Music, Deezer, Discogs,
Last.fm, TheAudioDB, Wikidata, and Cover Art Archive). Reliable matches are selected
automatically. Uncertain and unmatched files stay in a short review list where you
can choose a candidate or enter metadata manually. Corrected files are written as
copies with ReplayGain metadata, so source music is never modified. Damaged audio is
reported and blocked from the write queue.

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

## Product flow

1. Enter an input and output folder.
2. Optionally add source API keys.
3. Select **Scan and identify**.
4. Resolve only the files that need help.
5. Select **Write corrected files**.

The browser uses a deliberately small local API: `/api/setup`, `/api/status`,
`/api/identify`, `/api/tracks`, and `/api/write`. There are no Docker,
deployment, authentication, background-job, or production-build layers.
