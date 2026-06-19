# Backend API

All backend routes are mounted under `/api` in `src/http/router.rs`. Most
responses are JSON. `/api/events` is a server-sent events stream.

## Health

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/health` | Returns `{ "status": "ok" }`. |

## Settings

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/settings` | Returns public settings plus provider configured flags. |
| `PUT` | `/api/settings` | Validates and saves settings. |
| `POST` | `/api/settings/reset` | Resets settings while preserving deployment-owned paths and secrets. |
| `POST` | `/api/settings/reset/{section}` | Resets one section: `matching`, `metadata`, or `files`. |
| `POST` | `/api/providers/acoustid/test` | Checks that an AcoustID key is configured. |
| `POST` | `/api/providers/musicbrainz/test` | Performs a MusicBrainz test request. |

Settings requests use the `Config` shape from `src/config.rs`. Secret values are
kept server-side.

## Workspace

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/workspace` | Returns current workflow state and matched track counts. |
| `POST` | `/api/workspace/clear` | Deletes temporary workspace data and resets workflow state. |

Clearing the workspace deletes tracks, candidates, jobs, provider cache,
previews, and preview items.

## Scan And Jobs

| Method | Path | Purpose |
| --- | --- | --- |
| `POST` | `/api/scan/start` | Starts a background scan workflow. |
| `POST` | `/api/scan/stop` | Requests cancellation of the running scan. |
| `GET` | `/api/jobs` | Returns the current workflow state as a job-like object. |
| `GET` | `/api/jobs/{id}` | Returns the current workflow state. The current implementation ignores `id`. |

`POST /api/scan/start` clears existing `tracks`, invalidates ready previews,
resets workflow state, and starts `scan_pipeline::run()` in a Tokio task.

## Tracks And Candidates

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/tracks` | Lists tracks with candidates, counts, pagination, status filter, and search. |
| `GET` | `/api/tracks/{id}` | Returns one track. |
| `GET` | `/api/tracks/{id}/candidates` | Returns candidates for one track. |
| `POST` | `/api/tracks/{id}/select-candidate` | Selects or clears the chosen candidate. |
| `PUT` | `/api/candidates/{id}` | Edits a candidate's metadata fields. |
| `POST` | `/api/tracks/{id}/retry` | Marks one failed/review track for retry. |
| `POST` | `/api/tracks/bulk/retry` | Marks failed tracks for retry. |
| `POST` | `/api/tracks/bulk/skip` | Skips review tracks. |

Track stages use the wire values from `TrackStage`: `discovered`, `ready`,
`review`, `skipped`, and `failed`.

## Artwork

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/artwork/current/{id}` | Reads embedded artwork from the current source file. |
| `GET` | `/api/artwork/proposed/{id}` | Fetches proposed candidate artwork. |

Current artwork comes from local audio tags. Proposed artwork comes from the
candidate cover URL and may use provider cache.

## Preview And Apply

| Method | Path | Purpose |
| --- | --- | --- |
| `POST` | `/api/path-template/preview` | Renders configured or custom templates against a sample or real track. |
| `POST` | `/api/apply/preview` | Builds a dry-run list and stores a single-use preview token. |
| `POST` | `/api/apply/start` | Consumes a preview token and starts applying changes. |
| `POST` | `/api/apply/stop` | Requests cancellation of the running apply workflow. |

`/api/apply/start` requires:

```json
{
  "preview_token": "00000000-0000-0000-0000-000000000000"
}
```

A preview can fail to start if it is missing, stale, already consumed, or not
usable. The apply workflow writes only items whose duplicate action is not
`skip_duplicate`.

## Events

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/events` | Streams workflow and activity log events with server-sent events. |

Events use the `jobs::Event` shape:

- `kind`: event kind, such as workflow or activity_log.
- `stage`: scan, fetch, metadata, artwork, apply, or similar.
- `phase`: workflow phase when present.
- `current`, `total`, and `message`: progress values.
- Optional fields such as `file`, `level`, `error`, `detail`, `attempt`,
  `duration_ms`, and `context`.

The frontend subscribes to this stream so progress updates appear without
polling.
