import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "@/api/client";
import type { AutoApproveResult, Candidate, RetryIssuesResult, Setup, Track, TrackPage, Workflow } from "@/api/types";
import { Icon, type IconName } from "./Icons";

const emptySetup: Setup = { input_dir: "", output_dir: "", delete_source_after_write: false, sources: {} };
const busyPhases = new Set(["scan", "fetch", "apply"]);
type QueueFilter = "all" | "review" | "problems" | "ready";
type QueueOrder = "queue" | "title" | "artist" | "status";
type Theme = "dark" | "light";

const queueOrderLabels: Record<QueueOrder, string> = { queue: "Queue", title: "Title", artist: "Artist", status: "Status" };
const queueOrderSequence: QueueOrder[] = ["queue", "title", "artist", "status"];

export function App() {
  const [setup, setSetup] = useState<Setup>(emptySetup);
  const [keys, setKeys] = useState<Record<string, string>>({});
  const [workflow, setWorkflow] = useState<Workflow>();
  const [tracks, setTracks] = useState<Track[]>([]);
  const [selectedId, setSelectedId] = useState<number>();
  const [filter, setFilter] = useState<QueueFilter>("all");
  const [queueOrder, setQueueOrder] = useState<QueueOrder>("queue");
  const [query, setQuery] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [loading, setLoading] = useState(true);
  const [connected, setConnected] = useState(true);
  const [saving, setSaving] = useState(false);
  const [autoApproving, setAutoApproving] = useState(false);
  const [retryingIssues, setRetryingIssues] = useState(false);
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
      return preferredTrack(page.items)?.id;
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
      setSelectedId((current) => current ?? preferredTrack(page.items)?.id);
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
    if (!notice) return;
    const timer = window.setTimeout(() => setNotice(""), 5000);
    return () => window.clearTimeout(timer);
  }, [notice]);

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

  const retryIssues = async () => {
    const damaged = tracks.filter((track) => isProblem(track) && track.status === "corrupt").length;
    if (damaged && !confirm(`Ununknown will try to salvage ${damaged} damaged ${damaged === 1 ? "file" : "files"} by skipping unreadable frames and re-encoding the valid audio. Each damaged original will be kept beside the repaired file as an .ununknown-damaged backup. Continue?`)) return;
    setRetryingIssues(true);
    setError("");
    setNotice("");
    try {
      const result = await api<RetryIssuesResult>("/tracks/retry-issues", { method: "POST", body: "{}" });
      await refresh();
      if (result.started) {
        setNotice([
          `Checking ${result.queued} ${result.queued === 1 ? "file" : "files"}, repairing damaged streams, and retrying identification.`,
          result.unavailable ? `${result.unavailable} still missing.` : "",
        ].filter(Boolean).join(" "));
      } else {
        setNotice(result.unavailable ? `${result.unavailable} source ${result.unavailable === 1 ? "file is" : "files are"} still missing. Restore them to their original locations, then check again.` : "No issues need to be checked.");
      }
    } catch (reason) {
      setError((reason as Error).message);
    } finally {
      setRetryingIssues(false);
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

  const stop = async () => {
    setError("");
    try {
      await api("/stop", { method: "POST", body: "{}" });
      setNotice("Stopping safely after the current operation.");
      await refresh();
    } catch (reason) {
      setError((reason as Error).message);
    }
  };

  const visibleTracks = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    const filteredTracks = tracks.filter((track) => {
      const inFilter = filter === "all" || (filter === "review" && isReview(track)) ||
        (filter === "problems" && isProblem(track)) || (filter === "ready" && isReady(track));
      if (!inFilter) return false;
      if (!normalized) return true;
      const candidate = selectedCandidate(track) ?? track.candidates[0];
      return [track.filename, track.current_title, track.current_artist, track.current_album, candidate?.title, candidate?.artist, candidate?.album]
        .some((value) => value?.toLocaleLowerCase().includes(normalized));
    });
    if (queueOrder === "queue") return filteredTracks;
    return [...filteredTracks].sort((first, second) => compareTracks(first, second, queueOrder));
  }, [filter, query, queueOrder, tracks]);

  const cycleQueueOrder = () => {
    const currentIndex = queueOrderSequence.indexOf(queueOrder);
    setQueueOrder(queueOrderSequence[(currentIndex + 1) % queueOrderSequence.length]);
  };

  const selected = tracks.find((track) => track.id === selectedId);

  useEffect(() => {
    if (visibleTracks.some((track) => track.id === selectedId)) return;
    setSelectedId(visibleTracks[0]?.id);
  }, [selectedId, visibleTracks]);

  const changeFilter = (next: QueueFilter) => {
    setFilter(next);
    const nextTrack = next === "all" ? preferredTrack(tracks) : tracks.find((track) => (next === "review" && isReview(track)) || (next === "problems" && isProblem(track)) || (next === "ready" && isReady(track)));
    setSelectedId(nextTrack?.id);
  };

  const selectTrack = (track: Track) => {
    setSelectedId(track.id);
    setMobileInspector(true);
  };

  return (
    <div className="studio-app">
      <header className="topbar">
        <div className="topbar-main">
          <div className="topbar-leading">
            <a className="brand" href="#workspace" aria-label="Ununknown studio home">
              <span className="brand-mark"><Icon name="waveform" size={18} /></span>
              <span>Ununknown</span>
            </a>
            <button className="topbar-source-summary" onClick={() => setSettingsOpen(true)} aria-label="Change music source" title={setup.input_dir || "Choose a music folder"}>
              <span className="source-icon"><Icon name="folder" size={16} /></span>
              <span className="source-copy">
                <b>{folderName(setup.input_dir)}</b>
                <small>{tracks.length} {tracks.length === 1 ? "file" : "files"}</small>
              </span>
            </button>
          </div>
          <nav className="workspace-nav" aria-label="Workspace views">
            <NavButton className="all-tracks-tab" active={filter === "all"} onClick={() => changeFilter("all")} label="All tracks" count={tracks.length} />
            <div className="status-tabs" role="group" aria-label="Filter tracks by status">
              <NavButton active={filter === "review"} onClick={() => changeFilter("review")} label="Review" count={counts.review} />
              <NavButton active={filter === "problems"} onClick={() => changeFilter("problems")} label="Issues" count={counts.problems} />
              <NavButton active={filter === "ready"} onClick={() => changeFilter("ready")} label="Ready" count={counts.ready} />
            </div>
          </nav>
          <div className="topbar-controls">
            <button className={`topbar-scan-action ${tracks.length ? "" : "prominent"}`.trim()} disabled={busy || saving || !setup.input_dir.trim() || !setup.output_dir.trim()} onClick={identify} title={busy ? workflow?.phase === "apply" ? "Writing corrected files" : "Identifying music" : tracks.length ? "Rescan music folder" : "Start cleaning"}>
              {busy ? <span className="spinner" /> : <Icon name={tracks.length ? "refresh" : "sparkles"} />}
              {busy ? workflow?.phase === "apply" ? "Writing…" : "Identifying…" : tracks.length ? "Rescan" : "Scan folder"}
            </button>
            <div className="topbar-actions">
              {!connected && <span className="connection-state offline"><i />Offline</span>}
              <button className="icon-button" onClick={() => setTheme(theme === "dark" ? "light" : "dark")} aria-label={`Use ${theme === "dark" ? "light" : "dark"} theme`} title="Change theme">
                <Icon name={theme === "dark" ? "sun" : "moon"} />
              </button>
              <button className="icon-button" onClick={() => setSettingsOpen(true)} aria-label="Open settings" title="Settings"><Icon name="settings" /></button>
            </div>
          </div>
        </div>
      </header>

      <nav className="mobile-nav" aria-label="Workspace views">
        <NavButton className="all-tracks-tab" active={filter === "all"} onClick={() => changeFilter("all")} label="All tracks" count={tracks.length} />
        <div className="status-tabs" role="group" aria-label="Filter tracks by status">
          <NavButton active={filter === "review"} onClick={() => changeFilter("review")} label="Review" count={counts.review} />
          <NavButton active={filter === "problems"} onClick={() => changeFilter("problems")} label="Issues" count={counts.problems} />
          <NavButton active={filter === "ready"} onClick={() => changeFilter("ready")} label="Ready" count={counts.ready} />
        </div>
      </nav>

      <div className="announcement-region" aria-live="polite" aria-atomic="true">
        {error && <div className="toast error-toast" role="alert"><Icon name="alert" /><span><b>{connected ? "Couldn’t complete that action" : "Backend unavailable"}</b><small>{friendlyError(error)}</small></span><button onClick={() => setError("")} aria-label="Dismiss error"><Icon name="x" size={16} /></button></div>}
        {notice && <div className="toast notice-toast"><Icon name="check" /><span><b>Done</b><small>{notice}</small></span><button onClick={() => setNotice("")} aria-label="Dismiss notification"><Icon name="x" size={16} /></button></div>}
      </div>

      <main className="workspace" id="workspace">
        <section className="queue-panel" aria-labelledby="queue-title">
          <header className="queue-header">
            <div className="queue-heading"><h1 id="queue-title">{filterTitle(filter)}</h1><span className="queue-count">{visibleTracks.length}</span></div>
            <label className="search-field"><Icon name="search" size={16} /><span className="sr-only">Search queue</span><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search title, artist, album…" /></label>
            <button className="queue-order-button compact-button" onClick={cycleQueueOrder} aria-label={`Order tracks by ${queueOrderLabels[queueOrder]}`} title={`Current order: ${queueOrderLabels[queueOrder]}. Click to change.`}><Icon name="menu" size={14} /><span>{queueOrderLabels[queueOrder]}</span></button>
            {autoApprovable > 0 && filter === "review" && <button className="compact-button accent" disabled={autoApproving} onClick={autoApprove}><Icon name="sparkles" size={15} />{autoApproving ? "Checking…" : `Auto-select ${autoApprovable}`}</button>}
            {counts.problems > 0 && filter === "problems" && <button className="compact-button accent issue-retry-button" disabled={busy || retryingIssues} onClick={() => void retryIssues()}>{retryingIssues || busy ? <span className="spinner" /> : <Icon name="refresh" size={15} />}{retryingIssues || busy ? "Checking…" : `Check & fix ${counts.problems}`}</button>}
          </header>

          <div className="track-list" role="list" aria-label="Music queue">
            {loading ? <QueueSkeleton /> : !connected && tracks.length === 0 ? <EmptyState icon="alert" title="The studio is offline" description="Start the local Rust backend, then reconnect to load your workspace." action="Reconnect" onAction={loadApp} />
              : tracks.length === 0 ? <EmptyState icon={workflow?.phase === "finish" ? "check" : "music"} title={busy ? "Listening to your library" : workflow?.phase === "finish" ? "Cleaning complete" : "Your queue is ready"} description={busy ? "Audio files will appear here as they are discovered and identified." : workflow?.phase === "finish" ? `Corrected files are available in ${setup.output_dir || "your output folder"}.` : "Enter a music folder above, choose where corrected copies should go, then start cleaning."} />
              : visibleTracks.length === 0 ? <EmptyState icon={filter === "review" || filter === "ready" ? "check" : "search"} title={filter === "review" ? "Nothing needs review" : filter === "ready" ? "No tracks are ready yet" : "No tracks found"} description={filter === "review" ? "All current matches are resolved. Your library is ready for the next step." : filter === "ready" ? "Accept a match or enter metadata manually to prepare a track for writing." : "Try another search or switch workspace views."} />
              : visibleTracks.map((track) => <TrackRow key={track.id} track={track} active={track.id === selectedId} onClick={() => selectTrack(track)} />)}
          </div>
        </section>

        <aside className={`inspector-panel ${mobileInspector ? "mobile-open" : ""}`} aria-label="Metadata inspector">
          <button className="mobile-close icon-button" onClick={() => setMobileInspector(false)} aria-label="Close inspector"><Icon name="x" /></button>
          {selected ? <TrackInspector track={selected} onChoose={choose} onSaved={loadTracks} /> : <InspectorEmpty busy={busy} />}
        </aside>
      </main>

      <ProcessingDock workflow={workflow} counts={counts} busy={busy} deleteSources={setup.delete_source_after_write} onStop={() => void stop()} onWrite={() => void write()} />

      {settingsOpen && <SettingsDrawer setup={setup} setSetup={setSetup} keys={keys} setKeys={setKeys} saving={saving} onSave={async () => { try { await saveSetup(); setNotice("Studio settings saved."); setSettingsOpen(false); } catch (reason) { setError((reason as Error).message); } }} onClose={() => setSettingsOpen(false)} />}
    </div>
  );
}

