# Ununknown

Current release: **0.4.0**

Ununknown is a small, local-first web app that repairs music metadata. It scans folders recursively, fingerprints audio with Chromaprint, finds metadata using AcoustID and MusicBrainz, shows a required dry-run preview, then writes approved tags and artwork.

There are no accounts, login, streaming features, or cloud storage. Settings are managed in the web UI and saved locally in SQLite.

The complete tabbed Settings page exposes matching thresholds, every supported metadata field, path templates, collision behavior, provider configuration, and protected Expert Mode file operations.

## Fastest Start

Requirements: Docker with Docker Compose.

```bash
mkdir -p ununknown/music/input ununknown/music/output ununknown/cache
cd ununknown
curl -O https://raw.githubusercontent.com/artamrj/ununknown/main/docker-compose.yml
docker compose up -d
```

Open <http://localhost:7331>, open **Settings**, and enter a MusicBrainz contact such as:

```text
Ununknown/0.1 (you@example.com)
```

Copy test music into `music/input`, click **Scan music**, let the sequential pipeline finish, preview every matched change, then apply.

```text
./music/input  -> /music/input
./music/output -> /music/output
./cache        -> /cache
```

**Test with copied files first.** Ununknown has no backup or rollback system.

## Provider Setup

- **MusicBrainz does not require an API key.** Set `UNUNKNOWN_MUSICBRAINZ_USER_AGENT` in Docker Compose; requests are limited to one per second.
- **AcoustID is optional.** Set an application key from <https://acoustid.org/api-key> as `UNUNKNOWN_ACOUSTID_API_KEY` in Docker Compose.
- Without AcoustID, Ununknown can search MusicBrainz using existing tags. These lower-confidence results always require review.
- Cover artwork comes from Cover Art Archive when a release is known.

Provider failures are shown on affected tracks instead of being hidden as “no match.”

## Version 0.3 Workflow

```text
Scan -> Fetch -> Preview -> Apply -> Finish
```

Scan discovers and sorts all audio paths without saving them. Fetch processes one file at a time, retries provider failures using the configured attempt limit, and moves on. Unmatched and failed files are discarded after their totals are updated.

Only successful matched proposals are temporarily saved in SQLite for preview recovery. They expire after one day by default, starting a new scan clears them immediately, and successful Apply deletes them. Apply is rejected until a current successful preview exists.

- **Safe:** automatically selects fingerprint matches scoring at least 90.
- **Aggressive:** automatically selects fingerprint matches scoring at least 75.
- **Manual:** selects nothing automatically.

## Guides

- [Docker Compose and deployment](docs/DEPLOYMENT.md)
- [Setup and use after deployment](docs/USAGE.md)
- [Local developer run and testing](docs/DEVELOPMENT.md)

## Local Development

See the [developer guide](docs/DEVELOPMENT.md) for frontend hot reload, backend development, SQLite inspection, Docker development, and all test commands.

## Supported Formats

MP3, FLAC, M4A, OGG, and Opus are primary formats. WAV and AIFF are scanned but unsafe writes are skipped with a warning. Ununknown never transcodes files.

## License

MIT
