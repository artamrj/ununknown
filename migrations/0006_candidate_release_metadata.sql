ALTER TABLE candidates ADD COLUMN release_country TEXT;
ALTER TABLE candidates ADD COLUMN release_date TEXT;
ALTER TABLE candidates ADD COLUMN release_type TEXT;
ALTER TABLE candidates ADD COLUMN release_secondary_types TEXT;
ALTER TABLE candidates ADD COLUMN is_compilation INTEGER NOT NULL DEFAULT 0;
ALTER TABLE candidates ADD COLUMN duration_delta REAL;
ALTER TABLE candidates ADD COLUMN score_breakdown TEXT;
