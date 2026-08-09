import type { Candidate, Track } from "@/api/types";
import type { IconName } from "./Icons";

export const busyPhases = new Set(["scan", "fetch", "apply"]);
export type QueueFilter = "all" | "review" | "problems" | "ready";
export type QueueOrder = "queue" | "title" | "artist" | "status";
export type Theme = "dark" | "light";

export const queueOrderLabels: Record<QueueOrder, string> = {
  queue: "Queue",
  title: "Title",
  artist: "Artist",
  status: "Status",
};
export const queueOrderSequence: QueueOrder[] = ["queue", "title", "artist", "status"];

export function candidateConfidence(score: number) {
  if (score >= 85) return { label: "Strong match", tone: "strong" };
  if (score >= 70) return { label: "Likely match", tone: "likely" };
  if (score >= 55) return { label: "Uncertain", tone: "uncertain" };
  return { label: "Weak match", tone: "weak" };
}

export function candidateSignals(track: Track, candidate: Candidate) {
  let evidence: Record<string, unknown> = {};
  try {
    evidence = JSON.parse(candidate.score_breakdown || "{}");
  } catch {
    /* Compare available text directly. */
  }
  return [
    comparisonSignal("Title", track.current_title, candidate.title, evidence.title),
    comparisonSignal("Artist", track.current_artist, candidate.artist, evidence.artist),
    comparisonSignal("Album", track.current_album, candidate.album, evidence.album_context),
    durationSignal(evidence.duration),
    releaseContextSignal(evidence.release_context),
  ];
}

export function releaseContextSignal(raw: unknown) {
  if (!raw || typeof raw !== "object")
    return { label: "Release", value: "Not checked", tone: "muted" };
  const context = raw as { conflict?: unknown; matched_source_album?: unknown; reason?: unknown };
  if (context.conflict === true) {
    return {
      label: "Release",
      value:
        context.reason === "candidate has no release album"
          ? "Release not found"
          : "Conflicts with file",
      tone: "warning",
    };
  }
  if (context.matched_source_album === true)
    return { label: "Release", value: "Matches file", tone: "good" };
  return { label: "Release", value: "Alternate edition", tone: "warning" };
}

export function candidateVerdict(score: number, signals: Array<{ tone: string }>) {
  const warnings = signals.filter((signal) => signal.tone === "warning").length;
  if (score >= 85 && warnings === 0)
    return {
      tone: "good",
      title: "Good choice",
      detail: "The identity and audio evidence agree with your file.",
    };
  if (score >= 70 && warnings <= 1)
    return {
      tone: "caution",
      title: "Likely match",
      detail: "Most evidence agrees; check the highlighted difference before choosing.",
    };
  return {
    tone: "risk",
    title: "Needs caution",
    detail: warnings
      ? `${warnings} important ${warnings === 1 ? "difference" : "differences"} detected. Choose only if you recognize this release.`
      : "The available evidence is too weak to recommend this automatically.",
  };
}

export function comparisonSignal(
  label: string,
  original?: string,
  proposed?: string,
  rawSimilarity?: unknown,
) {
  if (!original?.trim()) return { label, value: "No original", tone: "muted" };
  if (!proposed?.trim()) return { label, value: "Missing", tone: "warning" };
  const similarity =
    typeof rawSimilarity === "number"
      ? rawSimilarity
      : normalizeText(original) === normalizeText(proposed)
        ? 1
        : 0;
  if (similarity >= 0.92) return { label, value: "Exact", tone: "good" };
  if (similarity >= 0.7) return { label, value: "Close", tone: "good" };
  if (similarity >= 0.4) return { label, value: "Different", tone: "warning" };
  return { label, value: "Mismatch", tone: "warning" };
}

export function durationSignal(rawSimilarity?: unknown) {
  if (typeof rawSimilarity !== "number")
    return { label: "Audio length", value: "Not checked", tone: "muted" };
  if (rawSimilarity >= 0.9) return { label: "Audio length", value: "Same", tone: "good" };
  if (rawSimilarity >= 0.55) return { label: "Audio length", value: "Close", tone: "good" };
  return { label: "Audio length", value: "Different", tone: "warning" };
}

export function normalizeText(value: string) {
  return value
    .toLocaleLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, "")
    .trim();
}

