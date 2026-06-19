# Backend Start Here

This guide explains the Ununknown backend in practical terms. Read this first if
you want to understand what the backend does, where the important code lives,
and how to change it without guessing.

Ununknown is a local music metadata repair app. The backend scans a music
folder, reads existing tags, fingerprints audio files with `fpcalc`, asks
AcoustID and MusicBrainz for better metadata, stores matches in SQLite, previews
the planned writes, and then applies selected tag and file path changes.

## Backend Responsibilities

The backend is one Rust process. It does four jobs:

1. Serves the HTTP API under `/api`.
2. Serves the built frontend from `frontend/dist`.
3. Stores workspace data, settings, preview tokens, provider cache, and run
   history in SQLite.
4. Runs long workflows such as scan and apply in background Tokio tasks while
   streaming progress to the frontend through `/api/events`.

The backend starts in `src/main.rs`. Startup does this:

1. Initializes logging.
2. Builds default config from environment variables.
3. Opens SQLite and runs migrations.
4. Loads saved settings from the database.
5. Cleans interrupted and expired work.
6. Builds shared `AppState`.
7. Mounts `/api` routes and the static frontend fallback.
8. Listens on `0.0.0.0:7331`.

## How To Run

Docker is the easiest path because the image includes `fpcalc`.

```bash
mkdir -p .local/cache .local/input .local/output
cp .env.example .env.local
docker compose --env-file .env.local -f docker-compose.local.yml up --build
```

Open `http://localhost:7331`. Put test music in `.local/input`. Copy-mode output
goes to `.local/output`. SQLite data goes to `.local/cache`.

More Docker details are in [Docker](../docker.md).

To run the Rust backend directly, install Rust and Chromaprint so `fpcalc` is on
your `PATH`.

```bash
export UNUNKNOWN_DB=.local/cache/ununknown.sqlite
export UNUNKNOWN_INPUT_DIR=.local/input
export UNUNKNOWN_OUTPUT_DIR=.local/output
export UNUNKNOWN_MUSICBRAINZ_USER_AGENT='Ununknown/0.5.0 (https://github.com/artamrj/ununknown)'
cargo run
```

The direct Rust run expects the frontend build to exist if you want the browser
UI served by the backend.

## Main Workflow

The normal user flow is:

```text
start scan
  -> discover supported audio files
  -> read current tags
  -> run fpcalc
  -> query AcoustID when configured
  -> query/search MusicBrainz
  -> score candidates
  -> store tracks and candidates in SQLite
  -> preview selected changes
  -> apply preview token
  -> copy/write tags or write in place
```

The apply step requires a preview token from `/api/apply/preview`. Preview tokens
are stored in SQLite, are single-use, and can become stale when a new scan or
workspace-changing action invalidates them.

## Code Map

- `src/main.rs`: process startup, router mounting, server bind.
- `src/config.rs`: settings, defaults, validation, secret hiding.
- `src/types.rs`: public wire enums and ID wrapper types.
- `src/app/`: shared runtime state, workflow counters, terminal log, events.
- `src/application/`: high-level workflows such as scan and preview storage.
- `src/domain/`: app rules that do not depend on HTTP, such as matching and path
  templates.
- `src/http/`: routes, handlers, request/response structs, API errors.
- `src/infrastructure/`: SQLite, provider clients, media I/O, static files.
- `src/jobs.rs`: event payload sent over server-sent events.
- `migrations/`: SQLite schema migrations embedded by `sqlx::migrate!()`.

## Rust Concepts Used Here

- `Arc<AppState>` lets all handlers and background tasks share the same app
  state safely.
- `RwLock<Config>` and `RwLock<Workflow>` allow many readers or one writer for
  mutable runtime state.
- `SqlitePool` is the shared database connection pool.
- `Semaphore` limits expensive work such as fingerprinting, provider calls,
  artwork downloads, and tag writes.
- `tokio::spawn` starts scan/apply workflows without blocking HTTP requests.
- `spawn_blocking` is used for local media reads that are not async.
- Server-sent events let the frontend receive workflow and terminal updates from
  `/api/events`.

## Read Next

- [Configuration](configuration.md)
- [API Reference](api.md)
- [Database](database.md)
- [Workflows](workflows.md)
- [Developer Guide](developer-guide.md)
- [Architecture](../architecture.md)
