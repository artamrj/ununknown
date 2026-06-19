# Ununknown Architecture

Ununknown is a local music metadata repair application. It scans an input music
folder, reads existing audio tags, fingerprints files with `fpcalc`, looks up
candidate metadata from AcoustID and MusicBrainz, previews proposed changes, and
then applies selected tag and file path updates.

Project identity:

- Package name: `ununknown`
- Version: `0.5.0`
- License: MIT
- Backend: Rust 2024
- Frontend: React, TypeScript, and Vite
- Database: SQLite
- Runtime HTTP address: `0.0.0.0:7331`

The repository is split into a Rust backend under `src/`, a React frontend under
`frontend/`, and SQLite migrations under `migrations/`. The backend also serves
the built frontend from `frontend/dist`.

## System Overview

The application is a single backend process with an embedded HTTP API, a static
frontend server, a SQLite database, and in-memory workflow state.

At runtime:

1. The backend starts from `src/main.rs`.
2. Configuration defaults are built from environment variables and hard-coded
   defaults.
3. SQLite is opened through `sqlx`, migrations are applied, and saved settings
   are loaded.
4. Startup cleanup marks interrupted work as failed and removes expired
   workspace, job, and provider-cache records.
5. An `AppState` is created and shared across all HTTP handlers.
6. `/api/*` routes serve JSON and server-sent events.
7. All other requests fall back to static files from `frontend/dist`.
8. The frontend calls the API, subscribes to `/api/events`, and renders the
   current workflow state.

The main data flow is:

```text
input folder
  -> audio discovery
  -> tag read
  -> fpcalc fingerprint
  -> AcoustID lookup when configured
  -> MusicBrainz lookup/search
  -> scoring and automatic candidate selection
  -> SQLite tracks/candidates
  -> preview token
  -> copy or in-place apply
  -> tag writing and optional artwork embedding
```

## Backend Architecture

The backend is an Axum application with Tokio async runtime, SQLite persistence,
HTTP clients for provider integrations, and blocking tasks for local media I/O.

Important dependencies from `Cargo.toml`:

- `axum`: HTTP routing, extractors, JSON responses, SSE responses.
- `tokio`: async runtime, tasks, file operations, process execution.
- `sqlx`: SQLite connection pooling, queries, and migrations.
- `reqwest`: HTTP client for AcoustID, MusicBrainz, and artwork downloads.
- `lofty`: audio metadata and artwork read/write.
- `tower-http`: request tracing and static file serving.
- `walkdir`: recursive input folder scanning.
- `strsim`: metadata similarity scoring.
- `uuid`: preview and apply job token generation.
- `filetime`: preserving modification timestamps for in-place writes.

### Entrypoint And Startup

`src/main.rs` owns process startup.

It initializes tracing, builds default configuration, opens the database, loads
saved settings, runs cleanup, creates shared state, builds the Axum router, and
binds the server to `0.0.0.0:7331`.

Environment variables:

| Variable | Purpose | Default |
| --- | --- | --- |
| `UNUNKNOWN_DB` | SQLite database path | `/cache/ununknown.sqlite` |
| `UNUNKNOWN_INPUT_DIR` | Root folder scanned for music | `/music/input` |
| `UNUNKNOWN_OUTPUT_DIR` | Copy-mode output folder | `/music/output` |
| `UNUNKNOWN_ACOUSTID_API_KEY` | AcoustID API key | empty |
| `UNUNKNOWN_MUSICBRAINZ_USER_AGENT` | MusicBrainz contact user agent | `Ununknown/0.5.0 (https://github.com/artamrj/ununknown)` |

The backend nests the API under `/api` and serves static frontend assets as the
fallback service:

```text
Router
  /api -> src/http/router.rs
  fallback -> src/infrastructure/static_files.rs -> frontend/dist
```

### Module Layout

`src/app/`

- Defines shared runtime state.
- `AppState` contains current settings, SQLite pool, HTTP client, event
  broadcast channel, cancellation state, preview-token storage, and workflow
  progress.