export function artworkStatusLabel(candidate: Candidate) {
  switch (candidate.artwork_status) {
    case "verified":
      return "Verified cover";
    case "searching":
      return "Checking cover";
    case "retryable_error":
      return "Cover source unavailable";
    case "cover_required":
      return "Cover required";
    default:
      return candidate.cover_url ? "Catalog cover" : "No catalog cover";
  }
}
export function formatDuration(seconds?: number) {
  if (!seconds || !Number.isFinite(seconds)) return undefined;
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${Math.round(seconds % 60)
    .toString()
    .padStart(2, "0")}`;
}

export function isReview(track: Track) {
  return track.stage === "review" && !isCompleted(track) && !isProblem(track);
}
export function isReady(track: Track) {
  return track.stage === "ready" && Boolean(track.selected_candidate_id) && !isCompleted(track);
}
export function isCompleted(track: Track) {
  return track.status === "applied";
}
export function isProblem(track: Track) {
  if (track.stage === "skipped") return false;
  return (
    track.status === "corrupt" ||
    track.is_missing ||
    track.stage === "failed" ||
    track.status === "failed" ||
    track.status === "provider_error"
  );
}
export function hasRetryableArtwork(track: Track) {
  return selectedCandidate(track)?.artwork_status === "retryable_error";
}
export function selectedCandidate(track: Track) {
  return track.candidates.find((candidate) => candidate.id === track.selected_candidate_id);
}

export function statusFor(track: Track): { label: string; tone: string; icon: IconName } {
  if (isCompleted(track)) return { label: "Cleaned", tone: "success", icon: "check" };
  if (track.status === "corrupt") return { label: "Damaged", tone: "error", icon: "alert" };
  if (track.stage === "skipped") return { label: "Skipped", tone: "muted", icon: "skip" };
  if (track.is_missing) return { label: "File missing", tone: "error", icon: "alert" };
  if (track.stage === "failed" || track.status === "failed" || track.status === "provider_error")
    return { label: "Failed", tone: "error", icon: "alert" };
  if (track.stage === "review")
    return {
      label: selectedCandidate(track)
        ? "Needs review"
        : track.candidates.length > 1
          ? "Needs review"
          : track.candidates.length
            ? "Uncertain"
            : "Not identified",
      tone: "review",
      icon: "info",
    };
  if (isReady(track)) return { label: "Ready", tone: "success", icon: "check" };
  return { label: "Processing", tone: "processing", icon: "waveform" };
}

export function problemTitle(track: Track) {
  if (track.status === "corrupt") return "Damaged audio file";
  if (track.is_missing) return "Source file is missing";
  return "Could not process this track";
}

export function friendlyTrackError(track: Track) {
  if (!track.error) return "";
  if (track.status === "corrupt")
    return "The audio stream could not be decoded safely. This file will not be written.";
  if (track.is_missing) return "The source file is no longer available at its original location.";
  return "Ununknown could not finish this track. Open technical details if you need the diagnostic message.";
}

export function queuePriority(track: Track) {
  if (isReview(track)) return 0;
  if (isProblem(track)) return 1;
  if (isReady(track)) return 2;
  if (isCompleted(track)) return 3;
  return 4;
}

export function compareTracks(first: Track, second: Track, order: QueueOrder) {
  if (order === "status")
    return (
      queuePriority(first) - queuePriority(second) ||
      compareText(trackTitle(first), trackTitle(second))
    );
  if (order === "artist")
    return (
      compareText(trackArtist(first), trackArtist(second)) ||
      compareText(trackTitle(first), trackTitle(second))
    );
  return compareText(trackTitle(first), trackTitle(second));
}

export function trackTitle(track: Track) {
  const candidate = selectedCandidate(track) ?? track.candidates[0];
  return candidate?.title || track.current_title || fileStem(track.filename);
}
export function trackArtist(track: Track) {
  const candidate = selectedCandidate(track) ?? track.candidates[0];
  return candidate?.artist || track.current_artist || "Unknown artist";
}
export function compareText(first: string, second: string) {
  return first.localeCompare(second, undefined, { numeric: true, sensitivity: "base" });
}

export function preferredTrack(tracks: Track[]) {
  return [...tracks].sort(
    (first, second) =>
      queuePriority(first) - queuePriority(second) || first.filename.localeCompare(second.filename),
  )[0];
}

export function filterTitle(filter: QueueFilter) {
  return {
    all: "Music queue",
    review: "Needs review",
    problems: "Problems",
    ready: "Ready to write",
  }[filter];
}
export function folderName(path: string) {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] || "Choose folder";
}
export function fileStem(filename: string) {
  return filename.replace(/\.[^/.]+$/, "");
}
export function outputFilename(track: Track, candidate: Candidate) {
  const extension = track.filename.includes(".")
    ? `.${track.filename.split(".").pop()?.toLowerCase()}`
    : "";
  return `${safeName(candidate.artist || "Unknown Artist")} - ${safeName(candidate.title || "Unknown Title")}${extension}`;
}
export function safeName(value: string) {
  return value
    .replace(/[\\/:*?"<>|]/g, " ")
    .replace(/\s+/g, " ")
    .replace(/^[ .]+|[ .]+$/g, "");
}
export function friendlyError(value: string) {
  if (/fetch|network|load failed/i.test(value))
    return "The local service is not responding. Make sure ./dev.sh is running, then reconnect.";
  if (/permission|denied/i.test(value))
    return `${value} Check that Ununknown can read the music folder and write to the output folder.`;
  return value;
}

export function artworkUrls(candidate?: Candidate) {
  const urls: string[] = candidate?.cover_url ? [candidate.cover_url] : [];
  try {
    const alternatives = JSON.parse(candidate?.score_breakdown || "{}")?.artwork_candidates || [];
    for (const artwork of alternatives)
      if (typeof artwork?.url === "string") urls.push(artwork.url);
  } catch {
    /* Old evidence can be malformed. */
  }
  return [...new Set(urls)];
}

export function googleMetadataUrl(candidate: Candidate) {
  const artist = String(candidate.artist || "").replaceAll('"', "");
  const title = String(candidate.title || "").replaceAll('"', "");
  return `https://www.google.com/search?q=${encodeURIComponent(`"${artist}" "${title}" song album or single album name original release year`)}`;
}

