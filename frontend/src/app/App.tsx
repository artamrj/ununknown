import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "@/api/client";
import type { AutoApproveResult, Candidate, Setup, Track, TrackPage, Workflow } from "@/api/types";
import { Icon, type IconName } from "./Icons";

const emptySetup: Setup = { input_dir: "", output_dir: "", delete_source_after_write: false, sources: {} };
const busyPhases = new Set(["scan", "fetch", "apply"]);
type QueueFilter = "all" | "review" | "problems" | "completed";
type Theme = "dark" | "light";

export function App() {
  const [setup, setSetup] = useState<Setup>(emptySetup);
  const [keys, setKeys] = useState<Record<string, string>>({});
  const [workflow, setWorkflow] = useState<Workflow>();
  const [tracks, setTracks] = useState<Track[]>([]);
  const [selectedId, setSelectedId] = useState<number>();
  const [filter, setFilter] = useState<QueueFilter>("all");
  const [query, setQuery] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [loading, setLoading] = useState(true);
  const [connected, setConnected] = useState(true);
  const [saving, setSaving] = useState(false);
  const [autoApproving, setAutoApproving] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [mobileInspector, setMobileInspector] = useState(false);
  const [theme, setTheme] = useState<Theme>(() => {
    const saved = localStorage.getItem("ununknown-theme");
    if (saved === "light" || saved === "dark") return saved;
    return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  });
  const pollCount = useRef(0);

  const loadTracks = useCallback(async () => {
    const page = await api<TrackPage>("/tracks?page_size=10000");
    setTracks(page.items);
    setSelectedId((current) => {
      if (current && page.items.some((track) => track.id === current)) return current;
      return page.items[0]?.id;
    });
  }, []);

  const loadApp = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const [nextSetup, status, page] = await Promise.all([
        api<Setup>("/setup"),
        api<Workflow>("/status"),
        api<TrackPage>("/tracks?page_size=10000"),
      ]);
      setSetup(nextSetup);
      setWorkflow(status);
      setTracks(page.items);
      setSelectedId((current) => current ?? page.items[0]?.id);
      setConnected(true);
    } catch (reason) {
      setConnected(false);
      setError((reason as Error).message || "The local backend could not be reached.");
    } finally {
      setLoading(false);
    }
  }, []);

  const refresh = useCallback(async () => {
    try {
      const status = await api<Workflow>("/status");
      setWorkflow(status);
      setConnected(true);
      pollCount.current += 1;
      if (!busyPhases.has(status.phase) || pollCount.current % 3 === 0) await loadTracks();
    } catch (reason) {
      setConnected(false);
      setError((reason as Error).message);
    }
  }, [loadTracks]);

  useEffect(() => { void loadApp(); }, [loadApp]);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem("ununknown-theme", theme);
  }, [theme]);

  useEffect(() => {
    if (!workflow || !busyPhases.has(workflow.phase)) return;
    const timer = window.setInterval(() => void refresh(), 900);
    return () => window.clearInterval(timer);
  }, [workflow, refresh]);

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      setSettingsOpen(false);
      setMobileInspector(false);
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, []);

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
      setNotice("Studio settings saved.");
      setConnected(true);
    } finally {
      setSaving(false);
    }
  };

  const identify = async () => {
    setNotice("");
    try {
      await saveSetup();
      setTracks([]);
      setSelectedId(undefined);
      await api("/identify", { method: "POST", body: "{}" });
      await refresh();
    } catch (reason) {
      setError((reason as Error).message);
    }
  };

  const choose = async (trackId: number, candidateId: number) => {
    setError("");
    try {
      await api(`/tracks/${trackId}/choose`, { method: "POST", body: JSON.stringify({ candidate_id: candidateId }) });
      await loadTracks();
      setNotice("Match accepted. This track is ready to clean.");
    } catch (reason) {
      setError((reason as Error).message);
    }
  };

  const counts = useMemo(() => ({
    review: tracks.filter(isReview).length,
    ready: tracks.filter(isReady).length,
    problems: tracks.filter(isProblem).length,
    completed: tracks.filter(isCompleted).length,
  }), [tracks]);

  const autoApprovable = useMemo(
    () => tracks.filter((track) => isReview(track) && !isProblem(track) && track.candidates.length > 0).length,
    [tracks],
  );
  const busy = workflow ? busyPhases.has(workflow.phase) : false;

  const autoApprove = async () => {
    setAutoApproving(true);
    setError("");
    setNotice("");
    try {
      const result = await api<AutoApproveResult>("/tracks/auto-approve", { method: "POST", body: "{}" });
      await loadTracks();
      setNotice([
        `${result.approved} review ${result.approved === 1 ? "track" : "tracks"} approved.`,
        result.low_confidence ? `${result.low_confidence} left for review.` : "",
        result.unavailable ? `${result.unavailable} unavailable.` : "",
      ].filter(Boolean).join(" "));
    } catch (reason) {
      setError((reason as Error).message);
    } finally {
      setAutoApproving(false);
    }
  };

  const write = async () => {
    if (setup.delete_source_after_write && !confirm("Corrected files will be written first. Each original will then be permanently removed only after its output succeeds. Continue?")) return;
    setError("");
    try {
      await saveSetup();
      await api("/write", { method: "POST", body: "{}" });
      await refresh();
    } catch (reason) {
      setError((reason as Error).message);
    }
  };

  const visibleTracks = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    return tracks.filter((track) => {
      const inFilter = filter === "all" || (filter === "review" && isReview(track)) ||
        (filter === "problems" && isProblem(track)) || (filter === "completed" && isCompleted(track));
      if (!inFilter) return false;
      if (!normalized) return true;
      const candidate = selectedCandidate(track) ?? track.candidates[0];
      return [track.filename, track.current_title, track.current_artist, track.current_album, candidate?.title, candidate?.artist, candidate?.album]
        .some((value) => value?.toLocaleLowerCase().includes(normalized));
    });
  }, [filter, query, tracks]);

  const selected = tracks.find((track) => track.id === selectedId);

  const changeFilter = (next: QueueFilter) => {
    setFilter(next);
    const nextTrack = tracks.find((track) => next === "all" || (next === "review" && isReview(track)) || (next === "problems" && isProblem(track)) || (next === "completed" && isCompleted(track)));
    setSelectedId(nextTrack?.id);
  };

  const selectTrack = (track: Track) => {
    setSelectedId(track.id);
    setMobileInspector(true);
  };

  return (
    <div className="studio-app">
      <header className="topbar">
        <a className="brand" href="#workspace" aria-label="Ununknown studio home">
          <span className="brand-mark"><Icon name="waveform" size={20} /></span>
          <span>Ununknown</span>
          <small>metadata studio</small>
        </a>
        <nav className="workspace-nav" aria-label="Workspace views">
          <NavButton active={filter === "all"} onClick={() => changeFilter("all")} label="Studio" />
          <NavButton active={filter === "review"} onClick={() => changeFilter("review")} label="Review" count={counts.review} />
          <NavButton active={filter === "problems"} onClick={() => changeFilter("problems")} label="Problems" count={counts.problems} />
          <NavButton active={filter === "completed"} onClick={() => changeFilter("completed")} label="Completed" count={counts.completed} />
        </nav>
        <div className="topbar-actions">
          <span className={`connection-state ${connected ? "online" : "offline"}`}><i />{connected ? "Local" : "Offline"}</span>
          <button className="icon-button" onClick={() => setTheme(theme === "dark" ? "light" : "dark")} aria-label={`Use ${theme === "dark" ? "light" : "dark"} theme`} title="Change theme">
            <Icon name={theme === "dark" ? "sun" : "moon"} />
          </button>
          <button className="icon-button" onClick={() => setSettingsOpen(true)} aria-label="Open settings" title="Settings"><Icon name="settings" /></button>
        </div>
      </header>

      <section className="source-bar" aria-labelledby="source-title">
        <div className="source-heading">
          <span className="source-icon"><Icon name="folder" /></span>
          <div><p className="eyebrow" id="source-title">Music source</p><p>{tracks.length ? `${tracks.length} audio ${tracks.length === 1 ? "file" : "files"} in queue` : "Select a folder to begin"}</p></div>
        </div>
        <label className="path-field">
          <span>Music folder</span>
          <input value={setup.input_dir} onChange={(event) => setSetup({ ...setup, input_dir: event.target.value })} placeholder="/Users/me/Music/to-clean" autoComplete="off" />
        </label>
        <div className="output-summary" title={setup.output_dir}>
          <span>Corrected copies</span>
          <b>{setup.output_dir || "Choose an output folder in settings"}</b>
        </div>
        <div className={`safety-state ${setup.delete_source_after_write ? "destructive" : "safe"}`}>
          <Icon name={setup.delete_source_after_write ? "trash" : "shield"} size={16} />
          <span><b>{setup.delete_source_after_write ? "Remove after success" : "Originals preserved"}</b><small>{setup.delete_source_after_write ? "Output is verified first" : "Safe default"}</small></span>
        </div>
        <button className="primary-action" disabled={busy || saving || !setup.input_dir.trim() || !setup.output_dir.trim()} onClick={identify}>
          {busy && workflow?.phase !== "apply" ? <span className="spinner" /> : <Icon name={tracks.length ? "refresh" : "sparkles"} />}
          {busy && workflow?.phase !== "apply" ? "Identifying…" : tracks.length ? "Rescan folder" : "Start cleaning"}
        </button>
      </section>

      <nav className="mobile-nav" aria-label="Workspace views">
        <NavButton active={filter === "all"} onClick={() => changeFilter("all")} label="Studio" />
        <NavButton active={filter === "review"} onClick={() => changeFilter("review")} label="Review" count={counts.review} />
        <NavButton active={filter === "problems"} onClick={() => changeFilter("problems")} label="Problems" count={counts.problems} />
        <NavButton active={filter === "completed"} onClick={() => changeFilter("completed")} label="Completed" count={counts.completed} />
      </nav>

      <div className="announcement-region" aria-live="polite" aria-atomic="true">
        {error && <div className="toast error-toast" role="alert"><Icon name="alert" /><span><b>{connected ? "Couldn’t complete that action" : "Backend unavailable"}</b><small>{friendlyError(error)}</small></span><button onClick={() => setError("")} aria-label="Dismiss error"><Icon name="x" size={16} /></button></div>}
        {notice && <div className="toast notice-toast"><Icon name="check" /><span><b>Done</b><small>{notice}</small></span><button onClick={() => setNotice("")} aria-label="Dismiss notification"><Icon name="x" size={16} /></button></div>}
      </div>

      <main className="workspace" id="workspace">
        <section className="queue-panel" aria-labelledby="queue-title">
          <header className="panel-header">
            <div><p className="eyebrow">Library</p><h1 id="queue-title">{filterTitle(filter)}</h1></div>
            <span className="queue-count">{visibleTracks.length}</span>
          </header>

          <div className="queue-toolbar">
            <label className="search-field"><Icon name="search" size={16} /><span className="sr-only">Search queue</span><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search title, artist, album…" /></label>
            {autoApprovable > 0 && filter === "review" && <button className="compact-button accent" disabled={autoApproving} onClick={autoApprove}><Icon name="sparkles" size={15} />{autoApproving ? "Checking…" : `Auto-select ${autoApprovable}`}</button>}
          </div>

          <div className="queue-columns" aria-hidden="true"><span>Track</span><span>Match</span><span>Status</span></div>

          <div className="track-list" role="list" aria-label="Music queue">
            {loading ? <QueueSkeleton /> : !connected && tracks.length === 0 ? <EmptyState icon="alert" title="The studio is offline" description="Start the local Rust backend, then reconnect to load your workspace." action="Reconnect" onAction={loadApp} />
              : tracks.length === 0 ? <EmptyState icon="music" title={busy ? "Listening to your library" : "Your queue is ready"} description={busy ? "Audio files will appear here as they are discovered and identified." : "Enter a music folder above, choose where corrected copies should go, then start cleaning."} />
              : visibleTracks.length === 0 ? <EmptyState icon={filter === "review" ? "check" : "search"} title={filter === "review" ? "Nothing needs review" : "No tracks found"} description={filter === "review" ? "All current matches are resolved. Your library is ready for the next step." : "Try another search or switch workspace views."} />
              : visibleTracks.map((track, index) => <TrackRow key={track.id} track={track} active={track.id === selectedId} index={index + 1} onClick={() => selectTrack(track)} />)}
          </div>
        </section>

        <aside className={`inspector-panel ${mobileInspector ? "mobile-open" : ""}`} aria-label="Metadata inspector">
          <button className="mobile-close icon-button" onClick={() => setMobileInspector(false)} aria-label="Close inspector"><Icon name="x" /></button>
          {selected ? <TrackInspector track={selected} onChoose={choose} onSaved={loadTracks} /> : <InspectorEmpty busy={busy} />}
        </aside>
      </main>

      <ProcessingDock workflow={workflow} counts={counts} busy={busy} deleteSources={setup.delete_source_after_write} onStop={() => void api("/stop", { method: "POST", body: "{}" })} onWrite={() => void write()} />

      {settingsOpen && <SettingsDrawer setup={setup} setSetup={setSetup} keys={keys} setKeys={setKeys} saving={saving} onSave={async () => { try { await saveSetup(); setSettingsOpen(false); } catch (reason) { setError((reason as Error).message); } }} onClose={() => setSettingsOpen(false)} />}
    </div>
  );
}