- `Workflow` is the frontend-facing process state.
- `TerminalLine` is a compact log entry exposed in the UI and over SSE.

`src/application/`

- Contains orchestration logic for high-level application workflows.
- `scan_pipeline.rs` performs recursive discovery, metadata read,
  fingerprinting, provider matching, scoring, and persistence.

`src/config.rs`

- Defines all application settings.
- Owns default values, validation rules, public settings serialization, and
  secret hiding.
- Important structs: `Config`, `PathTemplateConfig`, `InPlaceConfig`,
  `MetadataFields`, `PublicSettings`.

`src/domain/`

- Contains core local rules that do not depend on HTTP handlers.
- `audio.rs` reads audio metadata and embedded artwork and defines supported
  file extensions.
- `matcher.rs` scores provider candidates against current tags and duration.
- `path_templates.rs` renders destination paths, sanitizes path components,
  blocks traversal, preserves extensions, and resolves collisions.

`src/http/`

- Contains API routing, handlers, and error conversion.
- `router.rs` defines all public API paths.
- `handlers/mod.rs` contains endpoint implementations and API DTOs.
- `error.rs` maps typed application errors to JSON responses with specific HTTP status codes.

`src/infrastructure/`

- Contains integrations with external systems and local infrastructure.
- `db/mod.rs` opens SQLite, runs migrations, loads/saves settings, and performs
  startup cleanup.
- `media/fingerprint.rs` runs `fpcalc -json`.
- `media/tag_writer.rs` writes metadata and cover art with `lofty`.
- `providers/` integrates with AcoustID, MusicBrainz, and Cover Art Archive.
- `static_files.rs` serves `frontend/dist`.

`src/jobs.rs`

- Defines the event payload sent through the broadcast channel and SSE stream.
- `emit` publishes workflow progress events.

### Shared Runtime State

`AppState` is wrapped in `Arc` and attached to the Axum router.

It contains:

- `config: RwLock<Config>`: current runtime settings.
- `pool: SqlitePool`: shared SQLite pool.
- `client: reqwest::Client`: reused outbound HTTP client.
- `events: broadcast::Sender<jobs::Event>`: event stream fanout.
- `cancelled: RwLock<HashSet<String>>`: job cancellation IDs.
- `previews: RwLock<HashMap<String, Vec<PreviewItem>>>`: dry-run preview tokens.
- `workflow: RwLock<Workflow>`: current UI workflow state.

`Workflow` contains:

- `phase`: `idle`, `scan`, `fetch`, `preview`, `apply`, `finish`, or `failed`.
- `message`: current human-readable status.
- `current_file`: file currently being processed.
- counters for current, total, processed, matched, unmatched, and failed.
- `terminal_log`: last 160 terminal lines.
- `cancelled`: internal flag skipped during serialization.

`AppState::terminal` appends a terminal line, trims the log to 160 entries, and
emits a `terminal` event so the frontend can update without waiting for polling.

### Configuration Model

`Config` is the main settings object. It includes:

- path settings: `db_path`, `input_dir`, `output_dir`, `output_mode`.
- matching settings: `automation_mode`, `confidence_threshold`,
  `track_attempts`.
- metadata settings: `cover_art_enabled`, `overwrite_existing_tags`,
  `metadata_fields`.
- safety and retention: `expert_mode`, `workspace_retention_days`,
  `job_retention_days`.
- secrets and provider identity: `acoustid_api_key`,
  `musicbrainz_user_agent`.
- file naming: `path_templates`, `in_place`.

Secrets are not exposed through public settings:

- `db_path`, `acoustid_api_key`, and `musicbrainz_user_agent` are skipped by
  serde in the main config.
- `Config::public` clears the AcoustID key and exposes boolean status fields:
  `acoustid_configured` and `musicbrainz_configured`.

Validation rules include:

