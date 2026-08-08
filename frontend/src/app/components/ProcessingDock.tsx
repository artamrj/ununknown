import type { Workflow } from "@/api/types";
import { Icon } from "../Icons";

export function ProcessingDock({
  workflow,
  counts,
  busy,
  deleteSources,
  onStop,
  onWrite,
}: {
  workflow?: Workflow;
  counts: { review: number; ready: number; problems: number; completed: number };
  busy: boolean;
  deleteSources: boolean;
  onStop: () => void;
  onWrite: () => void;
}) {
  const completedOperations =
    workflow?.phase === "apply" || workflow?.phase === "finish"
      ? workflow.current
      : Math.max(workflow?.processed || 0, workflow?.current || 0);
  const percent = workflow?.total
    ? Math.min(100, Math.round((completedOperations / workflow.total) * 100))
    : 0;
  return (
    <footer className={`processing-dock ${busy ? "working" : ""}`}>
      <div className="dock-operation">
        <span className="operation-icon">
          {busy ? (
            <span className="equalizer">
              <i />
              <i />
              <i />
            </span>
          ) : (
            <Icon name={workflow?.phase === "failed" ? "alert" : "disc"} />
          )}
        </span>
        <span>
          <b>
            {busy
              ? workflow?.message || "Processing"
              : workflow?.phase === "failed"
                ? "Processing stopped"
                : workflow?.phase === "finish"
                  ? "Writing complete"
                  : "Studio ready"}
          </b>
          <small>
            {busy
              ? workflow?.current_file || "Preparing your music…"
              : workflow?.phase === "finish"
                ? `${workflow.current || workflow.total} corrected files written`
                : counts.ready
                  ? `${counts.ready} ${counts.ready === 1 ? "track" : "tracks"} ready to write`
                  : counts.review
                    ? `${counts.review} waiting for review`
                    : "Select a folder or inspect the queue"}
          </small>
        </span>
      </div>
      <div className="dock-progress">
        <progress max="100" value={percent} aria-label={`${percent}% complete`} />
        <small>
          {busy
            ? `${completedOperations} of ${workflow?.total || 0} · ${percent}%`
            : `${counts.ready} ready · ${counts.review} review · ${counts.problems} problems`}
        </small>
      </div>
      <div className="dock-actions">
        {busy ? (
          <button className="compact-button" onClick={onStop}>
            <Icon name="pause" size={15} />
            Stop
          </button>
        ) : (
          <button className="primary-action" disabled={!counts.ready} onClick={onWrite}>
            <Icon name="sparkles" />
            Write {counts.ready} {counts.ready === 1 ? "file" : "files"}
          </button>
        )}
        <span className={deleteSources ? "delete-note" : "safe-note"}>
          <Icon name={deleteSources ? "trash" : "shield"} size={14} />
          {deleteSources ? "Originals removed after success" : "Originals stay untouched"}
        </span>
      </div>
    </footer>
  );
}