function NavButton({ active, label, count, onClick }: { active: boolean; label: string; count?: number; onClick: () => void }) {
  return <button className={active ? "active" : ""} onClick={onClick}>{label}{Boolean(count) && <span>{count}</span>}</button>;
}

function TrackRow({ track, active, index, onClick }: { track: Track; active: boolean; index: number; onClick: () => void }) {
  const candidate = selectedCandidate(track) ?? track.candidates[0];
  const displayTitle = candidate?.title || track.current_title || fileStem(track.filename);
  const artist = candidate?.artist || track.current_artist || "Unknown artist";
  const score = candidate?.score;
  return (
    <button className={`track-row ${active ? "selected" : ""}`} onClick={onClick} role="listitem" aria-current={active ? "true" : undefined}>
      <span className="row-number">{String(index).padStart(2, "0")}</span>
      <Artwork candidate={candidate} trackId={track.selected_candidate_id ? track.id : undefined} size="small" />
      <span className="row-identity"><b>{displayTitle}</b><small>{artist}<i>·</i>{candidate?.album || track.current_album || track.filename}</small></span>
      <span className="confidence-cell">{typeof score === "number" ? <><b>{Math.round(score)}%</b><small>match</small></> : <small>—</small>}</span>
      <TrackStatus track={track} />
      <Icon name="chevron" size={16} className="row-chevron" />
    </button>
  );
}