- Input folder must be non-empty.
- Output folder is required unless `output_mode` is `in_place`.
- `output_mode` must be `copy` or `in_place`.
- `automation_mode` must be `safe`, `aggressive`, `manual`, or `custom`.
- Confidence threshold must be 0 through 100.
- Track attempts must be 1 through 10.
- MusicBrainz user agent must contain product/version, parentheses, and an
  email address or website.
- Collision strategy must be `skip`, `overwrite`, or `rename`.
- Number padding cannot exceed 8.
- Filename limit must be 32 through 255.
- Retention values must be 1 through 365 days.
- Destructive settings require Expert Mode.

Default behavior is conservative: copy mode, safe automation, 90 confidence,
three attempts per track, cover art enabled, overwrite existing tags enabled,
and Expert Mode disabled.

### HTTP API

All API routes are mounted under `/api`.

Health:

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/health` | Returns `{ "status": "ok" }`. |

Settings:

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/settings` | Returns public settings and provider configured flags. |
| `PUT` | `/settings` | Saves editable settings after validation. |
| `POST` | `/settings/reset` | Resets settings while preserving paths and secret values. |
| `POST` | `/settings/reset/{section}` | Resets `matching`, `metadata`, or `files` section. |

Workspace:

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/workspace` | Returns current workflow state and restores preview phase when matched tracks exist. |
| `POST` | `/workspace/clear` | Deletes tracks, jobs, provider cache, previews, and resets workflow. |

Provider tests:

| Method | Path | Purpose |
| --- | --- | --- |
| `POST` | `/providers/acoustid/test` | Checks that an AcoustID key is configured. |
| `POST` | `/providers/musicbrainz/test` | Performs a MusicBrainz test request. |

Scan control:

| Method | Path | Purpose |
| --- | --- | --- |
| `POST` | `/scan/start` | Starts a new scan after clearing temporary workspace data. |
| `POST` | `/scan/stop` | Sets the workflow cancellation flag. |

Jobs:

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/jobs` | Returns the current workflow as a single job-like item. |
| `GET` | `/jobs/{id}` | Returns the current workflow regardless of ID. |

Tracks and candidates:

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/tracks` | Lists paginated tracks with candidates and stage counts. |
| `GET` | `/tracks/{id}` | Returns one track row. |
| `GET` | `/tracks/{id}/candidates` | Returns candidates for a track. |
| `POST` | `/tracks/{id}/select-candidate` | Selects or clears the selected candidate. |
| `PUT` | `/candidates/{id}` | Edits candidate metadata and marks provider as `manual`. |
| `POST` | `/tracks/{id}/retry` | Marks one track for retry and starts a scan. |
| `POST` | `/tracks/bulk/retry` | Marks failed tracks for retry and starts a scan. |
| `POST` | `/tracks/bulk/skip` | Skips tracks in review stage. |

Artwork:

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/artwork/current/{id}` | Reads embedded artwork from the current track file. |
| `GET` | `/artwork/proposed/{id}` | Downloads and caches proposed candidate artwork. |

Path templates:

| Method | Path | Purpose |
| --- | --- | --- |
| `POST` | `/path-template/preview` | Renders sample or track-specific destination paths. |

Apply:

| Method | Path | Purpose |
| --- | --- | --- |
| `POST` | `/apply/preview` | Creates a dry-run preview and returns a preview token. |
| `POST` | `/apply/start` | Consumes a preview token and starts applying changes. |
| `POST` | `/apply/stop` | Sets the workflow cancellation flag. |

