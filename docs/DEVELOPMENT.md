# Local Developer Guide

This guide explains how to run, edit, inspect, and test the complete Ununknown project.

## Project Parts

Ununknown has three main parts:

```text
Rust backend       http://localhost:7331
Vite frontend      http://localhost:5173
SQLite database    /cache/ununknown.sqlite
```

During development, open the Vite frontend at `http://localhost:5173`. Vite automatically forwards `/api` requests to the Rust backend and refreshes the browser when frontend files change.

## Requirements

Install:

- Current stable Rust
- Node.js 24 and npm
- Chromaprint `fpcalc`
- Docker with Docker Compose

macOS:

```bash
brew install rustup-init node chromaprint
rustup-init
```

Verify:

```bash
rustc --version
cargo --version
node --version
npm --version
fpcalc -version
docker --version
docker compose version
```

## First-Time Setup

From the repository:

```bash
cd /Users/arta/Documents/GITHUB/ununknown
mkdir -p .local/ununknown/music/input .local/ununknown/music/output .local/ununknown/cache .local/ununknown/logs .local/ununknown/fixtures
npm ci --prefix frontend
```

All local runtime data lives under `.local/ununknown`, which is ignored by Git:

```text
.local/ununknown/music/input   copied test music
.local/ununknown/music/output  generated fixed files
.local/ununknown/cache         SQLite database, provider cache, artwork cache
.local/ununknown/logs          optional local logs
.local/ununknown/fixtures      manual/generated local fixtures
```

The backend still uses the same fixed paths as the production container:

```text
/music/input
/music/output
/cache/ununknown.sqlite
```

For native development on macOS, create local links once:

```bash
sudo ln -s "$PWD/.local/ununknown/music" /music
sudo ln -s "$PWD/.local/ununknown/cache" /cache
```

If `/music` or `/cache` already exists, do not replace it. Use Docker development instead.

Put only test copies of music inside:

```text
.local/ununknown/music/input
```

## Fast Edit Loop

Use two terminals.

### Terminal 1: Backend

```bash
cd /Users/arta/Documents/GITHUB/ununknown
cargo run
```

The API and compiled frontend server run at `http://localhost:7331`.

For automatic Rust restarts:

```bash
cargo install cargo-watch
cargo watch -x run
```

Rust changes restart the backend. Database migrations run automatically when the backend starts.

### Terminal 2: Frontend

```bash
cd /Users/arta/Documents/GITHUB/ununknown/frontend
npm run dev
```

Open:

```text
http://localhost:5173
```

Changes inside `frontend/src` appear immediately through Vite hot reload.

## Run Everything With Docker

Use this when native `/music` and `/cache` paths are unavailable, or when testing the production-style container:

```bash
docker compose -f docker-compose.dev.yml up --build
```

Open `http://localhost:7331`.

Docker does not provide fast hot reload. Rebuild after source changes:

```bash
docker compose -f docker-compose.dev.yml up --build
```

Stop it with:

```bash
docker compose -f docker-compose.dev.yml down
```

Do not run Docker and native `cargo run` together because both use port `7331`.

## Database

SQLite stores settings and temporary scan/matching data:

```text
.local/ununknown/cache/ununknown.sqlite
```

Install the SQLite CLI if needed:

```bash
brew install sqlite
```

Inspect the database:

```bash
sqlite3 .local/ununknown/cache/ununknown.sqlite
```

Useful commands inside SQLite:

```sql
.tables
.schema tracks
SELECT id, filename, stage, status, error FROM tracks;
SELECT key FROM settings;
.quit
```

The database contains non-provider settings and temporary workflow data. AcoustID and MusicBrainz configuration must not be stored in it.

For provider testing, export environment variables before starting the backend:

```bash
export UNUNKNOWN_ACOUSTID_API_KEY="your-application-key"
export UNUNKNOWN_MUSICBRAINZ_USER_AGENT="Ununknown/0.4.5 (you@example.com)"
cargo run
```

Reset all local settings and temporary data:

```bash
rm .local/ununknown/cache/ununknown.sqlite
```

Stop the backend before deleting the database. The next backend start creates and migrates a fresh database.

Successful applies remove their temporary track and candidate records automatically.

## Manual Development Test

1. Put copied MP3 or FLAC files in `.local/ununknown/music/input`.
2. Start backend and frontend.
3. Open `http://localhost:5173`.
4. Open **Settings**.
5. Confirm provider environment variables are set before backend startup.
6. Click **Scan music**.
7. Watch the Fetch terminal for provider, score, retry, and unmatched diagnostics.
8. Preview old-vs-new metadata cards and apply selected changes.
9. Verify corrected files under `.local/ununknown/music/output`.

Always test with copied music. Ununknown has no backup or rollback system.

## Automated Tests

### Rust Format

```bash
cargo fmt --check
```

### Rust Lint

```bash
cargo clippy --all-targets -- -D warnings
```

### Rust Unit Tests

```bash
cargo test
```

### Frontend Production Build

This runs TypeScript checking and builds the Vite frontend:

```bash
npm run build --prefix frontend
```

### Docker Build

This verifies the full production image and confirms `fpcalc` exists:

```bash
docker build -t ununknown:test .
```

### Complete MP3/FLAC Workflow Test

```bash
bash scripts/e2e-fixtures.sh
```

The script:

- Generates temporary mistagged MP3 and FLAC files.
- Builds and starts the Docker image.
- Verifies settings persist in SQLite.
- Scans and reads real metadata.
- Seeds deterministic candidates because synthetic audio cannot match AcoustID.
- Previews and applies changes.
- Verifies output tags.
- Confirms input files remain unchanged.
- Confirms applied temporary database records are deleted.
- Deletes only its temporary test files and container.

Expected final message:

```text
E2E passed: generated files scanned, previewed, copied, and retagged successfully.
```

### Run All Normal Checks

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
npm run build --prefix frontend
docker build -t ununknown:test .
bash scripts/e2e-fixtures.sh
```

## Reset Local Runtime Workspace

Stop any local container first:

```bash
docker compose -f docker-compose.dev.yml down
```

Remove generated cache and output, then recreate the folders:

```bash
rm -rf .local/ununknown/cache .local/ununknown/music/output
mkdir -p .local/ununknown/cache .local/ununknown/music/output
```

Keep `.local/ununknown/music/input` if you want to reuse your copied test music. Delete it too when you want a completely clean local workspace.

## Useful API Checks

With the backend running:

```bash
curl http://localhost:7331/api/health
curl http://localhost:7331/api/settings
curl "http://localhost:7331/api/tracks?page=1&page_size=100"
curl http://localhost:7331/api/jobs
```

Start a scan:

```bash
curl -X POST http://localhost:7331/api/scan/start
```

Watch backend logs in the terminal running `cargo run`. For Docker:

```bash
docker compose -f docker-compose.dev.yml logs -f
```

## Common Problems

- **Port 7331 already used:** stop Docker or another backend process.
- **Frontend opens but APIs fail:** confirm `cargo run` is running on port `7331`.
- **No files found:** confirm test music exists under `/music/input` or the Docker-mounted `.local/ununknown/music/input`.
- **`fpcalc` not found:** install Chromaprint or use Docker.
- **Database permission error:** ensure `/cache` or `.local/ununknown/cache` is writable.
- **Provider failures:** check the Fetch terminal, provider environment variables, and backend logs.
