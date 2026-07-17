import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "@/api/client";
import type { AutoApproveResult, Candidate, Setup, Track, TrackPage, Workflow } from "@/api/types";

const emptySetup: Setup = { input_dir: "", output_dir: "", delete_source_after_write: false, sources: {} };
const busyPhases = new Set(["scan", "fetch", "apply"]);

export function App() {
  const [setup, setSetup] = useState<Setup>(emptySetup);
  const [keys, setKeys] = useState<Record<string, string>>({});
  const [workflow, setWorkflow] = useState<Workflow>();
  const [tracks, setTracks] = useState<Track[]>([]);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [saving, setSaving] = useState(false);
  const [autoApproving, setAutoApproving] = useState(false);

  const loadTracks = useCallback(async () => {
    const page = await api<TrackPage>("/tracks?page_size=10000");
    setTracks(page.items);
  }, []);

  const refresh = useCallback(async () => {
    const status = await api<Workflow>("/status");
    setWorkflow(status);
    if (!busyPhases.has(status.phase)) await loadTracks();
  }, [loadTracks]);

  useEffect(() => {
    Promise.all([api<Setup>("/setup"), api<Workflow>("/status")])
      .then(([nextSetup, status]) => {
        setSetup(nextSetup);
        setWorkflow(status);
        return loadTracks();
      })
      .catch((reason) => setError(reason.message));
  }, [loadTracks]);

  useEffect(() => {
    if (!workflow || !busyPhases.has(workflow.phase)) return;
    const timer = window.setInterval(() => refresh().catch((reason) => setError(reason.message)), 900);
    return () => window.clearInterval(timer);
  }, [workflow, refresh]);

  const saveSetup = async () => {
    setSaving(true);
    setError("");
    try {
      await api("/setup", {
        method: "PUT",
        body: JSON.stringify({
          input_dir: setup.input_dir,
          output_dir: setup.output_dir,
          delete_source_after_write: setup.delete_source_after_write,
          acoustid_key: keys.acoustid || undefined,
          audd_token: keys.audd || undefined,
          spotify_client_id: keys.spotify_client_id || undefined,
          spotify_client_secret: keys.spotify_client_secret || undefined,
          soundcloud_client_id: keys.soundcloud_client_id || undefined,
          soundcloud_client_secret: keys.soundcloud_client_secret || undefined,
          youtube_api_key: keys.youtube || undefined,
          discogs_token: keys.discogs || undefined,
          lastfm_key: keys.lastfm || undefined,
          theaudiodb_key: keys.theaudiodb || undefined,
        }),
      });
      setKeys({});
      setSetup(await api<Setup>("/setup"));
    } finally {
      setSaving(false);
    }
  };

  const identify = async () => {
    try {
      await saveSetup();
      setTracks([]);
      await api("/identify", { method: "POST", body: "{}" });
      await refresh();
    } catch (reason) {
      setError((reason as Error).message);
    }
  };

  const choose = async (trackId: number, candidateId: number) => {
    await api(`/tracks/${trackId}/choose`, {
      method: "POST",
      body: JSON.stringify({ candidate_id: candidateId }),
    });
    await loadTracks();
  };

  const write = async () => {
    const message = setup.delete_source_after_write
      ? "Write corrected files, then permanently remove each original input file after its output succeeds?"
      : "Write corrected copies with metadata, cover art, ReplayGain, and “Artist - Title” filenames?";
    if (!confirm(message)) return;
    try {
      await saveSetup();
      await api("/write", { method: "POST", body: "{}" });
      await refresh();
    } catch (reason) {
      setError((reason as Error).message);
    }
  };

  const ready = useMemo(
    () => tracks.filter((track) => track.selected_candidate_id && track.stage === "ready"),
    [tracks],
  );
  const review = useMemo(
    () => tracks.filter((track) => track.stage !== "ready" && track.stage !== "skipped"),
    [tracks],
  );
  const autoApprovable = useMemo(
    () => review.filter((track) => track.stage === "review" && track.status !== "corrupt" && !track.is_missing && track.candidates.length > 0).length,
    [review],
  );
  const busy = workflow ? busyPhases.has(workflow.phase) : false;

  const autoApprove = async () => {
    if (!confirm(`Smart-select and complete metadata for up to ${autoApprovable} review tracks? Song identity, version, duration, album, source agreement, and verified cover art will be checked. Ambiguous or incomplete tracks will stay in review.`)) return;
    setAutoApproving(true);
    setError("");
    setNotice("");
    try {
      const result = await api<AutoApproveResult>("/tracks/auto-approve", { method: "POST", body: "{}" });
      await loadTracks();
      const details = [
        `${result.approved} review tracks smart-approved.`,
        result.low_confidence ? `${result.low_confidence} ambiguous or incomplete matches were left for review.` : "",
        result.unavailable ? `${result.unavailable} tracks have no usable candidate or cannot be written.` : "",
      ].filter(Boolean).join(" ");
      setNotice(details);
    } catch (reason) {
      setError((reason as Error).message);
    } finally {
      setAutoApproving(false);
    }
  };

  return (
    <main>
      <header className="hero">
        <span className="mark">♪</span>
        <div>
          <h1>Fix music metadata</h1>
          <p>Identify everything automatically. Only review what remains uncertain.</p>
        </div>
      </header>

      {error && <div className="error">{error}</div>}
      {notice && <div className="notice">{notice}</div>}

      <section className="setup card">
        <label>
          Music folder
          <input value={setup.input_dir} onChange={(event) => setSetup({ ...setup, input_dir: event.target.value })} placeholder="/Users/me/Music/to-fix" />
        </label>
        <label>
          Corrected copies
          <input value={setup.output_dir} onChange={(event) => setSetup({ ...setup, output_dir: event.target.value })} placeholder="/Users/me/Music/fixed" />
        </label>
        <label className={`delete-source-option ${setup.delete_source_after_write ? "enabled" : ""}`}>
          <input type="checkbox" checked={setup.delete_source_after_write} onChange={(event) => setSetup({ ...setup, delete_source_after_write: event.target.checked })} />
          <span>Remove input after successful output<small>Off by default. Originals are deleted only after their corrected file is safely written.</small></span>
        </label>
        <div className="source-status">
          <span>Active catalogs: Apple Music, Deezer, MusicBrainz, Radio Javan, Wikidata{setup.sources.genius ? ", Genius" : ""}{setup.sources.spotify ? ", Spotify" : ""}{setup.sources.soundcloud_search ? ", SoundCloud" : ""}</span>
          {(!setup.sources.fpcalc || !setup.sources.acoustid) && <small>For hard-to-name tracks, install Chromaprint (`fpcalc`) and <a href="https://acoustid.org/new-application" target="_blank" rel="noreferrer">add a free AcoustID application key</a>.</small>}
          {setup.sources.audd && <small className="recognition-active">AudD fallback recognition is active for fingerprints AcoustID cannot identify.</small>}
          {setup.sources.youtube && <small className="recognition-active">Exact YouTube video-ID recovery is active for downloaded filenames.</small>}
          {setup.sources.soundcloud && <small className="recognition-active">SoundCloud track-link metadata and cover lookup is active without a key.</small>}
          {setup.sources.radiojavan && <small className="recognition-active">Radio Javan metadata search, song-link lookup, and original cover art are active without a key.</small>}
          {setup.sources.genius && <small className="recognition-active">Genius metadata search, song-link lookup, and album artwork are active without a key.</small>}
          {setup.sources.ffmpeg
            ? <small className="replaygain-active">ReplayGain is active. Track loudness and peak are added when corrected files are written.</small>
            : <small>Install FFmpeg to add ReplayGain loudness metadata. Other corrections still work.</small>}
          {setup.sources.integrity_check
            ? <small className="integrity-active">Audio integrity checking is active. Damaged files are blocked before writing.</small>
            : <small>Install FFmpeg to detect corrupt audio during scanning.</small>}
        </div>
        <details>
          <summary>Optional source keys</summary>
          <p>Apple Music, Deezer, MusicBrainz, Radio Javan, Genius, Cover Art Archive, and Wikidata work without keys. Recognition services are called only when useful.</p>
          <p className="provider-links">Create credentials: <a href="https://acoustid.org/new-application" target="_blank" rel="noreferrer">AcoustID</a> · <a href="https://dashboard.audd.io/" target="_blank" rel="noreferrer">AudD</a> · <a href="https://developer.spotify.com/dashboard" target="_blank" rel="noreferrer">Spotify</a> · <a href="https://developers.soundcloud.com/docs/api/register-app" target="_blank" rel="noreferrer">SoundCloud</a> · <a href="https://console.cloud.google.com/apis/library/youtube.googleapis.com" target="_blank" rel="noreferrer">YouTube</a></p>
          <div className="key-grid">
            <Secret label="AcoustID (fingerprints)" active={setup.sources.acoustid} value={keys.acoustid} onChange={(value) => setKeys({ ...keys, acoustid: value })} />
            <Secret label="AudD token (fallback recognition)" active={setup.sources.audd} value={keys.audd} onChange={(value) => setKeys({ ...keys, audd: value })} />
            <Secret label="Spotify client ID" active={setup.sources.spotify} value={keys.spotify_client_id} onChange={(value) => setKeys({ ...keys, spotify_client_id: value })} />
            <Secret label="Spotify client secret" active={setup.sources.spotify} value={keys.spotify_client_secret} onChange={(value) => setKeys({ ...keys, spotify_client_secret: value })} />
            <Secret label="SoundCloud client ID" active={setup.sources.soundcloud_search} value={keys.soundcloud_client_id} onChange={(value) => setKeys({ ...keys, soundcloud_client_id: value })} />
            <Secret label="SoundCloud client secret" active={setup.sources.soundcloud_search} value={keys.soundcloud_client_secret} onChange={(value) => setKeys({ ...keys, soundcloud_client_secret: value })} />
            <Secret label="YouTube Data API key" active={setup.sources.youtube} value={keys.youtube} onChange={(value) => setKeys({ ...keys, youtube: value })} />
            <Secret label="Discogs" active={setup.sources.discogs} value={keys.discogs} onChange={(value) => setKeys({ ...keys, discogs: value })} />
            <Secret label="Last.fm" active={setup.sources.lastfm} value={keys.lastfm} onChange={(value) => setKeys({ ...keys, lastfm: value })} />
            <Secret label="TheAudioDB" active={setup.sources.theaudiodb} value={keys.theaudiodb} onChange={(value) => setKeys({ ...keys, theaudiodb: value })} />
          </div>
        </details>
        <button className="primary" disabled={busy || saving} onClick={identify}>{busy ? "Identifying…" : "Scan and identify"}</button>
      </section>

      {workflow && (busy || workflow.total > 0) && (
        <section className="progress card">
          <div><strong>{workflow.message}</strong><span>{workflow.current_file}</span></div>
          <b>{workflow.processed} / {workflow.total}</b>
          <progress max={workflow.total || 1} value={workflow.processed} />
          <small>{workflow.matched} found · {workflow.unmatched} need review · {workflow.failed} failed</small>
          {busy && <button className="link" onClick={() => api("/stop", { method: "POST", body: "{}" })}>Stop</button>}
        </section>
      )}

      {!busy && tracks.length > 0 && (
        <section className="results">
          <div className="section-title">
            <div><h2>{review.length ? `${review.length} need your help` : "Everything is identified"}</h2><p>{ready.length} tracks are ready to write.</p></div>
            <div className="write-action">
              {autoApprovable > 0 && <button className="secondary" disabled={autoApproving} onClick={autoApprove}>{autoApproving ? "Checking and completing metadata…" : `Smart auto-select ${autoApprovable} reviews`}</button>}
              <button className="primary" disabled={!ready.length} onClick={write}>Write {ready.length} corrected files</button>
              {autoApprovable > 0
                ? <small>Checks recording, version, duration, sources, album, cover, and metadata completeness—not just score.</small>
                : ready.length > 0 && <small>{setup.delete_source_after_write ? "Successful inputs will be removed" : "Includes ReplayGain track gain + peak"}</small>}
            </div>
          </div>
          {review.map((track) => <ReviewTrack key={track.id} track={track} onChoose={choose} onSaved={loadTracks} />)}
          <details className="ready-list">
            <summary>{ready.length} automatically identified</summary>
            {ready.map((track) => <TrackSummary key={track.id} track={track} candidate={track.candidates.find((candidate) => candidate.id === track.selected_candidate_id)} onSaved={loadTracks} />)}
          </details>
        </section>
      )}
    </main>
  );
}