Events:

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/events` | Streams workflow and terminal updates as server-sent events. |

### API Error Handling

`src/http/error.rs` defines `ApiError`.

Handlers return `ApiResult<T>` with typed errors. Errors are converted into:

```json
{
  "error": "message"
}
```

with status codes based on the failure class:

| Error | HTTP |
| --- | --- |
| Validation | `422 Unprocessable Entity` |
| Not found | `404 Not Found` |
| Conflict | `409 Conflict` |
| Provider failure | `502 Bad Gateway` |
| Provider timeout | `504 Gateway Timeout` |
| Secret/config issue | `403 Forbidden` |
| I/O/internal failure | `500 Internal Server Error` |

## Scan And Matching Workflow

The scan pipeline lives in `src/application/scan_pipeline.rs`.

### Scan Start

`POST /scan/start` refuses to start when the workflow phase is already `scan`,
`fetch`, or `apply`.

When accepted, it:

1. Deletes existing rows from `tracks`.
2. Deletes existing rows from `provider_cache`.
3. Clears in-memory previews.
4. Resets workflow to `scan`.
5. Writes a terminal message.
6. Spawns `scan_pipeline::run` in a Tokio task.

The scan is sequential. Files are discovered first, sorted, and then processed
one by one.

### File Discovery

The pipeline uses `WalkDir` over `Config.input_dir` with symlink following
disabled.

Supported extensions are defined in `domain/audio.rs`:

- `mp3`
- `flac`
- `m4a`
- `ogg`
- `opus`
- `wav`
- `aiff`
- `aif`

Paths are canonicalized and sorted before processing.

### Metadata Read

For each file, `domain::audio::read` runs in `tokio::task::spawn_blocking`
because `lofty` performs local file parsing.

The returned `AudioInfo` includes:

- title
- artist
- album
- album artist
- track number
- duration
- bitrate
- format

Artwork reading uses `domain::audio::artwork`, which prefers front cover art and
falls back to the first embedded picture.

### Fingerprinting

`infrastructure/media/fingerprint.rs` runs:

```text
fpcalc -json <path>
```

The command has a 180 second timeout. The JSON output must contain a
`fingerprint` and `duration`.

`fpcalc` must be available on the runtime `PATH`.

### Provider Identification

Provider orchestration is in `src/infrastructure/providers/mod.rs`.

If `Config.acoustid_api_key` is non-empty:

1. AcoustID `/v2/lookup` is called with `meta=recordings`.
2. Up to three recording hits are used.
3. Each hit is expanded through MusicBrainz recording lookup.
4. Scores combine AcoustID confidence, title similarity, artist similarity, and
   duration closeness.

If no AcoustID candidates are produced:

1. The current title tag is used for a MusicBrainz recording search.
2. The current artist tag is added to the query when available.
3. Up to three MusicBrainz search results are converted to candidates.
4. Scores are text-only and capped at 70.

MusicBrainz behavior:

- Every MusicBrainz request validates the configured user agent.
- Requests are rate limited to one request per second process-wide.
- HTTP 429 and server errors are retried up to three attempts with exponential
  delay.
- Recording lookups request `artists+releases+isrcs`.
- Cover URLs are generated from release IDs using Cover Art Archive front-500
  URLs.

AcoustID behavior:

- Requests are sent to `https://api.acoustid.org/v2/lookup`.
- The configured API key is sent as the `client` form value.
- Non-`ok` AcoustID responses are treated as errors.

### Scoring And Automatic Selection

Scoring rules are in `src/domain/matcher.rs`.

Fingerprint-backed score:

- AcoustID score contributes up to 70 points.
- Current title similarity contributes up to 8 points.
- Current artist similarity contributes up to 8 points.
- Duration contributes up to 14 points:
  - within 2 seconds: 14
  - within 5 seconds: 7
  - otherwise: 0
- Final score is clamped to 0 through 100.

Text fallback score:

- title similarity contributes up to 45 points.
- artist similarity contributes up to 25 points.
- final fallback score is capped at 70.

Automation thresholds:

| Mode | Threshold | Behavior |
| --- | ---: | --- |
| `safe` | 90 | Conservative automatic selection. |
| `aggressive` | 75 | Lower confidence accepted. |
| `manual` | 101 | No normal candidate can be auto-selected. |
| `custom` | `confidence_threshold` | Uses the configured value. |

The best candidate is selected only if its score is at or above the threshold.
If no candidate passes, the track is counted as unmatched and is not persisted
as a selected preview item.

### Persistence During Scan

Matched tracks are stored in `tracks`, and selected metadata is stored in
`candidates`.

