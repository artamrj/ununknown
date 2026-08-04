DELETE FROM automatic_scan_files
WHERE path IN (
  SELECT path FROM tracks WHERE status = 'duplicate' AND stage = 'skipped'
);

DELETE FROM tracks WHERE status = 'duplicate' AND stage = 'skipped';

DROP TABLE IF EXISTS reference_files;

CREATE TABLE IF NOT EXISTS content_hash_cache (
  path TEXT PRIMARY KEY,
  file_size INTEGER NOT NULL,
  file_mtime_ns INTEGER NOT NULL,
  sha256 TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
