ALTER TABLE tracks ADD COLUMN stage TEXT NOT NULL DEFAULT 'discovered';
ALTER TABLE tracks ADD COLUMN stage_message TEXT;
ALTER TABLE tracks ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tracks ADD COLUMN next_retry_at TEXT;
ALTER TABLE tracks ADD COLUMN updated_at TEXT;

CREATE INDEX tracks_stage ON tracks(stage);

DELETE FROM candidates;
DELETE FROM tracks;
DELETE FROM jobs;
DELETE FROM scan_runs;
DELETE FROM apply_runs;
DELETE FROM provider_cache;