If a track path already exists, its candidates are deleted and the existing row
is updated. Otherwise a new track row is inserted.

The selected candidate ID is stored on the track so preview generation can load
the chosen metadata without recalculating provider results.

## Preview And Apply Workflow

The preview and apply workflow is implemented mostly in `src/http/handlers/mod.rs`.

### Preview Generation

`POST /apply/preview`:

1. Loads tracks where `selected_candidate_id IS NOT NULL` and `is_missing = 0`.
2. Loads selected candidates.
3. Computes duplicate actions.
4. Computes destination paths.
5. Builds old/new metadata summaries.
6. Adds artwork URLs.
7. Generates a UUID preview token.
8. Stores writable preview items in memory under that token.
9. Returns all preview items plus summary counts.

Duplicate detection groups selected tracks by candidate title, artist, album,
track number, and duration bucket. The strongest candidate is kept; weaker
duplicates are marked with `duplicate_action = "skip_duplicate"` and are omitted
from the apply-token item list.

Preview items include:

- current path
- destination path
- write action
- warnings
- duplicate metadata
- old metadata summary
- new metadata summary
- current and proposed artwork URLs
- confidence score
- artwork action

The preview token is in-memory only. A backend restart invalidates existing
preview tokens.

### Destination Paths

Destination generation uses `domain/path_templates.rs`.

Template variables include:

- `$artist`
- `$albumartist`
- `$album`
- `$title`
- `$track`
- `$tracktotal`
- `$disc`
- `$disctotal`
- `$year`
- `$genre`
- `$composer`
- `$isrc`
- `$label`
- `$format`
- `$bitrate`
- `$ext`

Rendering behavior:

- Unknown artist, album, and title fall back to configured placeholder values.
- Track and disc numbers use configured zero-padding.
- Path components are Unicode-normalized, sanitized, whitespace-collapsed, and
  length-limited.
- Absolute paths, root components, prefixes, and parent-directory traversal are
  rejected.
- Original extension is preserved automatically.
- Existing destination collisions are handled by `skip`, `overwrite`, or
  `rename`.

Template choice:

- Copy mode with album artist `Various Artists` uses the compilation template.
- Copy mode otherwise uses the default template.
- In-place mode without folder rename uses `in_place.filename_template`.
- In-place mode with folder rename uses the default path template relative to
  the input directory.

### Apply Start

`POST /apply/start` requires a current preview token. The token is removed from
memory so it cannot be reused.

The handler:

1. Sets workflow phase to `apply`.
2. Generates an apply job UUID.
3. Spawns the apply process in a Tokio task.
4. Returns the job ID.

### Apply Execution

For each preview item:

1. The selected candidate is reloaded from SQLite.
2. Cover art is downloaded from Cover Art Archive when enabled and available.
3. In copy mode, parent directories are created and the source file is copied to
   the destination.
4. In in-place mode, the original file is used as the write target.
5. Tags are written in a blocking task through `media/tag_writer.rs`.
6. Modification time is restored when `in_place.preserve_mtime` is enabled.
7. In-place rename/folder moves are performed after successful tag writing.
8. The track row is updated with output path, status, error, and apply time.
9. Successfully applied tracks are deleted from the temporary workspace table.
10. Workflow progress events are emitted.

If copy-mode tag writing fails, the copied output file is removed.

### Tag Writing

`src/infrastructure/media/tag_writer.rs` writes metadata through `lofty`.

It supports field-level enablement through `Config.metadata_fields` and respects
`Config.overwrite_existing_tags`.

Writable metadata includes:

- title
- artist
- album
- album artist
- ISRC
- genre
- composer
- label
- release date
- comment
- MusicBrainz recording ID
- MusicBrainz release ID
- MusicBrainz artist ID
- MusicBrainz release artist ID
- track number and total
- disc number and total
- cover art

Cover art behavior:

- Cover art is embedded only when `metadata_fields.embed_cover_art` is enabled
  and artwork bytes are available.
- Existing front-cover art is removed only when
  `metadata_fields.replace_existing_cover_art` is enabled.