function TrackInspector({ track, onChoose, onSaved }: { track: Track; onChoose: (trackId: number, candidateId: number) => Promise<void>; onSaved: () => Promise<void> }) {
  const [editing, setEditing] = useState(false);
  const [advanced, setAdvanced] = useState(false);
  const [undoing, setUndoing] = useState(false);
  const candidate = selectedCandidate(track);
  const reviewCandidate = track.candidates[0];
  const heroCandidate = candidate ?? reviewCandidate;
  const corrupt = track.status === "corrupt";
  const undoIdentification = async () => {
    setUndoing(true);
    try {
      await api(`/tracks/${track.id}/review`, { method: "POST", body: "{}" });
      await onSaved();
    } finally { setUndoing(false); }
  };

  return (
    <div className="inspector-content">
      <header className="inspector-hero">
        <Artwork candidate={heroCandidate} trackId={candidate ? track.id : undefined} size="large" />
        <div className="inspector-title">
          <TrackStatus track={track} />
          <h2>{heroCandidate?.title || track.current_title || fileStem(track.filename)}</h2>
          <p>{heroCandidate?.artist || track.current_artist || "Unknown artist"}</p>
          <small title={track.filename}>{track.filename}</small>
        </div>
      </header>

      {!track.is_missing && !corrupt && <audio className="audio-player" controls preload="none" src={`/api/tracks/${track.id}/audio`} aria-label={`Play ${track.filename}`} />}

      {(isReview(track) || isProblem(track)) && <section className={`decision-note ${isProblem(track) ? "problem" : "review"}`}>
        <Icon name={isProblem(track) ? "alert" : "info"} />
        <div><b>{isProblem(track) ? problemTitle(track) : "Your review is needed"}</b><p>{track.stage_message || track.error || "The available matches are too close to choose safely."}</p>{track.error && track.stage_message && <small>{track.error}</small>}</div>
      </section>}

      {isReview(track) && track.candidates.length > 0 && <section className="inspector-section candidate-section">
        <div className="section-heading"><div><p className="eyebrow">Candidate matches</p><h3>Choose the right recording</h3></div><span>{track.candidates.length} found</span></div>
        <div className="candidate-list">
          {track.candidates.slice(0, 5).map((item, index) => <CandidateRow key={item.id} candidate={item} rank={index + 1} onChoose={() => void onChoose(track.id, item.id)} />)}
        </div>
      </section>}

      {candidate && <>
        <section className="inspector-section">
          <div className="section-heading"><div><p className="eyebrow">Metadata comparison</p><h3>Original <Icon name="arrow" size={15} /> Proposed</h3></div><button className="text-button" onClick={() => setEditing(!editing)}><Icon name="edit" size={14} />{editing ? "Close editor" : "Edit"}</button></div>
          <div className="metadata-comparison">
            <CompareRow label="Title" before={track.current_title} after={candidate.title} />
            <CompareRow label="Artist" before={track.current_artist} after={candidate.artist} />
            <CompareRow label="Album" before={track.current_album} after={candidate.album} />
            <CompareRow label="Album artist" before={track.current_album_artist} after={candidate.album_artist} />
            <CompareRow label="Track" before={track.current_track_number?.toString()} after={candidate.track_number ? `${candidate.track_number}${candidate.track_total ? ` / ${candidate.track_total}` : ""}` : undefined} />
            <CompareRow label="Year" after={candidate.year || candidate.release_date?.slice(0, 4)} />
            <CompareRow label="Genre" after={candidate.genre} />
          </div>
          <div className="output-filename"><Icon name="music" size={15} /><span><small>Clean filename</small><b>{outputFilename(track, candidate)}</b></span></div>
        </section>
        <section className="quality-strip">
          <QualityItem icon="layers" label="Sources" value={candidateSources(candidate)} />
          <QualityItem icon="album" label="Artwork" value={candidate.cover_url ? "Catalog cover" : "Embedded / missing"} />
          <QualityItem icon="waveform" label="ReplayGain" value="Added on write" />
        </section>
      </>}

      {editing && <ManualEditor track={track} candidate={candidate} onSaved={async () => { await onSaved(); setEditing(false); }} />}

      {!corrupt && !editing && <button className="manual-entry" onClick={() => setEditing(true)}><Icon name="edit" /><span><b>{candidate ? "Fine-tune metadata" : "Enter metadata manually"}</b><small>Use a source link or fill in fields yourself</small></span><Icon name="chevron" /></button>}

      {candidate && <details className="advanced-details" open={advanced} onToggle={(event) => setAdvanced(event.currentTarget.open)}>
        <summary>Source evidence and advanced details <Icon name="chevron" size={15} /></summary>
        <div><MetadataHealth candidate={candidate} /><GoogleCheck candidate={candidate} /><dl><div><dt>Provider</dt><dd>{candidateSources(candidate)}</dd></div><div><dt>ISRC</dt><dd>{candidate.isrc || "Not available"}</dd></div><div><dt>Label</dt><dd>{candidate.label || "Not available"}</dd></div><div><dt>Release date</dt><dd>{candidate.release_date || candidate.year || "Not available"}</dd></div></dl></div>
      </details>}

      {candidate && <div className="inspector-actions"><span><Icon name="check" />{isCompleted(track) ? "Corrected file written" : "Ready to write"}</span><button className="text-button warning" disabled={undoing} onClick={() => void undoIdentification()}>{undoing ? "Returning…" : "Return to review"}</button></div>}
    </div>
  );
}