function Secret({ label, active, value, onChange }: { label: string; active?: boolean; value?: string; onChange: (value: string) => void }) {
  return <label>{label} {active && <i>configured</i>}<input type="password" value={value || ""} onChange={(event) => onChange(event.target.value)} placeholder={active ? "Leave blank to keep current key" : "Optional"} /></label>;
}

function ReviewTrack({ track, onChoose, onSaved }: { track: Track; onChoose: (trackId: number, candidateId: number) => Promise<void>; onSaved: () => Promise<void> }) {
  const [manual, setManual] = useState(false);
  const corrupt = track.status === "corrupt";
  return <article className="review card">
    <header><div><h3>{track.filename}</h3><p className={corrupt ? "corrupt-message" : undefined}>{track.stage_message || track.error || "No reliable match was found."}</p>{corrupt && track.error && <small className="corrupt-detail">{track.error}</small>}</div><span>{Math.round(track.duration || 0)}s</span></header>
    <audio className="review-player" controls preload="none" src={`/api/tracks/${track.id}/audio`} aria-label={`Play ${track.filename}`} />
    {track.candidates.length > 0 && <div className="candidates">{track.candidates.slice(0, 5).map((candidate) => <div className="candidate-pair" key={candidate.id}><button onClick={() => onChoose(track.id, candidate.id)}><span className="provider-badge">From {candidateSources(candidate)}</span><Artwork candidate={candidate} /><b>{candidate.title}</b><span>{candidate.artist}</span><small>{[candidate.album, candidate.year, candidate.track_number ? `Track ${candidate.track_number}` : "", `${Math.round(candidate.score)}% match`].filter(Boolean).join(" · ")}</small><GenreLabel candidate={candidate} /><MetadataHealth candidate={candidate} compact /></button><GoogleCheck candidate={candidate} /></div>)}</div>}
    {!corrupt && <button className="link" onClick={() => setManual(!manual)}>{manual ? "Close manual editor" : "Enter metadata manually"}</button>}
    {!corrupt && manual && <ManualEditor track={track} onSaved={onSaved} />}
  </article>;
}

