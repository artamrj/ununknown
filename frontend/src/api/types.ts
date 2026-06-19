export type Candidate = {
  id: number;
  title?: string;
  artist?: string;
  album?: string;
  album_artist?: string;
  track_number?: number;
  track_total?: number;
  disc_number?: number;
  disc_total?: number;
  year?: string;
  genre?: string;
  composer?: string;
  label?: string;
  isrc?: string;
  cover_url?: string;
  score: number;
};

export type Track = {
  id: number;
  path?: string;
  filename: string;
  format?: string;
  duration?: number;
  current_title?: string;
  current_artist?: string;
  current_album?: string;
  current_album_artist?: string;
  current_track_number?: number;
  selected_candidate_id?: number;
  status: string;
  stage: string;
  stage_message?: string;
  error?: string;
  candidates: Candidate[];
};

export type TrackPage = { items: Track[]; total: number; counts: Record<string, number> };

export type MetadataSummary = {
  title?: string;
  artist?: string;
  album?: string;
  album_artist?: string;
  track_number?: number;
  disc_number?: number;
  year?: string;
  genre?: string;
  label?: string;
  isrc?: string;
  duration?: number;
  format?: string;
};

export type PreviewItem = {
  track_id: number;
  candidate_id: number;
  filename: string;
  current_path: string;
  destination_path: string;
  action: string;
  warnings?: string[];
  duplicate_group_id?: string;
  duplicate_action?: string;
  duplicate_reason?: string;
  kept_track_id?: number;
  old?: MetadataSummary;
  new?: MetadataSummary;
  cover_url?: string;
  current_cover_url?: string;
  proposed_cover_url?: string;
  confidence?: number;
  artwork_action?: string;
};

export type Preview = {
  preview_token: string;
  items: PreviewItem[];
  summary?: { write_count: number; duplicate_skipped: number };
};

export type ActivityLogLine = {
  timestamp: string;
  level: string;
  stage: string;
  file?: string;
  message: string;
  detail?: string;
  error?: string;
  attempt?: number;
  duration_ms?: number;
  context?: Record<string, unknown>;
};

export type Workflow = {
  phase: "idle" | "scan" | "fetch" | "preview" | "apply" | "finish" | "failed";
  message: string;
  current_file?: string;
  current: number;
  total: number;
  processed: number;
  matched: number;
  unmatched: number;
  failed: number;
  activity_log: ActivityLogLine[];
};
