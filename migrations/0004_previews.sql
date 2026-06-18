CREATE TABLE previews (
  token TEXT PRIMARY KEY,
  status TEXT NOT NULL,
  created_at TEXT NOT NULL,
  started_at TEXT,
  consumed_at TEXT,
  summary_json TEXT NOT NULL,
  settings_fingerprint TEXT
);

CREATE TABLE preview_items (
  preview_token TEXT NOT NULL REFERENCES previews(token) ON DELETE CASCADE,
  position INTEGER NOT NULL,
  track_id INTEGER NOT NULL,
  candidate_id INTEGER NOT NULL,
  duplicate_action TEXT NOT NULL,
  item_json TEXT NOT NULL,
  applied_at TEXT,
  error TEXT,
  PRIMARY KEY(preview_token, position)
);

CREATE INDEX preview_items_track_id ON preview_items(track_id);
CREATE INDEX previews_status ON previews(status);