function NavButton({ active, label, count, className = "", onClick }: { active: boolean; label: string; count?: number; className?: string; onClick: () => void }) {
  return <button className={`${className}${active ? " active" : ""}`.trim()} aria-current={active ? "page" : undefined} onClick={onClick}>{label}{Boolean(count) && <span>{count}</span>}</button>;
}

function TrackRow({ track, active, onClick }: { track: Track; active: boolean; onClick: () => void }) {
  const candidate = selectedCandidate(track) ?? track.candidates[0];
  const displayTitle = candidate?.title || track.current_title || fileStem(track.filename);
  const artist = candidate?.artist || track.current_artist || "Unknown artist";
  const score = candidate?.score;
  return (
    <button className={`track-row ${active ? "selected" : ""}`} onClick={onClick} role="listitem" aria-current={active ? "true" : undefined}>
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
  const [choosingId, setChoosingId] = useState<number>();
  const [actionError, setActionError] = useState("");
  const candidate = selectedCandidate(track);
  const reviewCandidate = track.candidates[0];
  const heroCandidate = candidate ?? reviewCandidate;
  const corrupt = track.status === "corrupt";
  const undoIdentification = async () => {
    setUndoing(true); setActionError("");
    try {
      await api(`/tracks/${track.id}/review`, { method: "POST", body: "{}" });
      await onSaved();
    } catch (reason) { setActionError((reason as Error).message); }
    finally { setUndoing(false); }
  };
  const acceptCandidate = async (candidateId: number) => {
    setChoosingId(candidateId); setActionError("");
    try { await onChoose(track.id, candidateId); }
    catch (reason) { setActionError((reason as Error).message); }
    finally { setChoosingId(undefined); }
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
        <div><b>{isProblem(track) ? problemTitle(track) : "Your review is needed"}</b><p>{track.stage_message || friendlyTrackError(track) || "The available matches are too close to choose safely."}</p>{track.error && track.stage_message && <details className="technical-error"><summary>Technical details</summary><code>{track.error}</code></details>}</div>
      </section>}

      {isReview(track) && track.candidates.length > 0 && <section className="inspector-section candidate-section">
        <div className="section-heading"><div><p className="eyebrow">Review matches</p><h3>Compare with the original file</h3></div><span>{track.candidates.length} found</span></div>
        <p className="candidate-help">Scores can be close even when the recordings are different. Compare the title, artist, album, and audio evidence before choosing.</p>
        <div className="candidate-reference">
          <div className="reference-file"><Icon name="music" size={15} /><span><small>Original file</small><b title={track.filename}>{track.filename}</b></span></div>
          <ReferenceField label="Title" value={track.current_title} />
          <ReferenceField label="Artist" value={track.current_artist} />
          <ReferenceField label="Album" value={track.current_album} />
          <ReferenceField label="Track" value={track.current_track_number?.toString()} />
        </div>
        <div className="candidate-list">
          {track.candidates.slice(0, 5).map((item, index) => <CandidateRow key={item.id} track={track} candidate={item} rank={index + 1} topScore={track.candidates[0]?.score || item.score} nextScore={index === 0 ? track.candidates[1]?.score : undefined} choosing={choosingId === item.id} disabled={choosingId !== undefined} onChoose={() => void acceptCandidate(item.id)} />)}
        </div>
      </section>}

      {actionError && <p className="inline-feedback error" role="alert">{actionError}</p>}

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

function ReferenceField({ label, value }: { label: string; value?: string }) {
  return <span className="reference-field"><small>{label}</small><b title={value}>{value || "Not set"}</b></span>;
}

function CandidateRow({ track, candidate, rank, topScore, nextScore, choosing, disabled, onChoose }: { track: Track; candidate: Candidate; rank: number; topScore: number; nextScore?: number; choosing: boolean; disabled: boolean; onChoose: () => void }) {
  const audit = metadataAudit(candidate);
  const confidence = candidateConfidence(candidate.score);
  const signals = candidateSignals(track, candidate);
  const gap = rank === 1 && typeof nextScore === "number" ? Math.max(0, candidate.score - nextScore) : Math.max(0, topScore - candidate.score);
  return <article className="candidate-row">
    <div className="candidate-main">
      <span className="candidate-rank" aria-label={`Result ${rank}`}>{rank}</span>
      <Artwork candidate={candidate} size="medium" />
      <div className="candidate-identity"><b>{candidate.title || "Untitled"}</b><span>{candidate.artist || "Unknown artist"}</span><small>{[candidate.album || "Album unknown", candidate.year || candidate.release_date?.slice(0, 4), candidate.track_number ? `Track ${candidate.track_number}${candidate.track_total ? ` of ${candidate.track_total}` : ""}` : ""].filter(Boolean).join(" · ")}</small><em>{candidateSources(candidate)} · {audit.coreComplete ? "Complete metadata" : `${audit.score}% metadata`}</em></div>
      <div className={`candidate-score ${confidence.tone}`}><span>{confidence.label}</span><strong>{Math.round(candidate.score)}%</strong><small>{rank === 1 ? typeof nextScore === "number" ? `${Math.round(gap)} points above #2` : "Only result" : `${Math.round(gap)} points below #1`}</small></div>
      <button className="accept-button" disabled={disabled} onClick={onChoose}>{choosing ? <span className="spinner" /> : <Icon name="check" size={15} />}{choosing ? "Choosing…" : "Use this match"}</button>
    </div>
    <div className="candidate-evidence" aria-label="Comparison with original file">
      {signals.map((signal) => <span className={signal.tone} key={signal.label}><small>{signal.label}</small><b>{signal.value}</b></span>)}
    </div>
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
  const completedOperations = workflow?.phase === "apply" || workflow?.phase === "finish"
    ? workflow.current
    : Math.max(workflow?.processed || 0, workflow?.current || 0);
  const percent = workflow?.total ? Math.min(100, Math.round((completedOperations / workflow.total) * 100)) : 0;
  return <footer className={`processing-dock ${busy ? "working" : ""}`}>
    <div className="dock-operation"><span className="operation-icon">{busy ? <span className="equalizer"><i/><i/><i/></span> : <Icon name={workflow?.phase === "failed" ? "alert" : "disc"} />}</span><span><b>{busy ? workflow?.message || "Processing" : workflow?.phase === "failed" ? "Processing stopped" : workflow?.phase === "finish" ? "Writing complete" : "Studio ready"}</b><small>{busy ? workflow?.current_file || "Preparing your music…" : workflow?.phase === "finish" ? `${workflow.current || workflow.total} corrected files written` : counts.ready ? `${counts.ready} ${counts.ready === 1 ? "track" : "tracks"} ready to write` : counts.review ? `${counts.review} waiting for review` : "Select a folder or inspect the queue"}</small></span></div>
    <div className="dock-progress"><progress max="100" value={percent} aria-label={`${percent}% complete`} /><small>{busy ? `${completedOperations} of ${workflow?.total || 0} · ${percent}%` : `${counts.ready} ready · ${counts.review} review · ${counts.problems} problems`}</small></div>
    <div className="dock-actions">{busy ? <button className="compact-button" onClick={onStop}><Icon name="pause" size={15} />Stop</button> : <button className="primary-action" disabled={!counts.ready} onClick={onWrite}><Icon name="sparkles" />Write {counts.ready} {counts.ready === 1 ? "file" : "files"}</button>}<span className={deleteSources ? "delete-note" : "safe-note"}><Icon name={deleteSources ? "trash" : "shield"} size={14} />{deleteSources ? "Originals removed after success" : "Originals stay untouched"}</span></div>
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
  const catalogUrls = artworkUrls(candidate);
  const urls = trackId ? [`/api/tracks/${trackId}/artwork/preview?v=${encodeURIComponent(candidate?.cover_url || candidate?.id || "embedded")}`, ...catalogUrls]
    : candidate?.id && catalogUrls.length ? [`/api/candidates/${candidate.id}/artwork/preview?v=${encodeURIComponent(candidate.cover_url || candidate.id)}`, ...catalogUrls] : catalogUrls;
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

function candidateConfidence(score: number) {
  if (score >= 85) return { label: "Strong match", tone: "strong" };
  if (score >= 70) return { label: "Likely match", tone: "likely" };
  if (score >= 55) return { label: "Uncertain", tone: "uncertain" };
  return { label: "Weak match", tone: "weak" };
}

function candidateSignals(track: Track, candidate: Candidate) {
  let evidence: Record<string, unknown> = {};
  try { evidence = JSON.parse(candidate.score_breakdown || "{}"); } catch { /* Compare available text directly. */ }
  return [
    comparisonSignal("Title", track.current_title, candidate.title, evidence.title),
    comparisonSignal("Artist", track.current_artist, candidate.artist, evidence.artist),
    comparisonSignal("Album", track.current_album, candidate.album, evidence.album_context),
    durationSignal(evidence.duration),
  ];
}

function comparisonSignal(label: string, original?: string, proposed?: string, rawSimilarity?: unknown) {
  if (!original?.trim()) return { label, value: "No original", tone: "muted" };
  if (!proposed?.trim()) return { label, value: "Missing", tone: "warning" };
  const similarity = typeof rawSimilarity === "number" ? rawSimilarity : normalizeText(original) === normalizeText(proposed) ? 1 : 0;
  if (similarity >= 0.92) return { label, value: "Exact", tone: "good" };
  if (similarity >= 0.7) return { label, value: "Close", tone: "good" };
  if (similarity >= 0.4) return { label, value: "Different", tone: "warning" };
  return { label, value: "Mismatch", tone: "warning" };
}

function durationSignal(rawSimilarity?: unknown) {
  if (typeof rawSimilarity !== "number") return { label: "Audio length", value: "Not checked", tone: "muted" };
  if (rawSimilarity >= 0.9) return { label: "Audio length", value: "Same", tone: "good" };
  if (rawSimilarity >= 0.55) return { label: "Audio length", value: "Close", tone: "good" };
  return { label: "Audio length", value: "Different", tone: "warning" };
}

function normalizeText(value: string) { return value.toLocaleLowerCase().replace(/[^\p{L}\p{N}]+/gu, "").trim(); }

function isReview(track: Track) { return track.stage === "review" && !isCompleted(track) && !isProblem(track); }
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

function friendlyTrackError(track: Track) {
  if (!track.error) return "";
  if (track.status === "corrupt") return "The audio stream could not be decoded safely. This file will not be written.";
  if (track.is_missing) return "The source file is no longer available at its original location.";
  return "Ununknown could not finish this track. Open technical details if you need the diagnostic message.";
}

function queuePriority(track: Track) {
  if (isReview(track)) return 0;
  if (isProblem(track)) return 1;
  if (isReady(track)) return 2;
  if (isCompleted(track)) return 3;
  return 4;
}

function compareTracks(first: Track, second: Track, order: QueueOrder) {
  if (order === "status") return queuePriority(first) - queuePriority(second) || compareText(trackTitle(first), trackTitle(second));
  if (order === "artist") return compareText(trackArtist(first), trackArtist(second)) || compareText(trackTitle(first), trackTitle(second));
  return compareText(trackTitle(first), trackTitle(second));
}

function trackTitle(track: Track) { const candidate = selectedCandidate(track) ?? track.candidates[0]; return candidate?.title || track.current_title || fileStem(track.filename); }
function trackArtist(track: Track) { const candidate = selectedCandidate(track) ?? track.candidates[0]; return candidate?.artist || track.current_artist || "Unknown artist"; }
function compareText(first: string, second: string) { return first.localeCompare(second, undefined, { numeric: true, sensitivity: "base" }); }

function preferredTrack(tracks: Track[]) {
  return [...tracks].sort((first, second) => queuePriority(first) - queuePriority(second) || first.filename.localeCompare(second.filename))[0];
}

function filterTitle(filter: QueueFilter) { return ({ all: "Music queue", review: "Needs review", problems: "Problems", ready: "Ready to write" })[filter]; }
function folderName(path: string) { const parts = path.split(/[\\/]/).filter(Boolean); return parts[parts.length - 1] || "Choose folder"; }
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
