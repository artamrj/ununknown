# Backend Workflows

The backend has two main long-running workflows: scan and apply. Both update
shared workflow state and emit events to `/api/events`.

## Workflow State

`AppState` owns a `workflow: RwLock<Workflow>`.

`Workflow` contains:

- `phase`: `idle`, `scan`, `fetch`, `preview`, `apply`, `finish`, or `failed`.
- `message`: status text shown by the frontend.
- `current_file`: file currently being processed.
- progress counters: `current`, `total`, `processed`, `matched`, `unmatched`,
  and `failed`.
- `activity_log`: recent activity log lines.
- `cancelled`: internal flag used by scan/apply stop endpoints.

`AppState::log_entry()` appends an activity log line, keeps the latest 500
lines, and emits an activity_log event.

## Scan

The scan starts at `POST /api/scan/start`.

Before the background task starts, the handler:

1. Rejects the request if another workflow is running.
2. Deletes existing `tracks`.
3. Invalidates ready previews.
4. Resets workflow state.
5. Spawns `scan_pipeline::run()`.

`src/application/scan_pipeline.rs` then:

1. Reads config.
2. Walks `input_dir` without following symlinks.
3. Keeps supported audio files from `domain::audio::is_supported`.
4. Sorts the canonical file paths.
5. Starts scan workers and one DB writer task.
6. For each file, reads tags, fingerprints audio, calls providers, scores
   candidates, and persists the selected result.

Worker limits come from config:

- `scan_worker_concurrency`
- `metadata_read_concurrency`
- `fingerprint_concurrency`
- `acoustid_concurrency`
- `db_write_batch_size`

Metadata reading uses `spawn_blocking` because media tag reading is local
blocking I/O. Fingerprinting runs `fpcalc -json`, so `fpcalc` must exist when
running outside Docker.

## Provider Lookup

AcoustID lookup is used when an API key is configured. It maps an audio
fingerprint and duration to possible MusicBrainz recording IDs.

MusicBrainz is then used to fetch recording metadata or search by existing tag
text. The MusicBrainz user agent must include contact details.

Cover Art Archive is used during apply or artwork preview when a candidate has a
cover URL. Release-based artwork fetches can be cached through `provider_cache`.

## Matching And Stages

`src/domain/matcher.rs` scores provider candidates against local audio
information. The score uses title/artist text similarity and duration distance.

Track stages are:

- `discovered`: found but not ready.
- `ready`: selected candidate is good enough to apply.
- `review`: candidate needs user review.
- `skipped`: user chose to skip.
- `failed`: processing failed.

Automation mode and confidence threshold decide how aggressive automatic
selection should be.

## Preview

`POST /api/apply/preview` collects tracks with selected candidates and builds a
dry-run list.

Preview generation:

1. Loads selected tracks and candidates.
2. Computes duplicate actions.
3. Renders destination paths with current settings.
4. Adds warnings, artwork action, and old/new metadata summaries.
5. Stores the preview and preview items in SQLite.
6. Returns a UUID `preview_token`.

Preview tokens are single-use. `previews::consume()` changes a `ready` preview
to `started` inside a transaction before returning items to apply. Missing,
stale, already consumed, or invalid previews are rejected.

## Apply

`POST /api/apply/start` consumes a preview token, switches the workflow to
`apply`, and spawns the apply task.

For each preview item, apply:

1. Checks cancellation.
2. Loads the selected candidate.
3. Downloads cover art when enabled and available.
4. Chooses the target path:
   - copy mode copies the source file to the destination first.
   - in-place mode writes to the source path unless rename settings say
     otherwise.
5. Writes tags through `src/infrastructure/media/tag_writer.rs`.
6. Uses semaphores to limit artwork downloads and tag writes.
7. Emits activity log and workflow events.

Some formats, such as WAV and AIFF, are treated cautiously in preview warnings.

## Cancellation

`POST /api/scan/stop` and `POST /api/apply/stop` set the workflow cancellation
flag. Workers check that flag between steps. Cancellation is cooperative, so
work already inside a provider request, copy operation, or tag write may finish
before the workflow stops.

## Events

`/api/events` streams `jobs::Event` values as server-sent events. Events are
sent for workflow progress and activity log entries.

The frontend uses this stream to update progress bars, counts, current file,
and activity log output without repeatedly polling the workspace endpoint.
