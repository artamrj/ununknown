CREATE TABLE IF NOT EXISTS reference_files (
  path TEXT PRIMARY KEY,
  root TEXT NOT NULL,
  file_size INTEGER NOT NULL,
  file_mtime_ns INTEGER NOT NULL,
  duration REAL,
  fingerprint TEXT,
  file_hash TEXT,
  error TEXT,
  indexed_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS reference_files_fingerprint_duration
  ON reference_files(fingerprint, duration);
CREATE INDEX IF NOT EXISTS reference_files_size
  ON reference_files(file_size);
