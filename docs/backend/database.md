# Backend Database

Ununknown uses SQLite through `sqlx`. The database path comes from
`UNUNKNOWN_DB`, defaulting to `/cache/ununknown.sqlite`.

`src/infrastructure/db/mod.rs` opens the database with `mode=rwc`, creates the
parent directory if needed, and runs embedded migrations with `sqlx::migrate!()`.

## Tables

### `tracks`

One row per discovered audio file in the temporary workspace.

Important columns:

- `path`: canonical source file path.
- `output_path`: destination path when known.
- `filename`, `format`, `bitrate`, `duration`: file facts.
- `current_*`: metadata read from existing tags.
- `selected_candidate_id`: chosen candidate for preview/apply.
- `status`, `stage`, `stage_message`: processing state.
- `retry_count`, `next_retry_at`: retry tracking.
- `is_missing`: marks files that disappeared.
- timestamps for first seen, last seen, scan, apply, and update times.

`POST /api/scan/start` currently deletes all rows from `tracks` before scanning.

### `candidates`

Provider metadata candidates for each track.

Important columns:

- `track_id`: owning track.
- `provider`: source provider.
- metadata fields such as title, artist, album, track number, IDs, and cover URL.
- `score`: matching confidence.
- `raw_json`: provider response or normalized raw data.

Deleting a track cascades candidate deletion.

### `settings`

Stores saved settings as key/value text. The main key is `config`, whose value
is serialized JSON from `Config`.

Deployment-owned values such as DB path, AcoustID key, and MusicBrainz user
agent are restored from environment-derived defaults when settings are loaded.

### `provider_cache`

Caches provider responses by `(provider, cache_key)` until `expires_at`.

Cache keys are built in `src/infrastructure/provider_cache.rs` for fingerprints,
recording IDs, search queries, and release IDs.

Startup cleanup deletes expired provider-cache rows.

### `previews`

Stores dry-run apply previews.

Important columns:

- `token`: UUID returned by `/api/apply/preview`.
- `status`: `ready`, `started`, `consumed`, or `stale`.
- timestamps for creation, start, and consumption.
- `summary_json`: dry-run summary.
- `settings_fingerprint`: serialized settings at preview time.

Ready previews become stale when `previews::invalidate()` runs, such as when a
new scan starts.

### `preview_items`

Stores each item in a preview token.

Important columns:

- `preview_token`: owning preview token.
- `position`: order in the dry run.
- `track_id`, `candidate_id`: selected write target.
- `duplicate_action`: `none`, `keep`, or `skip_duplicate`.
- `item_json`: serialized `PreviewItem`.
- `applied_at`, `error`: apply result tracking fields.

`previews::consume()` loads only items whose duplicate action is not
`skip_duplicate`.

### `jobs`, `scan_runs`, `apply_runs`

These tables exist for run history and compatibility. The current HTTP job
endpoints mostly expose in-memory `Workflow` state instead of detailed persisted
job rows.

Startup cleanup marks running jobs as failed and removes old non-running jobs.

## Migrations

Migrations live in `migrations/`:

- `0001_init.sql`: initial schema.
- `0002_v020.sql`: track stages, retry fields, updated timestamp, and cleanup of
  old temporary data.
- `0003_v030.sql`: clears temporary workspace/provider data for that version.
- `0004_previews.sql`: persisted preview tokens and preview items.

To add schema:

1. Create a new numbered migration, for example `0005_add_field.sql`.
2. Use `ALTER TABLE` or `CREATE TABLE`.
3. Update query structs and SQL in `src/http/handlers/` or
   `src/application/`.
4. Add or update tests that open a test pool through `infrastructure::db::connect`.
5. Run `cargo test`.

Do not edit old migrations after they have been used by other databases. Add a
new migration instead.