- New cover art is added only when replacement is enabled or no pictures exist.

Writing limitation:

- `wav`, `aiff`, and `aif` tag writing is skipped because those formats are
  treated as conditional/unsafe in this MVP.

## Database And Persistence

SQLite schema is defined by embedded `sqlx::migrate!()` migrations in
`migrations/`.

`src/infrastructure/db/mod.rs` opens the database URL as:

```text
sqlite://<db_path>?mode=rwc
```

The parent database directory is created before connecting. The pool uses up to
five connections.

### Tables

`tracks`

- Temporary workspace table for scanned audio files.
- Stores source path, output path, filename, audio format, duration, current
  metadata, file details, selected candidate ID, status, stage, errors, retry
  metadata, and apply timestamps.
- `path` is unique.
- Rows are deleted after successful apply.

`candidates`

- Candidate metadata for tracks.
- Includes provider name, track/album/artist metadata, MusicBrainz IDs, cover
  URL, score, and raw provider JSON.
- References `tracks(id)` with `ON DELETE CASCADE`.

`jobs`

- Historical job-like table with ID, kind, status, progress, error, and
  timestamps.
- Current handlers mostly expose in-memory `Workflow` instead of using this
  table as the source of truth.

`scan_runs`

- Run history table for scan-level counters and status.
- Present in schema; current visible workflow is primarily in memory.

`apply_runs`

- Run history table for apply-level counters and status.
- Present in schema; current visible workflow is primarily in memory.

`settings`

- Key/value table.
- The full settings JSON is stored under key `config`.

`provider_cache`

- Provider response cache table with provider, cache key, response JSON, and
  expiration timestamp.
- Startup cleanup deletes expired rows.

### Migrations

Migration files:

- `0001_init.sql`: creates the base schema and indexes.
- `0002_v020.sql`: adds stage, stage message, retry count, next retry time, and
  updated time columns.
- `0003_v030.sql`: clears temporary workspace-related data.

The migration history shows the application uses the database as a temporary
workspace and that later versions intentionally clear stale scan/apply/provider
data.

### Startup Cleanup

On startup, `db::cleanup`:

- Marks processing tracks as failed with an interrupted-by-restart message.
- Deletes old tracks based on `workspace_retention_days`.
- Deletes old non-running jobs based on `job_retention_days`.
- Marks running jobs as failed.
- Deletes expired provider cache rows.

### Settings Persistence

`db::load_settings`:

1. Reads `settings.value` where key is `config`.
2. Deserializes it into `Config`, or falls back to defaults.
3. Restores runtime-only values from environment/defaults:
   - `db_path`
   - `acoustid_api_key`
   - `musicbrainz_user_agent`
4. Saves the normalized config back to SQLite.

This means editable settings survive restarts, while secrets and runtime paths
come from the environment and startup defaults.

## External Integrations

### fpcalc

`fpcalc` is an external command-line dependency. It must be installed in the
runtime environment. The backend uses it for acoustic fingerprint generation.

Failure modes:

- command timeout after 180 seconds
- non-zero exit status
- invalid JSON output

### AcoustID

AcoustID is optional. If no API key is configured, the scan logs a warning and
falls back to MusicBrainz text search.

When configured, AcoustID supplies recording IDs and confidence scores. The
backend then asks MusicBrainz for richer metadata.

### MusicBrainz

MusicBrainz is required for metadata enrichment and fallback search. The backend
requires a valid contact-style user agent.

Requests are rate-limited to one per second to respect MusicBrainz usage
expectations.

### Cover Art Archive

Cover Art Archive is used when a MusicBrainz release ID is available. Candidate
cover URLs are generated as:

```text
https://coverartarchive.org/release/<release_id>/front-500
```

Proposed artwork preview responses are cached on disk beside the SQLite
database, in an `artwork/` directory.

## Frontend Architecture

The frontend is a Vite React application in `frontend/`.

Important frontend dependencies from `frontend/package.json`:

