# Backend Developer Guide

Use this guide when you want to edit the backend.

## Local Checks

Run the main test suite after backend changes:

```bash
cargo test
```

Useful extra checks:

```bash
cargo check
cargo fmt --check
```

When running without Docker, install Chromaprint so `fpcalc` is available.

## Add A New API Endpoint

1. Add the handler in the closest file under `src/http/handlers/`.
2. Re-export the handler from `src/http/handlers/mod.rs` if needed.
3. Add the route in `src/http/router.rs`.
4. Use `ApiResult<Json<T>>` for JSON handlers that can fail.
5. Use `ApiError::validation`, `not_found`, `conflict`, `provider`, `io`,
   `forbidden`, or `timeout` for expected failures.
6. Add tests near the behavior you changed.
7. Update `docs/backend/api.md`.

Keep HTTP handlers thin. Put reusable business rules in `src/domain/` and
workflow orchestration in `src/application/`.

## Add A New Setting

1. Add the field to `Config` in `src/config.rs`.
2. Add a default in `impl Default for Config` or the nested config struct.
3. Add validation in `Config::validate()` if invalid values can break behavior.
4. Decide whether the setting is public, secret, or deployment-owned.
5. Update settings reset behavior in `src/http/handlers/settings.rs` if the new
   field belongs to a reset section.
6. Update frontend API types if the UI reads or writes it.
7. Update `docs/backend/configuration.md`.
8. Run `cargo test`.

Secrets should not be returned directly through public settings.

## Add A Database Column Or Table

1. Add a new migration under `migrations/` with the next number.
2. Update SQL queries and `FromRow` structs.
3. Update any insert/update/select field lists.
4. Add tests with a migrated test database.
5. Update `docs/backend/database.md`.

Do not change old migrations once users may already have databases created from
them.

## Change Matching Or Scoring

Matching rules live in `src/domain/matcher.rs`. Provider candidate construction
lives under `src/infrastructure/providers/`.

When changing scoring:

- Keep scores on the same `0` to `100` scale unless you also update thresholds.
- Add focused tests for text similarity, duration handling, or threshold
  behavior.
- Check how `automation_mode` and `confidence_threshold` use the score during
  scan.
- Update `docs/backend/workflows.md` if the behavior changes.

## Change Output Path Naming

Path rendering lives in `src/domain/path_templates.rs`. Destination selection
for preview/apply lives in the apply handlers.

When changing templates:

- Preserve traversal protection.
- Preserve extension behavior.
- Preserve collision handling.
- Add tests for unsafe paths, missing values, and extensions.
- Update `docs/backend/configuration.md`.

## Add Or Adjust A Provider

Provider HTTP code belongs in `src/infrastructure/providers/`. Normalize
provider responses into `providers::Candidate` so the rest of the backend does
not depend on provider-specific JSON.

Use the shared `reqwest::Client` from `AppState`. Cache expensive or repeated
responses in `provider_cache` when useful.

Provider errors should become useful terminal events during scan/apply so the
frontend can show what failed.

## Debug Failed Scans

Check these first:

- Is `input_dir` correct and mounted into the container?
- Are the files supported by `domain::audio::is_supported`?
- Does `fpcalc` exist? In Docker, run `which fpcalc` inside the container.
- Is `UNUNKNOWN_ACOUSTID_API_KEY` configured if fingerprint matching is needed?
- Is `UNUNKNOWN_MUSICBRAINZ_USER_AGENT` valid?
- Look at `/api/workspace` and the terminal log from `/api/events`.

`POST /api/scan/start` clears the previous temporary track workspace. Do not use
it if you are trying to inspect old track rows.

## Debug Failed Applies

Check these first:

- Did you call `/api/apply/preview` and pass the returned token to
  `/api/apply/start`?
- Was the token already consumed or made stale by another action?
- In copy mode, can the backend create the output directory?
- In in-place mode, are files writable?
- Is Expert Mode enabled for destructive settings?
- Are collision settings causing destination failures?
- Did artwork download fail but tag writing continue?

Copy mode is the safer development mode. In-place mode edits source files.