function CandidateRow({ candidate, rank, onChoose }: { candidate: Candidate; rank: number; onChoose: () => void }) {
  const audit = metadataAudit(candidate);
  return <article className="candidate-row">
    <span className="candidate-rank">{rank}</span>
    <Artwork candidate={candidate} size="medium" />
    <div className="candidate-identity"><b>{candidate.title || "Untitled"}</b><span>{candidate.artist || "Unknown artist"}</span><small>{[candidate.album, candidate.year || candidate.release_date?.slice(0, 4), candidate.track_number ? `Track ${candidate.track_number}` : ""].filter(Boolean).join(" · ")}</small><em>{candidateSources(candidate)}</em></div>
    <div className="candidate-score"><strong>{Math.round(candidate.score)}%</strong><progress className="score-track" max="100" value={Math.max(0, Math.min(100, candidate.score))} aria-label={`${Math.round(candidate.score)} percent match`} /><small>{audit.coreComplete ? "Complete metadata" : `${audit.score}% complete`}</small></div>
    <button className="accept-button" onClick={onChoose}><Icon name="check" size={15} />Accept match</button>
  </article>;
}

function CompareRow({ label, before, after }: { label: string; before?: string; after?: string }) {
  const changed = Boolean(after && after !== before);
  return <div className="compare-row"><span>{label}</span><p className={!before ? "empty" : ""}>{before || "Not set"}</p><Icon name="arrow" size={14} /><p className={`${!after ? "empty" : ""} ${changed ? "changed" : ""}`}>{after || before || "Not found"}</p></div>;
}

