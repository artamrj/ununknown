import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "@/api/client";
import type { Candidate, Setup, Track, TrackPage, Workflow } from "@/api/types";

const emptySetup: Setup = { input_dir: "", output_dir: "", sources: {} };
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
          acoustid_key: keys.acoustid || undefined,
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
    if (!confirm("Write corrected copies named “Artist - Title” to the output folder?")) return;
    try {
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
        <div className="source-status">
          <span>Active catalogs: Apple Music, MusicBrainz, Wikidata</span>
          {(!setup.sources.fpcalc || !setup.sources.acoustid) && <small>For hard-to-name tracks, install Chromaprint (`fpcalc`) and add an AcoustID key.</small>}
        </div>
        <details>
          <summary>Optional source keys</summary>
          <p>Apple Music, MusicBrainz, Cover Art Archive, and Wikidata work without keys.</p>
          <div className="key-grid">
            <Secret label="AcoustID (fingerprints)" active={setup.sources.acoustid} value={keys.acoustid} onChange={(value) => setKeys({ ...keys, acoustid: value })} />
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
            <button className="primary" disabled={!ready.length} onClick={write}>Write {ready.length} corrected files</button>
          </div>
          {review.map((track) => <ReviewTrack key={track.id} track={track} onChoose={choose} onSaved={loadTracks} />)}
          <details className="ready-list">
            <summary>{ready.length} automatically identified</summary>
            {ready.map((track) => <TrackSummary key={track.id} track={track} candidate={track.candidates.find((candidate) => candidate.id === track.selected_candidate_id)} />)}
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
  return <article className="review card">
    <header><div><h3>{track.filename}</h3><p>{track.stage_message || track.error || "No reliable match was found."}</p></div><span>{Math.round(track.duration || 0)}s</span></header>
    {track.candidates.length > 0 && <div className="candidates">{track.candidates.slice(0, 5).map((candidate) => <button key={candidate.id} onClick={() => onChoose(track.id, candidate.id)}><b>{candidate.title}</b><span>{candidate.artist}</span><small>{[candidate.album, candidate.year, `${Math.round(candidate.score)}% match`].filter(Boolean).join(" · ")}</small><GenreLabel candidate={candidate} /></button>)}</div>}
    <button className="link" onClick={() => setManual(!manual)}>{manual ? "Close manual editor" : "Enter metadata manually"}</button>
    {manual && <ManualEditor track={track} onSaved={onSaved} />}
  </article>;
}

function ManualEditor({ track, onSaved }: { track: Track; onSaved: () => Promise<void> }) {
  const [form, setForm] = useState<Record<string, string | number>>({ title: track.current_title || "", artist: track.current_artist || "", album: track.current_album || "", album_artist: track.current_album_artist || "", track_number: track.current_track_number || "", year: "", genre: "" });
  const save = async () => {
    await api(`/tracks/${track.id}/manual`, { method: "PUT", body: JSON.stringify({ ...form, track_number: form.track_number ? Number(form.track_number) : null }) });
    await onSaved();
  };
  return <div className="manual">{Object.entries(form).map(([name, value]) => <label key={name}>{name.replace("_", " ")}<input value={value} onChange={(event) => setForm({ ...form, [name]: event.target.value })} /></label>)}<button className="primary" onClick={save}>Use this metadata</button></div>;
}

function TrackSummary({ track, candidate }: { track: Track; candidate?: Candidate }) {
  const extension = track.filename.includes(".") ? `.${track.filename.split(".").pop()?.toLowerCase()}` : "";
  const outputName = candidate ? `${safeName(candidate.artist || "Unknown Artist")} - ${safeName(candidate.title || "Unknown Title")}${extension}` : "Corrected filename";
  return <div className="track"><span>{track.filename}</span><b>→ {outputName}</b><small>{[candidate?.album, genreText(candidate)].filter(Boolean).join(" · ")}</small></div>;
}

function safeName(value: string) {
  return value.replace(/[\\/:*?"<>|]/g, " ").replace(/\s+/g, " ").replace(/^[ .]+|[ .]+$/g, "");
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
