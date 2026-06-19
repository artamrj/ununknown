CREATE TABLE fingerprint_cache (
  path TEXT PRIMARY KEY,
  file_size INTEGER NOT NULL,
  file_mtime INTEGER NOT NULL,
  fingerprint TEXT NOT NULL,
  duration REAL NOT NULL,
  updated_at TEXT NOT NULL
);
