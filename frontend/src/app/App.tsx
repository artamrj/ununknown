import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "@/api/client";
import type {
  AutoApproveResult,
  RetryIssuesResult,
  Setup,
  Track,
  TrackPage,
  Workflow,
} from "@/api/types";
import { Icon } from "./Icons";
import { NavButton } from "./components/NavButton";
import { TrackRow } from "./components/TrackRow";
import { TrackInspector, InspectorEmpty } from "./components/TrackInspector";
import { SettingsDrawer } from "./components/SettingsDrawer";
import { ProcessingDock } from "./components/ProcessingDock";
import { EmptyState, QueueSkeleton } from "./components/EmptyStates";
import type { QueueFilter, QueueOrder, Theme } from "./trackUtils";
import {
  busyPhases,
  compareTracks,
  filterTitle,
  folderName,
  friendlyError,
  hasRetryableArtwork,
  isCompleted,
  isProblem,
  isReady,
  isReview,
  preferredTrack,
  queueOrderLabels,
  queueOrderSequence,
  selectedCandidate,
} from "./trackUtils";

const emptySetup: Setup = {
  input_dir: "",
  output_dir: "",
  delete_source_after_write: false,
  automatic_scan_enabled: true,
  automatic_scan_interval_minutes: 5,
  sources: {},
};

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
  const lastPhaseRef = useRef<string>("");
  const trackCache = useRef(new Map<number, Track>());
  const lastTracksRef = useRef<Track[] | undefined>(undefined);
  const lastStatusRef = useRef<Workflow | undefined>(undefined);
  const busy = workflow ? busyPhases.has(workflow.phase) : false;

  const mergeTracks = useCallback((next: Track[]) => {
    const cache = trackCache.current;
    const merged = next.map((track) => {
      const cached = cache.get(track.id);
      if (cached && JSON.stringify(cached) === JSON.stringify(track)) return cached;
      cache.set(track.id, track);
      return track;
    });
    const previous = lastTracksRef.current;
    if (
      previous &&
      previous.length === merged.length &&
      merged.every((track, index) => track === previous[index])
    ) {
      return previous;
    }
    lastTracksRef.current = merged;
    return merged;
  }, []);

  const mergeStatus = useCallback((next: Workflow) => {
    const cached = lastStatusRef.current;
    if (cached && JSON.stringify(cached) === JSON.stringify(next)) return cached;
    lastStatusRef.current = next;
    return next;
  }, []);

  const loadTracks = useCallback(async () => {
    const page = await api<TrackPage>("/tracks");
    const merged = mergeTracks(page.items);
    setTracks(merged);
    setSelectedId((current) => {
      if (current && merged.some((track) => track.id === current)) return current;
      return preferredTrack(merged)?.id;
    });
  }, [mergeTracks]);

  const loadApp = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const [nextSetup, status, page] = await Promise.all([
        api<Setup>("/setup"),
        api<Workflow>("/status"),
        api<TrackPage>("/tracks"),
      ]);
      setSetup(nextSetup);
      setWorkflow(mergeStatus(status));
      setTracks(mergeTracks(page.items));
      setSelectedId((current) => current ?? preferredTrack(page.items)?.id);
      setConnected(true);
    } catch (reason) {
      setConnected(false);
      setError((reason as Error).message || "The local backend could not be reached.");
    } finally {
      setLoading(false);
    }
  }, [mergeStatus, mergeTracks]);

  const refresh = useCallback(async () => {
    try {
      const status = await api<Workflow>("/status");
      setWorkflow(mergeStatus(status));
      setConnected(true);
      const phase = status.phase;
      const phaseChanged = lastPhaseRef.current !== phase;
      lastPhaseRef.current = phase;
      if (phaseChanged || !busyPhases.has(phase)) {
        await loadTracks();
      } else {
        pollCount.current += 1;
        if (pollCount.current % 6 === 0) await loadTracks();
      }
    } catch (reason) {
      setConnected(false);
      setError((reason as Error).message);
    }
  }, [loadTracks, mergeStatus]);

  useEffect(() => {
    void loadApp();
  }, [loadApp]);

  useEffect(() => {
    const reportActivity = () => {
      void api("/activity", { method: "POST", body: "{}" }).catch(() => undefined);
    };
    reportActivity();
    const timer = window.setInterval(reportActivity, 30_000);
    window.addEventListener("focus", reportActivity);
    document.addEventListener("visibilitychange", reportActivity);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", reportActivity);
      document.removeEventListener("visibilitychange", reportActivity);
    };
  }, []);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem("ununknown-theme", theme);
  }, [theme]);

  useEffect(() => {
    if (!busy) return;
    const timer = window.setInterval(() => void refresh(), 900);
    return () => window.clearInterval(timer);
  }, [busy, refresh]);

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
          automatic_scan_enabled: setup.automatic_scan_enabled,
          automatic_scan_interval_minutes: setup.automatic_scan_interval_minutes,
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
    } catch (reason) {
      setError((reason as Error).message);
      throw reason;
    } finally {
      setSaving(false);
    }
  };

  const identify = async () => {
    setNotice("");
    try {
      await saveSetup();
      trackCache.current.clear();
      lastTracksRef.current = undefined;
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
      const result = await api<{ ready: boolean; cover_verified?: boolean }>(
        `/tracks/${trackId}/choose`,
        {
          method: "POST",
          body: JSON.stringify({ candidate_id: candidateId }),
        },
      );
      await loadTracks();
      setNotice(
        !result.ready
          ? "Match accepted. This track still needs required metadata before it can be cleaned."
          : result.cover_verified
            ? "Match accepted. This track is ready to clean."
            : "Match accepted. This track is ready to clean; a cover will be added when one becomes available.",
      );
    } catch (reason) {
      setError((reason as Error).message);
    }
  };

  const counts = useMemo(
    () => ({
      review: tracks.filter(isReview).length,
      ready: tracks.filter(isReady).length,
      problems: tracks.filter(isProblem).length,
      completed: tracks.filter(isCompleted).length,
    }),
    [tracks],
  );

  const autoApprovable = useMemo(
    () =>
      tracks.filter(
        (track) =>
          isReview(track) &&
          !isProblem(track) &&
          !hasRetryableArtwork(track) &&
          track.candidates.length > 0,
      ).length,
    [tracks],
  );
  const retryableArtwork = useMemo(
    () => tracks.filter(hasRetryableArtwork).length,
    [tracks],
  );

  const autoApprove = async () => {
    setAutoApproving(true);
    setError("");
    setNotice("");
    try {
      const result = await api<AutoApproveResult>("/tracks/auto-approve", {
        method: "POST",
        body: "{}",
      });
      await loadTracks();
      setNotice(
        [
          `${result.approved} review ${result.approved === 1 ? "track" : "tracks"} approved.`,
          result.low_confidence ? `${result.low_confidence} left for review.` : "",
          result.unavailable ? `${result.unavailable} unavailable.` : "",
        ]
          .filter(Boolean)
          .join(" "),
      );
    } catch (reason) {
      setError((reason as Error).message);
    } finally {
      setAutoApproving(false);
    }
  };

  const retryIssues = async () => {
    const damaged = tracks.filter((track) => isProblem(track) && track.status === "corrupt").length;
    if (
      damaged &&
      !confirm(
        `Ununknown will try to salvage ${damaged} damaged ${damaged === 1 ? "file" : "files"} by skipping unreadable frames and re-encoding the valid audio. Each damaged original will be kept beside the repaired file as an .ununknown-damaged backup. Continue?`,
      )
    )
      return;
    setRetryingIssues(true);
    setError("");
    setNotice("");
    try {
      const result = await api<RetryIssuesResult>("/tracks/retry-issues", {
        method: "POST",
        body: "{}",
      });
      await refresh();
      if (result.started) {
        setNotice(
          [
            `Checking ${result.queued} ${result.queued === 1 ? "file" : "files"}, repairing damaged streams, and retrying identification.`,
            result.unavailable ? `${result.unavailable} still missing.` : "",
          ]
            .filter(Boolean)
            .join(" "),
        );
      } else {
        setNotice(
          result.unavailable
            ? `${result.unavailable} source ${result.unavailable === 1 ? "file is" : "files are"} still missing. Restore them to their original locations, then check again.`
            : "No issues need to be checked.",
        );
      }
    } catch (reason) {
      setError((reason as Error).message);
    } finally {
      setRetryingIssues(false);
    }
  };

  const write = async () => {
    if (
      setup.delete_source_after_write &&
      !confirm(
        "Corrected files will be written first. Each original will then be permanently removed only after its output succeeds. Continue?",
      )
    )
      return;
    setError("");
    try {
      await saveSetup();
      const result = await api<{ count: number; outputs: number; duplicates_skipped: number }>(
        "/write",
        { method: "POST", body: "{}" },
      );
      setNotice(
        result.duplicates_skipped
          ? `Writing ${result.outputs} unique ${result.outputs === 1 ? "output" : "outputs"}; ${result.duplicates_skipped} duplicate ${result.duplicates_skipped === 1 ? "recording was" : "recordings were"} skipped.`
          : `Writing ${result.outputs} ${result.outputs === 1 ? "output" : "outputs"}.`,
      );
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
      const inFilter =
        filter === "all" ||
        (filter === "review" && isReview(track)) ||
        (filter === "problems" && isProblem(track)) ||
        (filter === "ready" && isReady(track));
      if (!inFilter) return false;
      if (!normalized) return true;
      const candidate = selectedCandidate(track) ?? track.candidates[0];
      return [
        track.filename,
        track.current_title,
        track.current_artist,
        track.current_album,
        candidate?.title,
        candidate?.artist,
        candidate?.album,
      ].some((value) => value?.toLocaleLowerCase().includes(normalized));
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
    const nextTrack =
      next === "all"
        ? preferredTrack(tracks)
        : tracks.find(
            (track) =>
              (next === "review" && isReview(track)) ||
              (next === "problems" && isProblem(track)) ||
              (next === "ready" && isReady(track)),
          );
    setSelectedId(nextTrack?.id);
  };

  const selectTrack = useCallback((track: Track) => {
    setSelectedId(track.id);
    setMobileInspector(true);
  }, []);

  return (
    <div className="studio-app">
      <header className="topbar">
        <div className="topbar-main">
          <div className="topbar-leading">
            <a className="brand" href="#workspace" aria-label="Ununknown studio home">
              <span className="brand-mark">
                <Icon name="waveform" size={18} />
              </span>
              <span>Ununknown</span>
            </a>
            <button
              className="topbar-source-summary"
              onClick={() => setSettingsOpen(true)}
              aria-label="Change music source"
              title={setup.input_dir || "Choose a music folder"}
            >
              <span className="source-icon">
                <Icon name="folder" size={16} />
              </span>
              <span className="source-copy">
                <b>{folderName(setup.input_dir)}</b>
                <small>
                  {tracks.length} {tracks.length === 1 ? "file" : "files"}
                </small>
              </span>
            </button>
          </div>
          <nav className="workspace-nav" aria-label="Workspace views">
            <NavButton
              className="all-tracks-tab"
              active={filter === "all"}
              onClick={() => changeFilter("all")}
              label="All tracks"
              count={tracks.length}
            />
            <div className="status-tabs" role="group" aria-label="Filter tracks by status">
              <NavButton
                active={filter === "review"}
                onClick={() => changeFilter("review")}
                label="Review"
                count={counts.review}
              />
              <NavButton
                active={filter === "problems"}
                onClick={() => changeFilter("problems")}
                label="Issues"
                count={counts.problems}
              />
              <NavButton
                active={filter === "ready"}
                onClick={() => changeFilter("ready")}
                label="Ready"
                count={counts.ready}
              />
            </div>
          </nav>
          <div className="topbar-controls">
            <button
              className={`topbar-scan-action ${tracks.length ? "" : "prominent"}`.trim()}
              disabled={busy || saving || !setup.input_dir.trim() || !setup.output_dir.trim()}
              onClick={identify}
              title={
                busy
                  ? workflow?.phase === "apply"
                    ? "Writing corrected files"
                    : "Identifying music"
                  : tracks.length
                    ? "Rescan music folder"
                    : "Start cleaning"
              }
            >
              {busy ? (
                <span className="spinner" />
              ) : (
                <Icon name={tracks.length ? "refresh" : "sparkles"} />
              )}
              {busy
                ? workflow?.phase === "apply"
                  ? "Writing…"
                  : "Identifying…"
                : tracks.length
                  ? "Rescan"
                  : "Scan folder"}
            </button>
            <div className="topbar-actions">
              {!connected && (
                <span className="connection-state offline">
                  <i />
                  Offline
                </span>
              )}
              <button
                className="icon-button"
                onClick={() => setTheme(theme === "dark" ? "light" : "dark")}
                aria-label={`Use ${theme === "dark" ? "light" : "dark"} theme`}
                title="Change theme"
              >
                <Icon name={theme === "dark" ? "sun" : "moon"} />
              </button>
              <button
                className="icon-button"
                onClick={() => setSettingsOpen(true)}
                aria-label="Open settings"
                title="Settings"
              >
                <Icon name="settings" />
              </button>
            </div>
          </div>
        </div>
      </header>

      <nav className="mobile-nav" aria-label="Workspace views">
        <NavButton
          className="all-tracks-tab"
          active={filter === "all"}
          onClick={() => changeFilter("all")}
          label="All tracks"
          count={tracks.length}
        />
        <div className="status-tabs" role="group" aria-label="Filter tracks by status">
          <NavButton
            active={filter === "review"}
            onClick={() => changeFilter("review")}
            label="Review"
            count={counts.review}
          />
          <NavButton
            active={filter === "problems"}
            onClick={() => changeFilter("problems")}
            label="Issues"
            count={counts.problems}
          />
          <NavButton
            active={filter === "ready"}
            onClick={() => changeFilter("ready")}
            label="Ready"
            count={counts.ready}
          />
        </div>
      </nav>

      <div className="announcement-region" aria-live="polite" aria-atomic="true">
        {error && (
          <div className="toast error-toast" role="alert">
            <Icon name="alert" />
            <span>
              <b>{connected ? "Couldn’t complete that action" : "Backend unavailable"}</b>
              <small>{friendlyError(error)}</small>
            </span>
            <button onClick={() => setError("")} aria-label="Dismiss error">
              <Icon name="x" size={16} />
            </button>
          </div>
        )}
        {notice && (
          <div className="toast notice-toast">
            <Icon name="check" />
            <span>
              <b>Done</b>
              <small>{notice}</small>
            </span>
            <button onClick={() => setNotice("")} aria-label="Dismiss notification">
              <Icon name="x" size={16} />
            </button>
          </div>
        )}
      </div>

      <main className="workspace" id="workspace">
        <section className="queue-panel" aria-labelledby="queue-title">
          <header className="queue-header">
            <div className="queue-heading">
              <h1 id="queue-title">{filterTitle(filter)}</h1>
              <span className="queue-count">{visibleTracks.length}</span>
            </div>
            <label className="search-field">
              <Icon name="search" size={16} />
              <span className="sr-only">Search queue</span>
              <input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder="Search title, artist, album…"
              />
            </label>
            <button
              className="queue-order-button compact-button"
              onClick={cycleQueueOrder}
              aria-label={`Order tracks by ${queueOrderLabels[queueOrder]}`}
              title={`Current order: ${queueOrderLabels[queueOrder]}. Click to change.`}
            >
              <Icon name="menu" size={14} />
              <span>{queueOrderLabels[queueOrder]}</span>
            </button>
            {autoApprovable > 0 && filter === "review" && (
              <button
                className="compact-button accent"
                disabled={autoApproving}
                onClick={autoApprove}
              >
                <Icon name="sparkles" size={15} />
                {autoApproving ? "Checking…" : `Auto-select ${autoApprovable}`}
              </button>
            )}
            {retryableArtwork > 0 && filter === "review" && (
              <button
                className="compact-button accent issue-retry-button"
                disabled={busy || retryingIssues}
                onClick={() => void retryIssues()}
              >
                {retryingIssues || busy ? (
                  <span className="spinner" />
                ) : (
                  <Icon name="refresh" size={15} />
                )}
                {retryingIssues || busy
                  ? "Retrying…"
                  : `Retry ${retryableArtwork} ${retryableArtwork === 1 ? "cover" : "covers"}`}
              </button>
            )}
            {counts.problems > 0 && filter === "problems" && (
              <button
                className="compact-button accent issue-retry-button"
                disabled={busy || retryingIssues}
                onClick={() => void retryIssues()}
              >
                {retryingIssues || busy ? (
                  <span className="spinner" />
                ) : (
                  <Icon name="refresh" size={15} />
                )}
                {retryingIssues || busy ? "Checking…" : `Check & fix ${counts.problems}`}
              </button>
            )}
          </header>

          <div className="track-list" role="list" aria-label="Music queue">
            {loading ? (
              <QueueSkeleton />
            ) : !connected && tracks.length === 0 ? (
              <EmptyState
                icon="alert"
                title="The studio is offline"
                description="Start the local Rust backend, then reconnect to load your workspace."
                action="Reconnect"
                onAction={loadApp}
              />
            ) : tracks.length === 0 ? (
              <EmptyState
                icon={workflow?.phase === "finish" ? "check" : "music"}
                title={
                  busy
                    ? "Listening to your library"
                    : workflow?.phase === "finish"
                      ? "Cleaning complete"
                      : "Your queue is ready"
                }
                description={
                  busy
                    ? "Audio files will appear here as they are discovered and identified."
                    : workflow?.phase === "finish"
                      ? `Corrected files are available in ${setup.output_dir || "your output folder"}.`
                      : "Enter a music folder above, choose where corrected copies should go, then start cleaning."
                }
              />
            ) : visibleTracks.length === 0 ? (
              <EmptyState
                icon={filter === "review" || filter === "ready" ? "check" : "search"}
                title={
                  filter === "review"
                    ? "Nothing needs review"
                    : filter === "ready"
                      ? "No tracks are ready yet"
                      : "No tracks found"
                }
                description={
                  filter === "review"
                    ? "All current matches are resolved. Your library is ready for the next step."
                    : filter === "ready"
                      ? "Accept a match or enter metadata manually to prepare a track for writing."
                      : "Try another search or switch workspace views."
                }
              />
            ) : (
              visibleTracks.map((track) => (
                <TrackRow
                  key={track.id}
                  track={track}
                  active={track.id === selectedId}
                  onSelect={selectTrack}
                />
              ))
            )}
          </div>
        </section>

        <aside
          className={`inspector-panel ${mobileInspector ? "mobile-open" : ""}`}
          aria-label="Metadata inspector"
        >
          <button
            className="mobile-close icon-button"
            onClick={() => setMobileInspector(false)}
            aria-label="Close inspector"
          >
            <Icon name="x" />
          </button>
          {selected ? (
            <TrackInspector track={selected} onChoose={choose} onSaved={loadTracks} />
          ) : (
            <InspectorEmpty busy={busy} />
          )}
        </aside>
      </main>

      <ProcessingDock
        workflow={workflow}
        counts={counts}
        busy={busy}
        deleteSources={setup.delete_source_after_write}
        onStop={() => void stop()}
        onWrite={() => void write()}
      />

      {settingsOpen && (
        <SettingsDrawer
          setup={setup}
          setSetup={setSetup}
          keys={keys}
          setKeys={setKeys}
          saving={saving}
          onSave={async () => {
            try {
              await saveSetup();
              setNotice("Studio settings saved.");
              setSettingsOpen(false);
            } catch (reason) {
              setError((reason as Error).message);
            }
          }}
          onClose={() => setSettingsOpen(false)}
        />
      )}
    </div>
  );
}
