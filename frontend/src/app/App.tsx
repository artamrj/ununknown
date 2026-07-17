import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "@/api/client";
import type { Candidate, Setup, Track, TrackPage, Workflow } from "@/api/types";

const emptySetup: Setup = { input_dir: "", output_dir: "", delete_source_after_write: false, sources: {} };
const busyPhases = new Set(["scan", "fetch", "apply"]);

export function App() {
  const [setup, setSetup] = useState<Setup>(emptySetup);
  const [keys, setKeys] = useState<Record<string, string>>({});
  const [workflow, setWorkflow] = useState<Workflow>();
  const [tracks, setTracks] = useState<Track[]>([]);
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);

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
  const busy = workflow ? busyPhases.has(workflow.phase) : false;

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
          <span>Active catalogs: Apple Music, Deezer, MusicBrainz, Wikidata{setup.sources.spotify ? ", Spotify" : ""}{setup.sources.soundcloud_search ? ", SoundCloud" : ""}</span>
          {(!setup.sources.fpcalc || !setup.sources.acoustid) && <small>For hard-to-name tracks, install Chromaprint (`fpcalc`) and <a href="https://acoustid.org/new-application" target="_blank" rel="noreferrer">add a free AcoustID application key</a>.</small>}
          {setup.sources.audd && <small className="recognition-active">AudD fallback recognition is active for fingerprints AcoustID cannot identify.</small>}
          {setup.sources.youtube && <small className="recognition-active">Exact YouTube video-ID recovery is active for downloaded filenames.</small>}
          {setup.sources.soundcloud && <small className="recognition-active">SoundCloud track-link metadata and cover lookup is active without a key.</small>}
          {setup.sources.ffmpeg
            ? <small className="replaygain-active">ReplayGain is active. Track loudness and peak are added when corrected files are written.</small>
            : <small>Install FFmpeg to add ReplayGain loudness metadata. Other corrections still work.</small>}
          {setup.sources.integrity_check
            ? <small className="integrity-active">Audio integrity checking is active. Damaged files are blocked before writing.</small>
            : <small>Install FFmpeg to detect corrupt audio during scanning.</small>}
        </div>
        <details>
          <summary>Optional source keys</summary>
          <p>Apple Music, Deezer, MusicBrainz, Cover Art Archive, and Wikidata work without keys. Recognition services are called only when useful.</p>
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
              <button className="primary" disabled={!ready.length} onClick={write}>Write {ready.length} corrected files</button>
              {ready.length > 0 && <small>{setup.delete_source_after_write ? "Successful inputs will be removed" : "Includes ReplayGain track gain + peak"}</small>}
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
    {track.candidates.length > 0 && <div className="candidates">{track.candidates.slice(0, 5).map((candidate) => <div className="candidate-pair" key={candidate.id}><button onClick={() => onChoose(track.id, candidate.id)}><span className="provider-badge">From {candidateSources(candidate)}</span><Artwork candidate={candidate} /><b>{candidate.title}</b><span>{candidate.artist}</span><small>{[candidate.album, candidate.year, `${Math.round(candidate.score)}% match`].filter(Boolean).join(" · ")}</small><GenreLabel candidate={candidate} /></button><a className="google-check" href={googleMetadataUrl(candidate)} target="_blank" rel="noreferrer" aria-label={`Verify the release of ${candidate.artist} ${candidate.title} on Google`}><b>Check on Google</b><small>Album or single?<br />Album name?<br />Original year?</small><span aria-hidden="true">↗</span></a></div>)}</div>}
    {!corrupt && <button className="link" onClick={() => setManual(!manual)}>{manual ? "Close manual editor" : "Enter metadata manually"}</button>}
    {!corrupt && manual && <ManualEditor track={track} onSaved={onSaved} />}
  </article>;
}

function ManualEditor({ track, onSaved }: { track: Track; onSaved: () => Promise<void> }) {
  const [form, setForm] = useState<Record<string, string | number>>({ title: track.current_title || "", artist: track.current_artist || "", album: track.current_album || "", album_artist: track.current_album_artist || "", track_number: track.current_track_number || "", year: "", genre: "", cover_url: "" });
  const [sourceUrl, setSourceUrl] = useState("");
  const resolveSource = async () => {
    const found = await api<Candidate>("/source/resolve", { method: "POST", body: JSON.stringify({ url: sourceUrl }) });
    setForm({ ...form, title: found.title || form.title, artist: found.artist || form.artist, cover_url: found.cover_url || form.cover_url });
  };
  const save = async () => {
    await api(`/tracks/${track.id}/manual`, { method: "PUT", body: JSON.stringify({ ...form, track_number: form.track_number ? Number(form.track_number) : null }) });
    await onSaved();
  };
  return <div className="manual"><label className="source-url">Spotify, SoundCloud, or YouTube source URL<input value={sourceUrl} onChange={(event) => setSourceUrl(event.target.value)} placeholder="https://open.spotify.com/track/…" /></label><button className="secondary" disabled={!sourceUrl.trim()} onClick={resolveSource}>Use source metadata and cover</button>{Object.entries(form).map(([name, value]) => <label key={name}>{name.replace("_", " ")}<input value={value} onChange={(event) => setForm({ ...form, [name]: event.target.value })} /></label>)}<button className="primary" onClick={save}>Use this metadata</button></div>;
}

function TrackSummary({ track, candidate, onSaved }: { track: Track; candidate?: Candidate; onSaved: () => Promise<void> }) {
  const extension = track.filename.includes(".") ? `.${track.filename.split(".").pop()?.toLowerCase()}` : "";
  const outputName = candidate ? `${safeName(candidate.artist || "Unknown Artist")} - ${safeName(candidate.title || "Unknown Title")}${extension}` : "Corrected filename";
  const setCover = async () => {
    const coverUrl = prompt("Paste a direct HTTPS image URL, Spotify track URL, or SoundCloud track URL", candidate?.cover_url || "");
    if (!coverUrl) return;
    await api(`/tracks/${track.id}/artwork`, { method: "PUT", body: JSON.stringify({ cover_url: coverUrl }) });
    await onSaved();
  };
  return <div className="track"><div className="artwork-control"><Artwork candidate={candidate} trackId={track.id} /><button className="link" onClick={setCover}>Set cover</button></div><span>{track.filename}</span><b>→ {outputName}</b><small>{[candidate?.album, genreText(candidate)].filter(Boolean).join(" · ")}</small></div>;
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
    musicbrainz: "MusicBrainz",
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
