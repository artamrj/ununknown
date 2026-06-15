# Ununknown

Ununknown is a fast, local-first web tool for repairing missing or incorrect music metadata. It recursively scans mounted folders, fingerprints tracks with Chromaprint, matches them through AcoustID and MusicBrainz, previews every planned change, then writes selected tags and Cover Art Archive artwork.

It is a practical NAS tool: no authentication, accounts, streaming, playlists, backups, or cloud service.

## Quick Start

1. Create local folders and copy the example configuration:

   ```bash
   mkdir -p music/input music/output config cache
   cp config.example.toml config/config.toml
   ```

2. Edit `config/config.toml` with an AcoustID API key and a meaningful MusicBrainz User-Agent.
3. Put test copies of audio files under `music/input`.
4. Start the app:

   ```bash
   docker compose up --build
   ```

5. Open <http://localhost:7331>.

Compose mounts:

```text
./music/input  -> /music/input
./music/output -> /music/output
./config       -> /config
./cache        -> /cache
```

**Test with copied files first.** In-place metadata writing and optional organization change existing files. Ununknown intentionally has no backup or rollback system.

## Workflow

Ununknown enforces:

```text
Scan -> Match -> Dry-run preview -> Confirm -> Apply
```

Scanning never modifies audio. A successful current dry-run is required before Apply can start.

- **Safe Auto:** selects matches scoring at least 90.
- **Aggressive Auto:** selects matches scoring at least 75.
- **Manual Review:** selects nothing automatically.

All modes require an explicit Apply confirmation.

## Providers

### AcoustID

Create an application API key at <https://acoustid.org/api-key> and set `acoustid_api_key` in `config/config.toml`. The key is used only to identify Chromaprint fingerprints.

### MusicBrainz

MusicBrainz requires applications to send a meaningful User-Agent containing application and contact information. Set:

```toml
musicbrainz_user_agent = "Ununknown/0.1 (you@example.com)"
```

Ununknown limits MusicBrainz requests to one request per second. Cover images come from Cover Art Archive when a release is known.

Secrets remain in TOML/environment configuration. Use `UNUNKNOWN_ACOUSTID_API_KEY` and
`UNUNKNOWN_MUSICBRAINZ_USER_AGENT` for environment-based configuration. The UI and API
expose only configured/not-configured flags.

## Output And Templates

Copy mode writes under `/music/output`. The default path template is:

```text
$albumartist/$album/$track - $title
```

Supported variables include `$artist`, `$albumartist`, `$album`, `$title`, `$track`, `$tracktotal`, `$disc`, `$disctotal`, `$year`, `$genre`, `$composer`, `$isrc`, `$label`, `$format`, `$bitrate`, and `$ext`.

The original extension is always preserved; Ununknown does not transcode. Generated paths are sanitized and prevented from escaping the configured root. Collision strategies are `skip`, `overwrite`, and `rename`.

In-place mode writes tags without moving files by default. File renaming and folder reorganization must be explicitly enabled.

## Format Safety

MP3, FLAC, M4A, OGG, and Opus are the primary formats. WAV and AIFF can be scanned, but this MVP skips writes because their tag combinations require conditional safety handling. Dry-run and apply results show the warning.

Tag writes use Lofty’s supported write path. Atomic behavior is best effort and varies by format.

## Development

Requirements: current stable Rust, Node.js 24, npm, and `fpcalc`.

```bash
npm ci --prefix frontend
npm run build --prefix frontend
cargo run
```

Checks:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
npm run build --prefix frontend
docker build -t ununknown .
```

The Docker runtime installs the Debian package that provides `fpcalc` and verifies the executable during image build. On other distributions, install the package commonly named `chromaprint-tools`, `libchromaprint-tools`, or the package that provides `fpcalc`.

## API

Core endpoints include health, settings, scan jobs, tracks, candidates, SSE events, path-template previews, required apply previews, and apply jobs. See the route definitions in `src/api.rs` for the compact MVP contract.

## License

MIT
