# AGENTS.md

Important: /caveman active

## What this is

Ununknown: a single-user, local-only music metadata corrector. A Rust backend (axum + tokio + sqlx/SQLite, edition 2024) serves a React 19/Vite/TS SPA and runs the pipeline: scan → identify via a provider cascade → human review → write tagged copies. `docs/TECHNICAL-OVERVIEW.md` is the authoritative design doc (pipeline phases, scoring, invariants, file-level map) — read it before touching pipeline behavior and keep it in sync with behavioral changes.

## Commands

- `./dev.sh` — dev mode: `cargo watch -x run` on `127.0.0.1:7331` + Vite on `localhost:5173` (proxies `/api` → 7331). Requires `cargo-watch` (`cargo install cargo-watch`), npm, curl. Dev data lives under `.local/` (gitignored). Safe to re-run; it detects already-running servers.
- CI gate (exactly this; there is no clippy gate):
  - `cargo fmt --all -- --check`
  - `cargo test --locked`
  - `npm --prefix frontend ci && npm --prefix frontend run format:check && npm --prefix frontend run lint && npm --prefix frontend run build`
- `cargo test <name>` for a single test. All tests are inline `#[cfg(test)] mod tests` in the source files; there is no separate test tree.
- `./scripts/build-release.sh` — full release archive into `dist/` (runs every check above plus a `--release --locked` build).

## External binaries

Runtime and tests shell out to `ffmpeg`/`ffprobe` (integrity check, ReplayGain, tag write/repair) and `fpcalc` (Chromaprint fingerprints); install with `brew install ffmpeg chromaprint`. ffmpeg-dependent tests self-skip when the binary is missing; CI installs ffmpeg before `cargo test`. SongRec is optional via `UNUNKNOWN_SONGREC_BIN`.

## Layout and wiring

- `src/http/` — axum router/handlers (deliberately small API) · `src/application/` — pipeline: `scan/`, `apply.rs`, `smart_approval.rs`, `metadata_completion.rs`, `artwork.rs`, `input_dedup.rs` · `src/infrastructure/` — media tools, ~20 providers under `providers/`, SQLite + caches · `src/domain/` — pure logic · `src/app/state.rs` — `AppState` with the concurrency semaphores · `src/config.rs` — env/settings.
- Migrations are embedded at compile time with `sqlx::migrate!("./migrations")`. Add a new `migrations/NNNN_name.sql` file (next number) — applied at startup. No `DATABASE_URL`/`.sqlx` is needed to build.
- All scan persistence goes through one DB writer task (`scan::persist::db_writer`); do not add other write connections in the scan path. Apply uses per-operation transactions.
- `UNUNKNOWN_*` env vars override settings stored in SQLite; provider credentials via env never touch disk (full list in README).

## Hard constraints — do not weaken

- **Loopback only.** The server refuses non-loopback `UNUNKNOWN_BIND` unless `UNUNKNOWN_ALLOW_NON_LOOPBACK=true` (a Docker-only exception; the published host port must stay loopback). The API accepts local filesystem paths and deletes input files, with no auth.
- **No-clobber writes.** Publication is atomic and never overwrites or bulk-deletes existing outputs (`publish_no_clobber`). Verified artwork is mandatory: no matching cover, no write — the track goes back to Review.
- **Proof-only dedup.** Duplicate grouping uses only SHA-256, Chromaprint+duration, or ISRC+duration evidence — never title similarity.
- **Automatic cleaning never approves.** Scheduled cycles write only strictly matched tracks; Review candidates are never auto-selected.

## Release and dependency conventions

- Keep `version` in `Cargo.toml` and `frontend/package.json` in sync. Pushing a `vX.Y.Z` tag (or manual `workflow_dispatch`) is the only release path: it builds both arch images, Trivy-scans them, publishes multi-arch GHCR tags, and creates the GitHub Release. Normal pushes/PRs run only the fast checks.
- `.trivyignore.yaml` exceptions are time-limited; the release build fails when one expires.
- Dependabot is grouped and monthly. TypeScript major bumps are intentionally ignored (typescript-eslint 8 requires TS <6.1) — do not force-upgrade TypeScript.
- `cargo audit` ignores RUSTSEC-2023-0071 via `.cargo/audit.toml` (`rsa` only appears through sqlx's unbuilt MySQL feature).
