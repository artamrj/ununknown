# Ununknown — Technical Overview & End-to-End Algorithm

Ununknown is a single-user, local music metadata correction tool. It takes a folder
of music, reads every audio file, verifies it can be decoded, identifies it through a
cascade of online metadata providers, lets the user approve or correct uncertain
matches, and writes corrected copies (with ReplayGain and verified cover art) into an
output folder.

This document describes how the whole system works: the components, the data model,
and the exact algorithm the pipeline runs from first launch to final corrected file.

---

## 1. High-level architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                         Browser (React SPA)                          │
│Setup / Scan button / Review queue / Inspector / Write dock           │
└──────────────────────────────────▲───────────────────────────────────┘
                                   │  local HTTP  (127.0.0.1:7331)
┌──────────────────────────────────┴───────────────────────────────────┐
│                          Rust server (axum)                          │
│HTTP router  →  handlers  →  application services                     │
│                                                                      │
│  ┌─────────────────────────────────────────────────────────────┐     │
│  │ scan (run · process · providers · persist)                  │     │
│  │ input_dedup · smart_approval · metadata_completion · apply  │     │
│  └──────────────────────────────┬──────────────────────────────┘     │
│                                  │                                   │
│  ┌──────────────────────────────┴───────────────────────────────┐    │
│  │  Infrastructure layer                                        │    │
│  │  media: audio reader · integrity · fingerprint · replaygain ·│    │
│  │         tags · repair                                        │    │
│  │  providers: 20+ metadata/audio sources (see §7)              │    │
│  │  db: SQLite + repository (tracks/candidates) + caches        │    │
│  └──────────────────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────────────────┘
```

- **Frontend** (`frontend/`): a React SPA served by the same Rust process. It polls
  `/api/status` and `/api/tracks` and renders the review workspace.
- **Backend** (`src/`): Rust + axum + tokio + SQLite (`sqlx`). All heavy work
  (audio decoding, fingerprinting, network lookups, tag writing) runs in workers with
  bounded concurrency.
- **Storage**: one SQLite database plus audio/tag caches. No external services are
  required to run; metadata providers are queried over HTTP when configured/enabled.

The API is deliberately small: `setup`, `status`, `identify`, `tracks`, `write`
(`src/http/router.rs`). Production builds bind only to loopback and refuse
non-loopback bind addresses because the API has intentional access to local paths
(`src/main.rs:160`).

---

## 2. Data model

SQLite schema is versioned in `migrations/` and applied at startup.

| Table | Purpose |
|---|---|
| `tracks` | One row per input file: current tags, status, stage, content fingerprint, selected candidate, output path |
| `candidates` | One row per metadata suggestion from a provider; full field set + `score` + `score_breakdown` (JSON) |
| `candidate_sources` | Per-provider evidence rows for a candidate |
| `db::cache` | Keyed HTTP responses from metadata providers (disposable, cleared at midnight) |
| `db::cache` | Chromaprint result keyed by path+size+mtime |
| `integrity_cache` | Healthy/corrupt verdict keyed by path+size+mtime |
| `replaygain_cache` | Track gain/peak keyed by path+size+mtime |
| `content_hash_cache` | SHA-256 fallback for duplicate detection |
| `artwork_overrides` | User-confirmed cover URL per path |
| `automatic_scan_files` | Size+mtime snapshot so automatic scans only re-process changed files |
| `settings` / `maintenance` | Key/value configuration |

**Track lifecycle** (the `status`/`stage` columns drive the UI):

```
new/discovered → needs_review/review ──▶ selected/ready ──▶ applied (row deleted after write)
                        │                        │
                        ├─ duplicate/skipped     ├─ needs_review/review (returned, "Undo")
                        ├─ corrupt/failed        └─ corrupt/failed
                        └─ unmatched/review