function ManualEditor({ track, candidate, onSaved }: { track: Track; candidate?: Candidate; onSaved: () => Promise<void> }) {
  const [form, setForm] = useState<Record<string, string | number>>({
    title: candidate?.title || track.current_title || "",
    artist: candidate?.artist || track.current_artist || "",
    album: candidate?.album || track.current_album || "",
    album_artist: candidate?.album_artist || track.current_album_artist || "",
    track_number: candidate?.track_number || track.current_track_number || "",
    year: candidate?.year || "",
    genre: candidate?.genre || "",
    cover_url: candidate?.cover_url || "",
  });
  const [sourceUrl, setSourceUrl] = useState("");
  const [sourceFeedback, setSourceFeedback] = useState<{ error?: boolean; text: string }>();
  const [resolvingSource, setResolvingSource] = useState(false);
  const resolveSource = async () => {
    setResolvingSource(true);
    setSourceFeedback(undefined);
    try {
      const found = await api<Candidate>("/source/resolve", { method: "POST", body: JSON.stringify({ url: sourceUrl }) });
      setForm({
        ...form,
        title: found.title || form.title,
        artist: found.artist || form.artist,
        album: found.album || form.album,
        album_artist: found.album_artist || form.album_artist,
        track_number: found.track_number || form.track_number,
        year: found.year || form.year,
        genre: found.genre || form.genre,
        cover_url: found.cover_url || form.cover_url,
      });
      setSourceFeedback({ text: `Loaded ${found.artist || "artist"} — ${found.title || "track"}, metadata, and cover.` });
    } catch (reason) {
      setSourceFeedback({ error: true, text: (reason as Error).message });
    } finally {
      setResolvingSource(false);
    }
  };
  const save = async () => {
    await api(`/tracks/${track.id}/manual`, { method: "PUT", body: JSON.stringify({ ...form, track_number: form.track_number ? Number(form.track_number) : null }) });
    await onSaved();
  };
  return <div className="manual"><label className="source-url">Spotify, SoundCloud, Radio Javan, Genius, or YouTube source URL<input value={sourceUrl} onChange={(event) => setSourceUrl(event.target.value)} placeholder="https://genius.com/Artist-song-lyrics" /></label><button className="secondary" disabled={!sourceUrl.trim() || resolvingSource} onClick={resolveSource}>{resolvingSource ? "Loading source…" : "Use source metadata and cover"}</button>{sourceFeedback && <small className={`source-feedback${sourceFeedback.error ? " error" : ""}`}>{sourceFeedback.text}</small>}{Object.entries(form).map(([name, value]) => <label key={name}>{name.replace("_", " ")}<input value={value} onChange={(event) => setForm({ ...form, [name]: event.target.value })} /></label>)}<button className="primary" onClick={save}>Use this metadata</button></div>;
}