function QualityItem({ icon, label, value }: { icon: IconName; label: string; value: string }) {
  return <div><Icon name={icon} /><span><small>{label}</small><b>{value}</b></span></div>;
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
  const [feedback, setFeedback] = useState<{ error?: boolean; text: string }>();
  const [resolving, setResolving] = useState(false);
  const [saving, setSaving] = useState(false);
  const resolveSource = async () => {
    setResolving(true); setFeedback(undefined);
    try {
      const found = await api<Candidate>("/source/resolve", { method: "POST", body: JSON.stringify({ url: sourceUrl }) });
      setForm((current) => ({ ...current, title: found.title || current.title, artist: found.artist || current.artist, album: found.album || current.album, album_artist: found.album_artist || current.album_artist, track_number: found.track_number || current.track_number, year: found.year || current.year, genre: found.genre || current.genre, cover_url: found.cover_url || current.cover_url }));
      setFeedback({ text: `Loaded metadata and artwork for ${found.artist || "this track"}.` });
    } catch (reason) { setFeedback({ error: true, text: (reason as Error).message }); }
    finally { setResolving(false); }
  };
  const save = async () => {
    setSaving(true); setFeedback(undefined);
    try {
      await api(`/tracks/${track.id}/manual`, { method: "PUT", body: JSON.stringify({ ...form, track_number: form.track_number ? Number(form.track_number) : null }) });
      await onSaved();
    } catch (reason) { setFeedback({ error: true, text: (reason as Error).message }); }
    finally { setSaving(false); }
  };
  const labels: Record<string, string> = { title: "Title", artist: "Artist", album: "Album", album_artist: "Album artist", track_number: "Track number", year: "Release year", genre: "Genre", cover_url: "Cover artwork URL" };
  return <section className="manual-editor" aria-labelledby="manual-title">
    <div className="section-heading"><div><p className="eyebrow">Manual correction</p><h3 id="manual-title">Edit proposed metadata</h3></div></div>
    <div className="source-resolver"><label><span>Use a track link</span><input value={sourceUrl} onChange={(event) => setSourceUrl(event.target.value)} placeholder="Spotify, SoundCloud, Genius, Radio Javan, or YouTube URL" /></label><button className="compact-button" disabled={!sourceUrl.trim() || resolving} onClick={() => void resolveSource()}>{resolving ? "Loading…" : "Import"}</button></div>
    {feedback && <p className={`inline-feedback ${feedback.error ? "error" : "success"}`} role={feedback.error ? "alert" : "status"}>{feedback.text}</p>}
    <div className="editor-grid">{Object.entries(form).map(([name, value]) => <label className={name === "cover_url" ? "wide" : ""} key={name}><span>{labels[name]}</span><input value={value} onChange={(event) => setForm({ ...form, [name]: event.target.value })} /></label>)}</div>
    <button className="primary-action editor-save" disabled={saving || !String(form.title).trim() || !String(form.artist).trim()} onClick={() => void save()}>{saving ? <span className="spinner" /> : <Icon name="check" />}Use this metadata</button>
  </section>;
}

