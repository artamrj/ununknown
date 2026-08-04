ALTER TABLE candidates ADD COLUMN artist_credits_json TEXT;
ALTER TABLE candidates ADD COLUMN album_artist_credits_json TEXT;
ALTER TABLE candidates ADD COLUMN artwork_candidates_json TEXT;
ALTER TABLE candidates ADD COLUMN artwork_status TEXT NOT NULL DEFAULT 'searching';
ALTER TABLE candidates ADD COLUMN artwork_message TEXT;

CREATE TABLE IF NOT EXISTS canonical_names (
  kind TEXT NOT NULL,
  identity_key TEXT NOT NULL,
  canonical_value TEXT NOT NULL,
  authority INTEGER NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY(kind, identity_key)
);
