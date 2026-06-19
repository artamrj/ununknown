CREATE TABLE candidate_sources (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  candidate_id INTEGER NOT NULL REFERENCES candidates(id) ON DELETE CASCADE,
  provider TEXT NOT NULL,
  confidence REAL,
  reason_json TEXT,
  raw_json TEXT,
  created_at TEXT NOT NULL
);

CREATE INDEX candidate_sources_candidate_id ON candidate_sources(candidate_id);