function SettingsDrawer({ setup, setSetup, keys, setKeys, saving, onSave, onClose }: { setup: Setup; setSetup: (setup: Setup) => void; keys: Record<string, string>; setKeys: (keys: Record<string, string>) => void; saving: boolean; onSave: () => Promise<void>; onClose: () => void }) {
  return <div className="drawer-layer" role="presentation">
    <button className="drawer-backdrop" onClick={onClose} aria-label="Close settings" />
    <aside className="settings-drawer" role="dialog" aria-modal="true" aria-labelledby="settings-title">
      <header><div><p className="eyebrow">Studio</p><h2 id="settings-title">Settings</h2></div><button className="icon-button" onClick={onClose} aria-label="Close settings" autoFocus><Icon name="x" /></button></header>
      <div className="drawer-content">
        <section><h3>File locations</h3><label><span>Music folder</span><input value={setup.input_dir} onChange={(event) => setSetup({ ...setup, input_dir: event.target.value })} /></label><label><span>Corrected copies</span><input value={setup.output_dir} onChange={(event) => setSetup({ ...setup, output_dir: event.target.value })} /></label></section>
        <section><h3>Original files</h3><label className={`danger-toggle ${setup.delete_source_after_write ? "enabled" : ""}`}><input type="checkbox" checked={setup.delete_source_after_write} onChange={(event) => setSetup({ ...setup, delete_source_after_write: event.target.checked })} /><span className="toggle-ui" /><span><b>{setup.delete_source_after_write ? "Remove after successful output" : "Preserve originals"}</b><small>{setup.delete_source_after_write ? "Each source is deleted only after its corrected copy is written successfully." : "Recommended. Corrected files are written as separate copies."}</small></span></label></section>
        <ProviderStatus sources={setup.sources} />
        <details className="source-key-settings"><summary>Optional source credentials <Icon name="chevron" size={16} /></summary><p>Free catalogs work without keys. Add credentials only to improve hard-to-identify tracks.</p><div className="key-grid"><Secret label="AcoustID" active={setup.sources.acoustid} value={keys.acoustid} onChange={(value) => setKeys({ ...keys, acoustid: value })} /><Secret label="AudD token" active={setup.sources.audd} value={keys.audd} onChange={(value) => setKeys({ ...keys, audd: value })} /><Secret label="Spotify client ID" active={setup.sources.spotify} value={keys.spotify_client_id} onChange={(value) => setKeys({ ...keys, spotify_client_id: value })} /><Secret label="Spotify client secret" active={setup.sources.spotify} value={keys.spotify_client_secret} onChange={(value) => setKeys({ ...keys, spotify_client_secret: value })} /><Secret label="SoundCloud client ID" active={setup.sources.soundcloud_search} value={keys.soundcloud_client_id} onChange={(value) => setKeys({ ...keys, soundcloud_client_id: value })} /><Secret label="SoundCloud secret" active={setup.sources.soundcloud_search} value={keys.soundcloud_client_secret} onChange={(value) => setKeys({ ...keys, soundcloud_client_secret: value })} /><Secret label="YouTube API key" active={setup.sources.youtube} value={keys.youtube} onChange={(value) => setKeys({ ...keys, youtube: value })} /><Secret label="Discogs token" active={setup.sources.discogs} value={keys.discogs} onChange={(value) => setKeys({ ...keys, discogs: value })} /><Secret label="Last.fm key" active={setup.sources.lastfm} value={keys.lastfm} onChange={(value) => setKeys({ ...keys, lastfm: value })} /><Secret label="TheAudioDB key" active={setup.sources.theaudiodb} value={keys.theaudiodb} onChange={(value) => setKeys({ ...keys, theaudiodb: value })} /></div></details>
      </div>
      <footer><button className="primary-action" disabled={saving || !setup.input_dir.trim() || !setup.output_dir.trim()} onClick={() => void onSave()}>{saving ? <span className="spinner" /> : <Icon name="check" />}Save settings</button></footer>
    </aside>
  </div>;
}

function ProviderStatus({ sources }: { sources: Record<string, boolean> }) {
  const available = Object.values(sources).filter(Boolean).length;
  return <section className="provider-settings"><div className="section-heading"><div><h3>Recognition tools</h3><p>{available} sources and local tools available</p></div><span className="health-dot">Ready</span></div><div className="provider-pills">{["musicbrainz", "deezer", "itunes", "radiojavan", "genius", "spotify", "acoustid", "ffmpeg"].map((source) => <span className={sources[source] ? "active" : "inactive"} key={source}><i />{providerName(source)}</span>)}</div>{!sources.ffmpeg && <p className="tool-warning"><Icon name="alert" size={15} />Install FFmpeg for audio integrity checks and ReplayGain. Metadata cleaning still works.</p>}</section>;
}