- `react`
- `react-dom`
- `@tanstack/react-query`
- `vite`
- `typescript`
- `tailwindcss`
- `@vitejs/plugin-react`
- `eslint`
- `prettier`

Build scripts:

| Script | Purpose |
| --- | --- |
| `npm run dev` | Start Vite dev server. |
| `npm run build` | Type-check and build frontend assets. |
| `npm run lint` | Run ESLint. |
| `npm run format` | Rewrite files with Prettier. |
| `npm run format:check` | Check Prettier formatting. |

### Entrypoints

`frontend/src/app/main.tsx`

- Creates the React root.
- Installs `QueryClientProvider`.
- Renders `App`.
- Imports global styles from `frontend/src/styles/index.css`.

`frontend/src/app/App.tsx`

- Owns top-level UI state.
- Fetches settings and workspace data.
- Starts/stops scans.
- Creates apply previews.
- Starts apply.
- Switches between the settings screen and workflow workspace.
- Automatically requests an apply preview when workflow enters preview phase
  with matched tracks.

### API Layer

`frontend/src/api/client.ts`

- Wraps `fetch`.
- Prefixes all paths with `/api`.
- Sends `Content-Type: application/json`.
- Parses JSON.
- Throws `Error` when the response is not OK.

`frontend/src/api/types.ts`

- Mirrors backend-facing DTOs used by the UI:
  - `Candidate`
  - `Track`
  - `TrackPage`
  - `MetadataSummary`
  - `PreviewItem`
  - `Preview`
  - `TerminalLine`
  - `Workflow`

### Live Updates

`frontend/src/hooks/useEvents.ts` subscribes to:

```text
/api/events
```

through `EventSource`.

On each event:

- The hook parses the server event JSON.
- It updates the TanStack Query cache for `["workspace"]`.
- Terminal events append to `terminal_log` and keep the last 160 lines.
- Preview, finish, failed, and done-like events invalidate workspace and track
  queries.
- Connection state is exposed as `connecting`, `connected`, or `reconnecting`.

The frontend also polls `/api/workspace` every 1500 ms, which gives a fallback
state refresh path when SSE reconnects.

### Major Screens And Components

`SettingsPage`

- Edits settings grouped by tabs.
- Saves through `PUT /settings`.
- Resets full settings or sections.
- Uses `/path-template/preview` for live file path examples.

`Workspace`

- Chooses the main visible workflow view.
- Shows idle hero, processing view, failed view, or preview page depending on
  `workflow.phase`.

`Flow`

- Displays the pipeline phase progression.

`ProcessingCard`

- Displays active scan/fetch/apply progress and counters.

`Terminal`

- Displays backend terminal lines from `Workflow.terminal_log`.

`PreviewPage`

- Displays matched preview output, duplicate skip counts, apply controls, and
  completion state.

Preview feature components:

- `PreviewVirtualList`: renders the preview list efficiently.
- `PreviewRow`: compares current and proposed metadata.
- `MusicMetadataCard`: displays metadata and artwork for one side of a preview.
- `CoverImage`: renders current/proposed artwork.

Settings feature components:

- `SettingsSections.tsx`, `SettingsFields.tsx`, and `SettingsTypes.ts` define
  the settings tab content, field controls, choices, toggles, and metadata field
  groups.

### UI States

The frontend recognizes these workflow phases:

- `idle`: ready to start scanning.
- `scan`: discovering supported files.
- `fetch`: reading metadata, fingerprinting, and fetching provider data.
- `preview`: showing selected metadata changes before writing.
- `apply`: writing metadata and copying/renaming files.
- `finish`: apply completed.
- `failed`: workflow stopped because of an error.

## Functional Behavior

### Safe Defaults

The default mode is designed to avoid destructive writes:

- `output_mode = "copy"`
- input files are left unchanged
- output files are written under `output_dir`
- Expert Mode is disabled
- destructive settings are rejected unless Expert Mode is enabled

Destructive settings include:

