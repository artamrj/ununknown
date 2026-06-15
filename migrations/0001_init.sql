PRAGMA foreign_keys = ON;

CREATE TABLE tracks (
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
  last_apply_run_id TEXT
);

CREATE TABLE candidates (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
  provider TEXT NOT NULL,
  title TEXT, artist TEXT, album TEXT, album_artist TEXT,
  track_number INTEGER, track_total INTEGER, disc_number INTEGER, disc_total INTEGER,
  year TEXT, genre TEXT, composer TEXT, label TEXT, isrc TEXT, cover_url TEXT,
  musicbrainz_recording_id TEXT, musicbrainz_release_id TEXT,
  musicbrainz_artist_id TEXT, musicbrainz_album_artist_id TEXT,
  score REAL NOT NULL,
  raw_json TEXT
);

CREATE TABLE jobs (
  id TEXT PRIMARY KEY, kind TEXT NOT NULL, status TEXT NOT NULL,
  progress_current INTEGER NOT NULL DEFAULT 0, progress_total INTEGER NOT NULL DEFAULT 0,
  error TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);

CREATE TABLE scan_runs (
  id TEXT PRIMARY KEY, started_at TEXT NOT NULL, finished_at TEXT, status TEXT NOT NULL,
  total_files INTEGER DEFAULT 0, new_files INTEGER DEFAULT 0, updated_files INTEGER DEFAULT 0,
  unchanged_files INTEGER DEFAULT 0, missing_files INTEGER DEFAULT 0,
  matched_files INTEGER DEFAULT 0, failed_files INTEGER DEFAULT 0
);

CREATE TABLE apply_runs (
  id TEXT PRIMARY KEY, preview_token TEXT NOT NULL, started_at TEXT NOT NULL,
  finished_at TEXT, status TEXT NOT NULL, total_files INTEGER DEFAULT 0,
  applied_files INTEGER DEFAULT 0, failed_files INTEGER DEFAULT 0
);

CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE provider_cache (
  provider TEXT NOT NULL, cache_key TEXT NOT NULL, response_json TEXT NOT NULL,
  expires_at TEXT NOT NULL, PRIMARY KEY(provider, cache_key)
);
CREATE INDEX candidates_track_id ON candidates(track_id);
CREATE INDEX tracks_status ON tracks(status);
