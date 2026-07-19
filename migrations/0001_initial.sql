CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS maintenance (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS tracks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  path TEXT NOT NULL UNIQUE,
  output_path TEXT,
  filename TEXT NOT NULL,
  format TEXT,
  bitrate INTEGER,
  duration REAL,
  current_title TEXT,
  current_artist TEXT,
  current_album TEXT,
  current_album_artist TEXT,
  current_track_number INTEGER,
  file_mtime INTEGER,
  file_size INTEGER,
  content_fingerprint TEXT,
  selected_candidate_id INTEGER,
  status TEXT NOT NULL DEFAULT 'new',
  error TEXT,
  is_missing INTEGER NOT NULL DEFAULT 0,
  first_seen_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL,
  last_scanned_at TEXT NOT NULL,
  last_applied_at TEXT,
  last_apply_run_id TEXT,
  stage TEXT NOT NULL DEFAULT 'discovered',
  stage_message TEXT,
  retry_count INTEGER NOT NULL DEFAULT 0,
  next_retry_at TEXT,
  updated_at TEXT
);

CREATE TABLE IF NOT EXISTS candidates (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
  provider TEXT NOT NULL,
  title TEXT,
  artist TEXT,
  album TEXT,
  album_artist TEXT,
  track_number INTEGER,
  track_total INTEGER,
  disc_number INTEGER,
  disc_total INTEGER,
  year TEXT,
  genre TEXT,
  composer TEXT,
  label TEXT,
  isrc TEXT,
  cover_url TEXT,
  musicbrainz_recording_id TEXT,
  musicbrainz_release_id TEXT,
  musicbrainz_artist_id TEXT,
  musicbrainz_album_artist_id TEXT,
  score REAL NOT NULL,
  raw_json TEXT,
  release_country TEXT,
  release_date TEXT,
  release_type TEXT,
  release_secondary_types TEXT,
  is_compilation INTEGER NOT NULL DEFAULT 0,
  duration_delta REAL,
  score_breakdown TEXT
);

CREATE TABLE IF NOT EXISTS candidate_sources (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  candidate_id INTEGER NOT NULL REFERENCES candidates(id) ON DELETE CASCADE,
  provider TEXT NOT NULL,
  confidence REAL,
  reason_json TEXT,
  raw_json TEXT,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS provider_cache (
  provider TEXT NOT NULL,
  cache_key TEXT NOT NULL,
  response_json TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  PRIMARY KEY(provider, cache_key)
);

CREATE TABLE IF NOT EXISTS fingerprint_cache (
  path TEXT PRIMARY KEY,
  file_size INTEGER NOT NULL,
  file_mtime INTEGER NOT NULL,
  fingerprint TEXT NOT NULL,
  duration REAL NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS replaygain_cache (
  path TEXT PRIMARY KEY,
  file_size INTEGER NOT NULL,
  file_mtime_ns INTEGER NOT NULL,
  track_gain_db REAL NOT NULL,
  track_peak REAL NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS artwork_overrides (
  path TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  artist TEXT NOT NULL,
  cover_url TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS integrity_cache (
  path TEXT PRIMARY KEY,
  file_size INTEGER NOT NULL,
  file_mtime_ns INTEGER NOT NULL,
  is_healthy INTEGER NOT NULL,
  diagnostic TEXT,
  checked_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS candidates_track_id ON candidates(track_id);
CREATE INDEX IF NOT EXISTS tracks_stage ON tracks(stage);
CREATE INDEX IF NOT EXISTS candidate_sources_candidate_id ON candidate_sources(candidate_id);