function TrackSummary({ track, candidate, onSaved }: { track: Track; candidate?: Candidate; onSaved: () => Promise<void> }) {
  const [editing, setEditing] = useState(false);
  const [undoing, setUndoing] = useState(false);
  const extension = track.filename.includes(".") ? `.${track.filename.split(".").pop()?.toLowerCase()}` : "";
  const outputName = candidate ? `${safeName(candidate.artist || "Unknown Artist")} - ${safeName(candidate.title || "Unknown Title")}${extension}` : "Corrected filename";
  const setCover = async () => {
    const coverUrl = prompt("Paste a direct HTTPS image URL or a Spotify, SoundCloud, Radio Javan, or Genius track URL", candidate?.cover_url || "");
    if (!coverUrl) return;
    await api(`/tracks/${track.id}/artwork`, { method: "PUT", body: JSON.stringify({ cover_url: coverUrl }) });
    await onSaved();
  };
  const undoIdentification = async () => {
    setUndoing(true);
    try {
      await api(`/tracks/${track.id}/review`, { method: "POST", body: "{}" });
      await onSaved();
    } finally {
      setUndoing(false);
    }
  };
  return <div className="track">
    <div className="artwork-control"><Artwork candidate={candidate} trackId={track.id} /><button className="link" onClick={setCover}>Set cover</button></div>
    <div className="track-identity">
      <small>{track.filename}</small>
      <strong>{candidate?.title || "Missing title"}</strong>
      <span>{candidate?.artist || "Missing artist"}</span>
      <small>Output: {outputName}</small>
    </div>
    <div className="metadata-facts">
      <span><small>Album</small><b>{candidate?.album || "Missing"}</b></span>
      <span><small>Year</small><b>{candidate?.year || candidate?.release_date?.slice(0, 4) || "Missing"}</b></span>
      <span><small>Genre</small><b>{genreText(candidate) || "Missing"}</b></span>
      <span><small>Track</small><b>{candidate?.track_number ? `${candidate.track_number}${candidate.track_total ? ` / ${candidate.track_total}` : ""}` : "Missing"}</b></span>
    </div>
    <div className="track-metadata">
      {candidate && <span className="provider-badge">From {candidateSources(candidate)}</span>}
      <MetadataHealth candidate={candidate} />
      {track.stage_message && <small className="selection-reason">{track.stage_message}</small>}
      <div className="track-actions">
        <button className="link" onClick={() => setEditing(!editing)}>{editing ? "Close editor" : "Edit metadata"}</button>
        <button className="link undo" disabled={undoing} onClick={undoIdentification}>{undoing ? "Returning…" : "Undo identification"}</button>
      </div>
    </div>
    {candidate && <GoogleCheck candidate={candidate} compact />}
    {editing && <ManualEditor track={track} candidate={candidate} onSaved={async () => { await onSaved(); setEditing(false); }} />}
  </div>;
}