- in-place output mode
- file rename
- folder rename
- overwrite destination collision behavior
- replacing existing cover art

### Copy Mode

Copy mode:

1. Computes a destination path under `output_dir`.
2. Creates parent directories.
3. Copies the source file.
4. Writes tags to the copy.
5. Removes the copied file if tag writing fails.
6. Deletes the workspace track row after successful apply.

### In-Place Mode

In-place mode:

1. Uses the source file as the write target.
2. Writes metadata directly to the original file.
3. Optionally preserves modification time.
4. Optionally renames the file or reorganizes folders.
5. Updates the track row path and filename when a rename occurs.

Because in-place mode edits originals, validation requires Expert Mode.

### Metadata Field Control

`MetadataFields` controls whether individual metadata fields are written.

`overwrite_existing_tags` controls whether enabled fields replace existing tag
values. When it is disabled, Ununknown only fills missing enabled values.

### Artwork Behavior

Artwork has separate controls:

- `cover_art_enabled`: whether the app downloads provider artwork.
- `metadata_fields.embed_cover_art`: whether downloaded artwork is embedded.
- `metadata_fields.replace_existing_cover_art`: whether existing cover art is
  removed before embedding.

Preview artwork endpoints are separate from apply-time artwork download.

## File And Directory Reference

Backend:

```text
src/main.rs
src/config.rs
src/jobs.rs
src/app/state.rs
src/application/scan_pipeline.rs
src/domain/audio.rs
src/domain/matcher.rs
src/domain/path_templates.rs
src/http/router.rs
src/http/handlers/mod.rs
src/http/error.rs
src/infrastructure/db/mod.rs
src/infrastructure/media/fingerprint.rs
src/infrastructure/media/tag_writer.rs
src/infrastructure/providers/acoustid.rs
src/infrastructure/providers/musicbrainz.rs
src/infrastructure/providers/cover_art_archive.rs
src/infrastructure/static_files.rs
```

Frontend:

```text
frontend/src/app/main.tsx
frontend/src/app/App.tsx
frontend/src/api/client.ts
frontend/src/api/types.ts
frontend/src/hooks/useEvents.ts
frontend/src/layouts/Workspace.tsx
frontend/src/layouts/Flow.tsx
frontend/src/layouts/ProcessingCard.tsx
frontend/src/layouts/Terminal.tsx
frontend/src/pages/SettingsPage.tsx
frontend/src/pages/PreviewPage.tsx
frontend/src/features/settings/
frontend/src/features/preview/
frontend/src/styles/index.css
```

Database:

```text
migrations/0001_init.sql
migrations/0002_v020.sql
migrations/0003_v030.sql
```

Manifests:

```text
Cargo.toml
Cargo.lock
frontend/package.json
frontend/package-lock.json
frontend/tsconfig.json
frontend/vite.config.ts
frontend/eslint.config.js
```

## Operational Notes

Runtime requirements:

- Rust backend binary.
- SQLite file path writable by the process.
- Input directory readable by the process.
- Output directory writable in copy mode.
- `fpcalc` available on `PATH`.
- Network access for AcoustID, MusicBrainz, and Cover Art Archive.
- Built frontend files in `frontend/dist` when using the backend static server.

Important current limitations:

- Preview tokens are in memory and are lost on restart.
- Workflow state is mostly in memory; database job/run tables are not the
  primary UI source of truth.
- API errors use typed HTTP status codes for validation, missing resources,
  workflow conflicts, provider failures, timeouts, config issues, and internal
  failures.
- Scan processing is sequential.
- `provider_cache` exists in schema and cleanup, but provider request code does
  not currently use it as a read-through cache.
- `wav`, `aiff`, and `aif` are discoverable but tag writing is intentionally
  skipped.
- Existing scan start clears the temporary tracks table and provider cache.

## Verification

This document describes the current source tree and does not require runtime
code changes.

Recommended checks after documentation edits:

```text
cargo test
cd frontend && npm run build
```

These checks are non-mutating with respect to source code, although they may
write normal build artifacts or caches.