function ProcessingDock({ workflow, counts, busy, deleteSources, onStop, onWrite }: { workflow?: Workflow; counts: { review: number; ready: number; problems: number; completed: number }; busy: boolean; deleteSources: boolean; onStop: () => void; onWrite: () => void }) {
  const percent = workflow?.total ? Math.min(100, Math.round((workflow.processed / workflow.total) * 100)) : 0;
  return <footer className={`processing-dock ${busy ? "working" : ""}`}>
    <div className="dock-operation"><span className="operation-icon">{busy ? <span className="equalizer"><i/><i/><i/></span> : <Icon name={workflow?.phase === "failed" ? "alert" : "disc"} />}</span><span><b>{busy ? workflow?.message || "Processing" : workflow?.phase === "failed" ? "Processing stopped" : counts.completed ? "Library cleaned" : "Studio ready"}</b><small>{busy ? workflow?.current_file || "Preparing your music…" : counts.ready ? `${counts.ready} ${counts.ready === 1 ? "track" : "tracks"} ready to write` : counts.review ? `${counts.review} waiting for review` : "Select a folder or inspect the queue"}</small></span></div>
    <div className="dock-progress"><progress max="100" value={percent} aria-label={`${percent}% complete`} /><small>{busy ? `${workflow?.processed || 0} of ${workflow?.total || 0}` : `${counts.completed} cleaned · ${counts.review} review · ${counts.problems} problems`}</small></div>
    <div className="dock-actions">{busy ? <button className="compact-button" onClick={onStop}><Icon name="pause" size={15} />Stop</button> : <button className="primary-action" disabled={!counts.ready} onClick={onWrite}><Icon name="sparkles" />Write {counts.ready} corrected {counts.ready === 1 ? "file" : "files"}</button>}<span className={deleteSources ? "delete-note" : "safe-note"}><Icon name={deleteSources ? "trash" : "shield"} size={14} />{deleteSources ? "Originals removed after success" : "Originals stay untouched"}</span></div>
  </footer>;
}

function TrackStatus({ track }: { track: Track }) {
  const status = statusFor(track);
  return <span className={`status-pill ${status.tone}`}><Icon name={status.icon} size={13} />{status.label}</span>;
}

function EmptyState({ icon, title, description, action, onAction }: { icon: IconName; title: string; description: string; action?: string; onAction?: () => void }) {
  return <div className="empty-state"><span><Icon name={icon} size={28} /></span><h2>{title}</h2><p>{description}</p>{action && <button className="compact-button" onClick={onAction}><Icon name="refresh" size={15} />{action}</button>}</div>;
}

function InspectorEmpty({ busy }: { busy: boolean }) {
  return <div className="inspector-empty"><span><Icon name={busy ? "waveform" : "disc"} size={32} /></span><h2>{busy ? "Building your queue" : "Select a track"}</h2><p>{busy ? "Identification results will appear while the scan continues." : "Inspect the original tags, proposed corrections, artwork, sources, and confidence here."}</p></div>;
}

function QueueSkeleton() { return <div className="skeleton-list" aria-label="Loading tracks">{[1, 2, 3, 4, 5].map((item) => <div key={item}><i /><span><b /><small /></span><em /></div>)}</div>; }

function Secret({ label, active, value, onChange }: { label: string; active?: boolean; value?: string; onChange: (value: string) => void }) {
  return <label><span>{label}{active && <i>Configured</i>}</span><input type="password" value={value || ""} onChange={(event) => onChange(event.target.value)} placeholder={active ? "Leave blank to keep" : "Optional"} autoComplete="off" /></label>;
}

function Artwork({ candidate, trackId, size = "small" }: { candidate?: Candidate; trackId?: number; size?: "small" | "medium" | "large" }) {
  const urls = trackId ? [`/api/tracks/${trackId}/artwork/preview?v=${encodeURIComponent(candidate?.cover_url || candidate?.id || "embedded")}`]
    : candidate?.id ? [`/api/candidates/${candidate.id}/artwork/preview?v=${encodeURIComponent(candidate.cover_url || candidate.id)}`, ...artworkUrls(candidate)] : artworkUrls(candidate);
  const [index, setIndex] = useState(0);
  useEffect(() => setIndex(0), [candidate?.id, candidate?.cover_url]);
  return urls[index] ? <img className={`artwork artwork-${size}`} src={urls[index]} alt={`Cover for ${candidate?.album || candidate?.title || "track"}`} loading="lazy" onError={() => setIndex((current) => current + 1)} />
    : <span className={`artwork artwork-${size} artwork-missing`}><Icon name="music" size={size === "large" ? 28 : 18} /></span>;
}

function GoogleCheck({ candidate }: { candidate: Candidate }) {
  return <a className="google-check" href={googleMetadataUrl(candidate)} target="_blank" rel="noreferrer"><span>Verify release details on Google</span><Icon name="arrow" size={15} /></a>;
}

function MetadataHealth({ candidate }: { candidate: Candidate }) {
  const audit = metadataAudit(candidate);
  return <span className={`metadata-health ${audit.coreComplete ? "complete" : "incomplete"}`}><Icon name={audit.coreComplete ? "check" : "alert"} size={14} />{audit.score}% metadata {audit.coreComplete ? "complete" : `· missing ${audit.missing.slice(0, 3).join(", ")}`}</span>;
}

function isReview(track: Track) { return track.stage === "review" && !isCompleted(track); }
function isReady(track: Track) { return track.stage === "ready" && Boolean(track.selected_candidate_id) && !isCompleted(track); }
function isCompleted(track: Track) { return track.status === "applied"; }
function isProblem(track: Track) { return track.status === "corrupt" || track.is_missing || track.stage === "failed" || track.status === "failed" || track.status === "provider_error"; }
function selectedCandidate(track: Track) { return track.candidates.find((candidate) => candidate.id === track.selected_candidate_id); }