function GoogleCheck({ candidate, compact = false }: { candidate: Candidate; compact?: boolean }) {
  return <a className={`google-check${compact ? " compact" : ""}`} href={googleMetadataUrl(candidate)} target="_blank" rel="noreferrer" aria-label={`Verify the release of ${candidate.artist} ${candidate.title} on Google`}>
    <b>Check on Google</b>
    <small>Album or single?<br />Album name?<br />Original year?</small>
    <span aria-hidden="true">↗</span>
  </a>;
}

function Artwork({ candidate, trackId }: { candidate?: Candidate; trackId?: number }) {
  const urls = trackId
    ? [`/api/tracks/${trackId}/artwork/preview?v=${encodeURIComponent(candidate?.cover_url || candidate?.id || "embedded")}`]
    : candidate?.id
      ? [`/api/candidates/${candidate.id}/artwork/preview?v=${encodeURIComponent(candidate.cover_url || candidate.id)}`, ...artworkUrls(candidate)]
      : artworkUrls(candidate);
  const [index, setIndex] = useState(0);
  useEffect(() => setIndex(0), [candidate?.id, candidate?.cover_url]);
  return urls[index]
    ? <img className="artwork" src={urls[index]} alt={`Cover for ${candidate?.album || candidate?.title || "track"}`} loading="lazy" onError={() => setIndex((current) => current + 1)} />
    : <span className="artwork missing">No catalog cover</span>;
}

function artworkUrls(candidate?: Candidate) {
  const urls: string[] = candidate?.cover_url ? [candidate.cover_url] : [];
  try {
    const alternatives = JSON.parse(candidate?.score_breakdown || "{}")?.artwork_candidates || [];
    for (const artwork of alternatives) if (typeof artwork?.url === "string") urls.push(artwork.url);
  } catch {
    // A malformed explanation must not break the rest of the review screen.
  }
  return [...new Set(urls)];
}