export function candidateSources(candidate: Candidate) {
  try {
    const sources = JSON.parse(candidate.score_breakdown || "{}")?.sources;
    if (Array.isArray(sources) && sources.length)
      return [
        ...new Set<string>(
          sources
            .filter((source: unknown): source is string => typeof source === "string")
            .map(providerName),
        ),
      ].join(" + ");
  } catch {
    /* Use primary provider. */
  }
  return providerName(candidate.provider || "catalog");
}

export function providerName(source: string) {
  const names: Record<string, string> = {
    acoustid: "AcoustID",
    audiomack: "Audiomack",
    audd: "AudD",
    deezer: "Deezer",
    discogs: "Discogs",
    itunes: "Apple Music",
    lastfm: "Last.fm",
    genius: "Genius",
    musicbrainz: "MusicBrainz",
    navahang: "Navahang",
    radiojavan: "Radio Javan",
    shazam: "Shazam",
    songrec: "SongRec / Shazam",
    "SongRec / Shazam": "SongRec / Shazam",
    soundcloud: "SoundCloud",
    spotify: "Spotify",
    theaudiodb: "TheAudioDB",
    wikidata: "Wikidata",
    youtube: "YouTube",
    ffmpeg: "FFmpeg",
    manual: "Manual entry",
    catalog: "Catalog",
  };
  return names[source] || source;
}

export function metadataAudit(candidate: Candidate) {
  try {
    const stored = JSON.parse(candidate.score_breakdown || "{}")?.metadata_completion;
    if (stored && typeof stored.score === "number" && Array.isArray(stored.missing_fields))
      return {
        score: stored.score,
        coreComplete: stored.core_complete === true,
        missing: stored.missing_fields as string[],
      };
  } catch {
    /* Derive a display audit. */
  }
  const fields: Array<[string, boolean, number]> = [
    ["title", Boolean(candidate.title?.trim()), 18],
    ["artist", Boolean(candidate.artist?.trim()), 18],
    ["album", Boolean(candidate.album?.trim()), 16],
    ["cover", Boolean(candidate.cover_url?.trim()), 16],
    ["year", Boolean(candidate.year?.trim() || candidate.release_date?.trim()), 10],
    ["genre", Boolean(candidate.genre?.trim()), 8],
    ["track", Boolean(candidate.track_number), 6],
    ["album artist", Boolean(candidate.album_artist?.trim()), 4],
    ["ISRC", Boolean(candidate.isrc?.trim()), 4],
  ];
  return {
    score: fields
      .filter(([, present]) => present)
      .reduce((total, [, , weight]) => total + weight, 0),
    coreComplete: fields.slice(0, 4).every(([, present]) => present),
    missing: fields.filter(([, present]) => !present).map(([name]) => name),
  };
}
