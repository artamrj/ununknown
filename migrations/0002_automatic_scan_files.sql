CREATE TABLE IF NOT EXISTS automatic_scan_files (
  path TEXT PRIMARY KEY,
  file_size INTEGER NOT NULL,
  file_mtime_ns INTEGER NOT NULL,
  checked_at TEXT NOT NULL
);