function safeName(value: string) {
  return value.replace(/[\\/:*?"<>|]/g, " ").replace(/\s+/g, " ").replace(/^[ .]+|[ .]+$/g, "");
}

function googleMetadataUrl(candidate: Candidate) {
  const artist = String(candidate.artist || "").replaceAll('"', "");
  const title = String(candidate.title || "").replaceAll('"', "");
  const terms = `"${artist}" "${title}" song album or single album name original release year`;
  return `https://www.google.com/search?q=${encodeURIComponent(terms)}`;
}

function candidateSources(candidate: Candidate) {
  try {
    const sources = JSON.parse(candidate.score_breakdown || "{}")?.sources;
    if (Array.isArray(sources) && sources.length > 0) {
      return [...new Set(sources.filter((source): source is string => typeof source === "string"))].join(" + ");
    }
  } catch {
    // Fall back to the primary provider when evidence JSON is malformed.
  }
  const names: Record<string, string> = {
    acoustid: "AcoustID",
    audd: "AudD",
    deezer: "Deezer",
    discogs: "Discogs",
    itunes: "Apple Music",
    lastfm: "Last.fm",
    genius: "Genius",
    musicbrainz: "MusicBrainz",
    radiojavan: "Radio Javan",
    soundcloud: "SoundCloud",
    spotify: "Spotify",
    theaudiodb: "TheAudioDB",
    wikidata: "Wikidata",
    youtube: "YouTube",
    manual: "Manual entry",
  };
  return names[candidate.provider || ""] || candidate.provider || "Catalog";
}

function GenreLabel({ candidate }: { candidate: Candidate }) {
  const text = genreText(candidate);
  return text ? <em className="genre">{text}</em> : <em className="genre uncertain">Genre needs review</em>;
}

function MetadataHealth({ candidate, compact = false }: { candidate?: Candidate; compact?: boolean }) {
  if (!candidate) return <span className="metadata-health incomplete">Metadata missing</span>;
  const audit = metadataAudit(candidate);
  return <span className={`metadata-health ${audit.coreComplete ? "complete" : "incomplete"}${compact ? " compact" : ""}`} title={audit.missing.length ? `Missing: ${audit.missing.join(", ")}` : "Core metadata is complete"}>
    {audit.score}% metadata{audit.missing.length ? ` · missing ${audit.missing.slice(0, compact ? 1 : 3).join(", ")}` : " · complete"}
  </span>;
}

function metadataAudit(candidate: Candidate) {
  try {
    const stored = JSON.parse(candidate.score_breakdown || "{}")?.metadata_completion;
    if (stored && typeof stored.score === "number" && Array.isArray(stored.missing_fields)) {
      return { score: stored.score, coreComplete: stored.core_complete === true, missing: stored.missing_fields as string[] };
    }
  } catch {
    // Compute a display-only audit when older candidates have no worker report.
  }
  const fields: Array<[string, boolean, number]> = [
    ["title", Boolean(candidate.title?.trim()), 18],
    ["artist", Boolean(candidate.artist?.trim()), 18],
    ["album", Boolean(candidate.album?.trim()), 16],
    ["cover", Boolean(candidate.cover_url?.trim()), 16],
    ["year", Boolean(candidate.year?.trim() || candidate.release_date?.trim()), 10],
    ["genre", Boolean(candidate.genre?.trim()), 8],
    ["track number", Boolean(candidate.track_number), 6],
    ["album artist", Boolean(candidate.album_artist?.trim()), 4],
    ["ISRC", Boolean(candidate.isrc?.trim()), 4],
  ];
  return {
    score: fields.filter(([, present]) => present).reduce((total, [, , weight]) => total + weight, 0),
    coreComplete: fields.slice(0, 4).every(([, present]) => present),
    missing: fields.filter(([, present]) => !present).map(([name]) => name),
  };
}

function genreText(candidate?: Candidate) {
  if (!candidate?.genre) return "";
  try {
    const detail = JSON.parse(candidate.score_breakdown || "{}")?.genre;
    const confidence = typeof detail?.confidence === "number" ? `${Math.round(detail.confidence * 100)}% genre` : "";
    return [candidate.genre, detail?.language, confidence].filter(Boolean).join(" · ");
  } catch {
    return candidate.genre;
  }
}