```

---

## 3. Startup sequence

1. Resolve config (`UNUNKNOWN_*` env vars override saved settings), connect to
   SQLite, run migrations, apply pending cleanup.
2. Launch three background tasks (`src/main.rs`):
   - **Midnight cleanup** — deletes disposable provider responses / downloaded artwork
     while the app is idle.
   - **Hourly media-cache limit** — enforces a shared 100 MiB budget on
     fingerprint/integrity/ReplayGain caches (oldest first).
   - **Automatic cleaning scheduler** — event-driven sleep until the next interval,
     pauses while the frontend is open, resumes when the page closes.
3. Start the HTTP server on loopback, serving `/api/*` and the built SPA.

---

## 4. The end-to-end algorithm (user journey)

### Phase 0 — Setup

The user picks an input folder and an output folder and optionally pastes provider
credentials. Everything is stored in `settings` (credentials are stored there too,
but can instead be supplied via environment variables so they never touch disk).

### Phase 1 — Scan / identify (`POST /api/identify`)

`workers::scan::run` (`src/workers/scan.rs:39`):

1. Clear the previous workspace (`DELETE FROM tracks` + `automatic_scan_files`).
2. Walk the input folder **off the async runtime** (`spawn_blocking`) and collect all
   supported audio files, sorted by path.
3. For each file, in parallel (limited by `metadata` + `fingerprint` semaphores):
   - Read tags/audio properties with `audio::read`.
   - Check decodability with `integrity::check` (cached).
   - Compute a Chromaprint fingerprint (cached) or fall back to SHA-256.
4. **Duplicate detection** runs before any network lookup (see §5).

### Phase 2 — Per-track matching

Each duplicate group is processed as one "file job" (`scan::process::process_file` →
`process`, `src/workers/process.rs:3`). Two attempts are allowed per
track; transient provider failures retry with backoff.

For each member of the group:

1. **Read metadata** — title/artist/album/etc. plus format, bitrate, duration.
2. **Integrity check** — decode the stream. Corrupt files are marked `corrupt`,
   blocked from writing, and counted as failed. (A later "check & fix" step can
   salvage them via `media/repair.rs`.)
3. **Fingerprint** — `fpcalc` via `fingerprint::calculate`, cached by
   path+size+mtime. Failure is non-fatal; text/web sources still run.
4. **Identify** — run the provider cascade (§7) and collect candidates.
5. **Dedupe + sort candidates** by trust tier then score.
6. **Smart auto-selection** (§6) decides whether to auto-approve.
   - No candidates → `unmatched`, stays in Review with a human-readable reason.
   - Candidates exist but fail the strict rules → stays in Review with the exact
     reason (ambiguous release, conflicting album, duration mismatch, different
     performer, etc.).
   - A decision is made → run **metadata completion** (§8).
7. **Metadata completion worker** merges missing fields only from *agreeing*
   recordings and *compatible* releases, then verifies the actual cover image by
   downloading it (§8). If title/artist/album/cover are still missing, the track
   stays in Review instead of creating an incomplete identification.
8. Persist the track + candidates + selected candidate. Matched tracks go to
   **Ready**; everything else stays in **Review**.

The whole scan stream is written through a single DB writer task
(`workers::persist::db_writer`, `src/workers/persist.rs:56`) so SQLite is only
ever touched from one connection at a time.

### Phase 3 — Review loop (human in the loop)

Tracks that auto-selection refused stay in Review (`stage='review'`). The user can:

- **Choose a candidate** (`POST /api/tracks/{id}/choose`) — re-runs completion +
  cover verification for that candidate. The choice is accepted on recording
  identity alone (title + artist + album); the track becomes Ready even if no
  catalog cover could be verified, and the cover is retried at write time.
- **Smart auto-select** (`POST /api/tracks/auto-approve`) — bulk-runs the same
  `workers::approve::select` + completion + cover pipeline over all Review items.
  Ambiguous/low-confidence ones stay in Review.
- **Edit manually** (`PUT /api/tracks/{id}/manual`) — free-form editor; pasting a
  Shazam/Spotify/YouTube/etc. song link can auto-fill fields via
  `/api/source/resolve`. Creates a `manual` candidate (score 100).
- **Update artwork** (`PUT /api/tracks/{id}/artwork`) — accepts a cover URL or a
  supported song link, verifies the downloaded image, records an `artwork_overrides`
  row so the choice survives future rescans.
- **Undo identification** (`POST /api/tracks/{id}/review`) — clears the selected
  candidate without deleting saved candidates.
- **Remove the file** (`DELETE /api/tracks/{id}`) — deletes the source file *only if*
  it resolves inside the configured input folder, and promotes the best remaining
  duplicate input if one exists.
- **Check & fix issues** (`POST /api/tracks/retry-issues`) — re-runs the pipeline on
  `corrupt`/`failed`/`provider_error`/missing/retryable-cover tracks. Corrupt files
  first go through `repair::repair`, which skips unreadable frames, re-encodes the
  valid audio, and keeps the damaged original as a `.ununknown-damaged` backup.

### Phase 4 — Write corrected files (`POST /api/write`)

The **apply service** (`src/workers/apply.rs`, reached from the HTTP endpoint in
`src/http/handlers/apply.rs`):

1. **Prepare** (`prepare_apply`): load every `stage='ready'` track with its selected
   candidate, re-run completion/canonicalization at this last mutable boundary,
   re-group by recording evidence, pick one representative per group (promoting the
   best *available* input if the selected one is missing), and compute each
   destination filename. Duplicates become `DuplicateSource` entries on the item.
   When the candidate's artist credits are a strict subset of the source file's
   credits (a candidate that dropped featured artists or is featured-only), the
   fuller source credit wins — the same `merge_source_credits` rule already applied
   at scan selection, `/choose`, and auto-approve, so the review UI, the written
   ARTIST/ARTISTS tags, and the filename all keep every real performer.
2. **Per output** (`apply`, `apply.rs:722`):
   - **ReplayGain** — measure loudness with `replaygain::get_or_analyze` (cached).
     Failure never blocks the write.
   - **Artwork** — `resolve_artwork` fetches the verified cover bytes at write
     time (cached) via the shared cover-art worker. Artwork is **mandatory**: if no
     usable matching cover can be fetched, the write is aborted and the track is
     returned to Review instead of being written with missing or wrong art. Only
     artwork that matches the selected release is considered.
   - **Copy to a temporary file** in the destination directory
     (`.Name.ununknown-{id}.ext`).
   - **Write tags** — `tags::write_resilient` with ReplayGain tags, then
     `verify_written_metadata` and `verify_embedded_artwork`. Malformed legacy tags
     are removed with a lossless stream-copy and retried. Bounded by the
     `tag_writes` semaphore.
   - **Publish atomically** — `publish_no_clobber` (§9). Existing outputs are never
     overwritten or bulk-deleted; identical audio reuses an existing file instead of
     numbering.
   - **Optional source removal** — when `delete_source_after_write` is on, the
     original is deleted only *after* the corrected output exists. Safeguards refuse
     removal if input and output resolve to the same file.
   - Duplicate sources are finished the same way (`finish_duplicate`).
   - On success the `tracks` row is deleted (the job is done).

### Phase 5 — Automatic cleaning (optional)

Enabled by default every 5 minutes (`Settings → Automatic cleaning`). The scheduler
in `main.rs:79` sleeps between deadlines and wakes only on config change or an
automatic workflow completing. Each cycle:

1. Skips entirely while the frontend is open (`frontend_active_until`) or a workflow
   is running.
2. `workers::scan::run_automatic` (`src/workers/scan.rs:91`) walks the folder and
   compares each file's
   size+mtime against `automatic_scan_files` — unchanged files are skipped without
   decoding audio or touching providers.
3. Only strictly matched, complete tracks are written
   (`apply_ready_automatically`). Review candidates are never auto-approved.

---

## 5. Duplicate detection algorithm

`workers::dedup::group_recordings` (`src/workers/dedup.rs:51`) groups input
files using **proof-only evidence** (never title similarity):

- **Exact SHA-256** (`sha256:` key) — definitive, unioned unconditionally.
- **Chromaprint fingerprint** (`fp:` key) — unioned only when durations match within
  ±3 seconds.
- **ISRC** — unioned only when the 12-character normalized ISRC matches *and*
  durations are within ±3 seconds.

Group membership is computed with a disjoint-set union; within each group the
representative is the **best-quality** file: lossless formats first, then higher
bitrate, then stable path order (`compare_quality`).

Only the representative is matched online and produces one corrected output; the
others are recorded as `duplicate/skipped` and later collapsed into that single
output. Different audio, remixes, live versions, and materially different durations
never union, so they remain separate and receive numbered filenames when corrected
names collide.

The same grouping is re-run at **apply time** (over `RecordingEvidence` built from
the DB) so the workspace and the write queue always agree.

---

## 6. Smart auto-selection

`workers::approve::rank` (`src/workers/approve.rs:51`) scores every
candidate instead of blindly trusting the raw provider score:

**Score components** (weighted sum):

| Signal | Weight / effect |
|---|---|
| Title similarity (normalized Levenshtein) | ×28 (or 21 if no existing title) |
| Artist similarity | ×18 (or 15 if no existing artist) |
| Duration delta | +16 (≤3s), +13 (≤8s), +7 (≤15s), +1 (≤30s), −14 (>30s) |
| Album match to existing tag | similarity ×18, −8 if conflicting |
| Metadata completeness | album, cover, year, track no., genre, album artist, ISRC |
| Multi-source agreement | up to +15 |
| Provider trust | 0–5 |
| Source-credit coverage | −50 if the candidate drops any performer credited on the source file; +8 if it covers the full source credit set |
| Audio recognition bonus | +15 |
| Original album year bonus | +4 |
| Version tags (live/remix/acoustic/…) | −22 per unexpected, −16 per missing |
| Compilation penalty | −7 |

Release context is a hard gate, not only a score component. When the source file
has a meaningful album tag, a candidate from a different album or release edition
is retained for review but cannot be auto-approved. MusicBrainz recording lookup
first prefers a release whose title matches the source album, preventing a later
EP, compilation, or reissue from replacing the original album silently.

**Decision rules** — a candidate is auto-approved only when:
- It is *supported*: recognized by audio, corroborated by ≥2 sources, or an exact
  catalog match (credible provider + ≥0.90/≥0.82 title/artist + close duration).
- Its total is ≥ 68, and no *different* recording is within 12 points (ties with the
  same recording are tolerated; `same_recording` compares ISRC/version-tags/title).
- Its version tags match the filename's version tags.
- Its release context does not conflict with a meaningful source album.
- Its artist credits do **not drop any performer credited on the source file**
  (a −50 coverage penalty sends degraded credits back to Review; the fuller
  candidate — e.g. `Ali Azimi feat. Golshifteh Farahani` over `Golshifteh Farahani`
  — wins).

Tracks that fail stay in Review with a specific human reason (this is why "Auto
approve" never writes a questionable release, a duet over a plain recording, a
stripped version over an album version, or a wrong album).

---

## 7. Provider cascade (identification)

`core::identify::identify` (`src/core/identify.rs:3`) runs a staged
cascade — the "catalog runner". Order matters — cheap/free/authoritative sources run
first, expensive or optional ones only when needed:

1. **AcoustID** (fingerprint lookup) → returns MusicBrainz recordings.
2. **YouTube Data API** — only if a filename contains an exact video ID.
3. **MusicBrainz text search** (title/artist).
4. **Apple Music (iTunes)**, **Deezer**, **Radio Javan**, **Audiomack**, **Navahang**
   — free catalogs, good for Persian + international music.
5. **Genius** — only when enrichment is still needed.
6. **SongRec / Shazam** (audio recognition) — only when AcoustID found nothing and
   no ≥90-score candidate exists; recognition is serialized (max 2 concurrent) and
   cached. Its artist/title is then **fed back through the catalogs** to find album,
   artwork, credits, release info, ISRC.
7. **AudD** — only if SongRec also failed (needs a token).
8. **Spotify** — ISRC cross-check plus release/artwork/track-number enrichment.
9. **SoundCloud** (needs credentials), then **Discogs**, **Last.fm**,
   **TheAudioDB**, **Wikidata** (also supplies artist genres).

Post-processing per track:
- Credits are normalized (`workers::canonical`, `domain/credits`). A `& X` tail that
  duplicates a title-featured credit is reconciled, and ambiguous `X & Y` credits
  split into separate artists only when the `workers::canonical` evidence knows each
  side as a real artist — MBID-bearing groups (e.g. "Selena Gomez & the Scene")
  always stay whole.
- Candidates are canonicalized (alias/prefer-latin names).
- **Source agreement** (`apply_source_agreement`) boosts corroborating rows.
- Artwork fallbacks + `artwork_overrides` are applied.
- Genres are enriched from Wikidata artist lookups when needed.
- The `score` stored per candidate is the final blended score with a JSON
  `score_breakdown` (sources, duration delta, audio-recognition flag, artwork list).

All HTTP goes through `net` (timeouts, retries) and a
`db::cache`; MusicBrainz requests are rate-limited to one per second.

---

## 8. Metadata completion & cover verification

`workers::complete::complete` (`src/workers/complete.rs:32`):

1. Collect **agreeing recordings** (same ISRC, or same version tags + ≥0.90 title +
   ≥0.82 artist + compatible duration), ranked by donor quality.
2. **Never replace** a field that already has a value — only fill missing ones.
3. Copy release-specific fields (album, album artist, track/disc number, dates,
   label, country) **only from albums compatible with the chosen release** so a
   compilation can't silently change the original release.
4. Normalize release fields: `Single`/`EP` naming and album artist defaults.
   Featured performers remain track artists and are not copied into `album_artist`.
   Non-compilation album artists are derived from primary/co-primary credits.
5. `audit` produces a weighted completeness score plus the readiness flag
   `core_complete` (title, artist, album, **verified cover**).

**Artist credits** are resolved through a single pipeline
(`domain::credits::finalize_credits`): provider credits are parsed (title featured
credits moved into the artist list), then `prefer_source_credits` keeps the fuller
source-file credit when the candidate is a strict subset (constituent `&`-expanded
comparison, so a split `Koorosh` + `Sami Low` and an unsplit `Koorosh & Sami Low`
compare equal). `workers::canonical::canonicalize_candidates` then resolves ambiguous
bare `&` credits against persisted library evidence (MBID-bearing groups stay
whole, e.g. `Selena Gomez & the Scene`) and canonicalizes spelling. Every path to
Ready — scan, `/choose`, auto-approve, manual entry, and apply — funnels the
selected candidate through this same finalize step.

**Cover-art worker** (`src/workers/artwork.rs`):

- `collect_artwork_candidates` gathers every allowed URL for the selected
  recording: the candidate's own artwork list + `cover_url`, `score_breakdown`
  artwork alternatives, and **artwork already verified for the same release or
  ISRC elsewhere in the library** (`library_verified_artwork`). Bytes are shared
  through the artwork-url provider cache, so the first verified cover of an album
  becomes available to every other track on the same release.
- Each candidate image must *match the selected release* (release ID, ISRC+album,
  or compatible album+artist) unless user-confirmed. It is downloaded and
  validated (`cover_art_archive` / `inspect_artwork`), rejecting broken, blank,
  corrupted, or too-small images.
- `ensure_usable_cover` is the scan/selection worker. On temporary failures
  (timeout/429/5xx) it marks `retryable_error`; if the stored URLs are unusable it
  refreshes once from Deezer-by-ISRC + iTunes/Deezer, then checks the library
  again. Only when nothing verifies does it declare the cover missing
  (`cover_required`).
- `resolve_for_write` is the apply-time fetch: it returns the exact bytes to embed
  or fails, so a file is never written without valid matching artwork.

**Readiness**: `core_complete` — which requires a *verified* cover — is the bar for
every path to Ready: automatic write, explicit `/choose`, manual entry, artwork
update, and the on-demand `/tracks/{id}/artwork/search` worker. A track with a
missing, broken, or merely retryable cover stays in Review; the UI offers a
"Search for cover art" action that re-runs the worker for that track.

---

## 9. Atomic, no-clobber publication

`publish_no_clobber` (`src/workers/apply.rs:644`):

1. `fsync` the finished temporary file.
2. List existing destination variants (`Name.ext`, `Name (2).ext`, …).
3. If an existing output is **equivalent audio** (same file size + SHA-256, or same
   Chromaprint fingerprint + ≤3s duration) **and already carries the exact artwork
   that is about to be published**, reuse it and delete the temporary — never
   creating a numbered copy. A stale output with missing or different cover art is
   not reused; the fresh temporary is published instead, so reused files always
   have valid matching artwork.
4. Otherwise create a **hard link** from the temporary to the next free numbered
   destination. `AlreadyExists` bumps the number. The source of the write is
   excluded from equivalence checks so re-running your own output isn't mistaken for
   a different recording.

This makes publication atomic, never clobbers existing files, and never creates
needless duplicates.

---

## 10. Concurrency, rate limiting, and lifecycle

Bounded concurrency lives in `AppState` (`src/core/state.rs`) and
`workers::scan::PipelineLimits` (`src/workers/scan.rs:532`):

| Resource | Limit |
|---|---|
| `scan_workers` (tag readers / scan jobs) | configurable, default 6 |
| `fingerprint_workers` | default 3 |
| `lookup_workers` (metadata lookups + artwork downloads) | default 3 |
| SongRec recognition | hard cap 2 |
| `write_workers` (tag writes) | default 2 |
| MusicBrainz HTTP | 1 req/s |

- A single DB **writer task** serializes persistence during scans; apply uses
  per-operation transactions.
- **Cancellation**: `POST /api/stop` sets a `cancelled` flag that every worker checks
  at safe boundaries; SIGTERM/Ctrl+C asks the active workflow to stop, waits up to
  30 s, drains HTTP, and closes SQLite.
- **Frontend coordination**: the SPA POSTs `/api/activity` every 30 s. While the
  page is open, automatic cleaning is suspended and an in-flight automatic cycle is
  cancelled at its next safe boundary, keeping background work out of active review
  sessions.

---

## 11. Caching summary

| Cache | Key | Invalidated by |
|---|---|---|
| `db::cache` | provider + search key | expiry (disposable, midnight cleanup) |
| `db::cache` | path + size + mtime | file change / cache-limit eviction |
| `integrity_cache` | path + size + mtime | explicit retry deletes the row |
| `replaygain_cache` | path + size + mtime(ns) | file change |
| `content_hash_cache` | path + size + mtime(ns) | file change |
| `artwork_overrides` | path | overwritten when user confirms new cover |

Fingerprint/integrity/ReplayGain caches share a 100 MiB budget; the oldest entries
are evicted first (checked at startup and hourly while idle).

---

## 12. API reference

| Method & path | Purpose |
|---|---|
| `GET  /api/setup` | Read current config |
| `PUT  /api/setup` | Save folders, automatic-cleaning settings, provider keys |
| `GET  /api/status` | Workflow phase/progress + matched/review/failed counts |
| `POST /api/activity` | Heartbeat; cancels automatic work while UI is open |
| `POST /api/identify` | Clear workspace, scan + identify everything |
| `POST /api/stop` | Cancel current scan/write at a safe boundary |
| `GET  /api/tracks` | All tracks + candidates |
| `DELETE /api/tracks/{id}` | Remove a review file from disk (input folder only) |
| `POST /api/tracks/retry-issues` | Repair + re-run failed/corrupt/retryable tracks |
| `POST /api/tracks/auto-approve` | Bulk smart auto-select over Review |
| `GET  /api/tracks/{id}/audio` | Range-stream the source file for playback |
| `POST /api/tracks/{id}/choose` | Accept a candidate |
| `POST /api/tracks/{id}/review` | Undo identification (back to Review) |
| `PUT  /api/tracks/{id}/manual` | Save a manually entered candidate |
| `PUT  /api/tracks/{id}/artwork` | Set + verify a cover URL / song link |
| `POST /api/tracks/{id}/artwork/search` | Re-run the cover-art worker for a track |
| `POST /api/source/resolve` | Resolve a Shazam/Spotify/YouTube/… link to metadata |
| `GET  /api/candidates/{id}/artwork/preview`, etc. | Artwork previews |
| `POST /api/write` | Write all Ready tracks to the output folder |

---

## 13. Key source files

| Area | File |
|---|---|
| Entry point, startup tasks, graceful shutdown | `src/main.rs` |
| HTTP routes / API contract | `src/http/router.rs`, `src/http/handlers/*` |
| Scan + identify pipeline (walk, file jobs, cascade, DB writer) | `src/workers/` (`scan.rs`, `process.rs`, `persist.rs`) + `src/core/` (`identify.rs`, `scoring.rs`) |
| Catalog runner (provider cascade) | `src/core/identify.rs` |
| Duplicate detection | `src/workers/dedup.rs` |
| Smart auto-selection | `src/workers/approve.rs` |
| Metadata completion | `src/workers/complete.rs` |
| Cover-art worker (search, verify, reuse, apply-time fetch) | `src/workers/artwork.rs` |
| Apply service (prepare / write / publish) | `src/workers/apply.rs` (+ endpoint `src/http/handlers/apply.rs`) |
| Review / scan / workspace handlers | `src/http/handlers/tracks.rs`, `scan.rs`, `workspace.rs`, `queries.rs` |
| Repository (track + candidate types and queries) | `src/db/queries.rs` |
| Audio read / integrity / fingerprint / replaygain / tag write / repair | `src/media/*` |
| Provider integrations | `src/providers/*` |
| App state, workflow, limits | `src/core/state.rs`, `src/workers/scan.rs` |
| Configuration | `src/config.rs` |
| SQLite schema | `migrations/*.sql` |
| React UI | `frontend/src/app/App.tsx` |

---

## 14. Summary flow

```
Setup → Scan & identify
  ├─ walk folder → read tags → integrity check → fingerprint → dedup group
  ├─ provider cascade → candidates → smart auto-select
  │     ├─ auto-approved → metadata completion → cover verified → READY
  │     └─ uncertain/unmatched/corrupt → REVIEW
  └─ (automatic mode: only new/changed files, only strict matches)
Review → choose / auto-select / manual / source-link / artwork / retry-issues
  ├─ every path to READY requires core_complete, incl. a verified cover
  ├─ cover-art worker searches catalogs + already-verified same-release covers
  └─ artwork/search re-runs the worker for a single track
Write → per track: ReplayGain → fetch verified cover (mandatory) → copy to temp
      → write+verify tags → atomic no-clobber publish (reuse only if cover matches)
      → (optional) remove original → delete row
```