function statusFor(track: Track): { label: string; tone: string; icon: IconName } {
  if (isCompleted(track)) return { label: "Cleaned", tone: "success", icon: "check" };
  if (track.status === "corrupt") return { label: "Damaged", tone: "error", icon: "alert" };
  if (track.is_missing) return { label: "File missing", tone: "error", icon: "alert" };
  if (track.stage === "failed" || track.status === "failed" || track.status === "provider_error") return { label: "Failed", tone: "error", icon: "alert" };
  if (track.stage === "review") return { label: track.candidates.length > 1 ? "Needs review" : track.candidates.length ? "Uncertain" : "Not identified", tone: "review", icon: "info" };
  if (isReady(track)) return { label: "Ready", tone: "success", icon: "check" };
  if (track.stage === "skipped") return { label: "Skipped", tone: "muted", icon: "skip" };
  return { label: "Processing", tone: "processing", icon: "waveform" };
}

function problemTitle(track: Track) {
  if (track.status === "corrupt") return "Damaged audio file";
  if (track.is_missing) return "Source file is missing";
  return "Could not process this track";
}

function filterTitle(filter: QueueFilter) { return ({ all: "Music queue", review: "Needs review", problems: "Problems", completed: "Completed" })[filter]; }
function fileStem(filename: string) { return filename.replace(/\.[^/.]+$/, ""); }
function outputFilename(track: Track, candidate: Candidate) { const extension = track.filename.includes(".") ? `.${track.filename.split(".").pop()?.toLowerCase()}` : ""; return `${safeName(candidate.artist || "Unknown Artist")} - ${safeName(candidate.title || "Unknown Title")}${extension}`; }
function safeName(value: string) { return value.replace(/[\\/:*?"<>|]/g, " ").replace(/\s+/g, " ").replace(/^[ .]+|[ .]+$/g, ""); }
function friendlyError(value: string) { if (/fetch|network|load failed/i.test(value)) return "The local service is not responding. Make sure ./dev.sh is running, then reconnect."; if (/permission|denied/i.test(value)) return `${value} Check that Ununknown can read the music folder and write to the output folder.`; return value; }

function artworkUrls(candidate?: Candidate) {
  const urls: string[] = candidate?.cover_url ? [candidate.cover_url] : [];
  try { const alternatives = JSON.parse(candidate?.score_breakdown || "{}")?.artwork_candidates || []; for (const artwork of alternatives) if (typeof artwork?.url === "string") urls.push(artwork.url); } catch { /* Old evidence can be malformed. */ }
  return [...new Set(urls)];
}

function googleMetadataUrl(candidate: Candidate) { const artist = String(candidate.artist || "").replaceAll('"', ""); const title = String(candidate.title || "").replaceAll('"', ""); return `https://www.google.com/search?q=${encodeURIComponent(`"${artist}" "${title}" song album or single album name original release year`)}`; }

function candidateSources(candidate: Candidate) {
  try { const sources = JSON.parse(candidate.score_breakdown || "{}")?.sources; if (Array.isArray(sources) && sources.length) return [...new Set<string>(sources.filter((source: unknown): source is string => typeof source === "string").map(providerName))].join(" + "); } catch { /* Use primary provider. */ }
  return providerName(candidate.provider || "catalog");
}

function providerName(source: string) { const names: Record<string, string> = { acoustid: "AcoustID", audd: "AudD", deezer: "Deezer", discogs: "Discogs", itunes: "Apple Music", lastfm: "Last.fm", genius: "Genius", musicbrainz: "MusicBrainz", radiojavan: "Radio Javan", soundcloud: "SoundCloud", spotify: "Spotify", theaudiodb: "TheAudioDB", wikidata: "Wikidata", youtube: "YouTube", ffmpeg: "FFmpeg", manual: "Manual entry", catalog: "Catalog" }; return names[source] || source; }

function metadataAudit(candidate: Candidate) {
  try { const stored = JSON.parse(candidate.score_breakdown || "{}")?.metadata_completion; if (stored && typeof stored.score === "number" && Array.isArray(stored.missing_fields)) return { score: stored.score, coreComplete: stored.core_complete === true, missing: stored.missing_fields as string[] }; } catch { /* Derive a display audit. */ }
  const fields: Array<[string, boolean, number]> = [["title", Boolean(candidate.title?.trim()), 18], ["artist", Boolean(candidate.artist?.trim()), 18], ["album", Boolean(candidate.album?.trim()), 16], ["cover", Boolean(candidate.cover_url?.trim()), 16], ["year", Boolean(candidate.year?.trim() || candidate.release_date?.trim()), 10], ["genre", Boolean(candidate.genre?.trim()), 8], ["track", Boolean(candidate.track_number), 6], ["album artist", Boolean(candidate.album_artist?.trim()), 4], ["ISRC", Boolean(candidate.isrc?.trim()), 4]];
  return { score: fields.filter(([, present]) => present).reduce((total, [, , weight]) => total + weight, 0), coreComplete: fields.slice(0, 4).every(([, present]) => present), missing: fields.filter(([, present]) => !present).map(([name]) => name) };
}
