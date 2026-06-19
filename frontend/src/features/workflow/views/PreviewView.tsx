import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { api, type Track, type TrackPage } from "@/api";
import type { WorkflowViewProps } from "@/features/workflow/types";
import { Button } from "@/shared/components/Button";
import { PreviewList } from "@/features/workflow/components/preview/PreviewList";
import { ActivityLog } from "@/features/workflow/components/ActivityLog";

type PreviewTab = "results" | "unmatched" | "failed" | "logs";

const tabs: { id: PreviewTab; label: string }[] = [
  { id: "results", label: "Results" },
  { id: "unmatched", label: "Unmatched" },
  { id: "failed", label: "Failed" },
  { id: "logs", label: "Logs" },
];

export function PreviewView({
  workflow,
  preview,
  applyPending,
  eventStatus,
  onScan,
  onApply,
}: WorkflowViewProps) {
  const [tab, setTab] = useState<PreviewTab>("results");
  const items = preview?.items || [];
  const writeCount = preview?.summary?.write_count ?? items.length;
  const empty = workflow.phase !== "finish" && !items.length;
  const unmatched = useQuery({
    queryKey: ["tracks", "view", "unmatched"],
    queryFn: () => api<TrackPage>("/tracks?view=unmatched&page_size=200"),
    enabled: tab === "unmatched",
  });
  const failed = useQuery({
    queryKey: ["tracks", "view", "failed"],
    queryFn: () => api<TrackPage>("/tracks?view=failed&page_size=200"),
    enabled: tab === "failed",
  });

  return (
    <section className="preview-workspace preview-v4">
      <header>
        <div>
          <span className="eyebrow">{workflow.phase === "finish" ? "Finished" : "Preview"}</span>
          <h1>
            {workflow.phase === "finish"
              ? "Metadata apply complete"
              : `${workflow.matched} matched tracks ready`}
          </h1>
          <p>
            {workflow.unmatched} unmatched · {workflow.failed} failed
            {preview?.summary?.duplicate_skipped
              ? ` · ${preview.summary.duplicate_skipped} duplicate skipped`
              : ""}
          </p>
        </div>
        <Button
          kind="quiet"
          onClick={() =>
            confirm(
              "Clear this preview and rescan files? Fingerprints for unchanged files will be reused.",
            ) && onScan()
          }
        >
          Rescan
        </Button>
      </header>

      <nav className="preview-tabs" aria-label="Preview sections">
        {tabs.map((item) => (
          <button
            className={tab === item.id ? "active" : ""}
            key={item.id}
            onClick={() => setTab(item.id)}
            type="button"
          >
            {item.label}
          </button>
        ))}
      </nav>

      {tab === "results" && (
        <>
          {empty && (
            <div className="empty-preview">
              <div>
                <h2>No writable matches yet</h2>
                <p>
                  The activity log explains provider errors, missing AcoustID configuration, low
                  confidence scores, and unmatched decisions.
                </p>
              </div>
            </div>
          )}
          {workflow.phase !== "finish" && !empty && <PreviewList items={items} />}
        </>
      )}
      {tab === "unmatched" && (
        <TrackReviewList
          emptyText="No unmatched tracks"
          loading={unmatched.isLoading}
          tracks={unmatched.data?.items || []}
        />
      )}
      {tab === "failed" && (
        <TrackReviewList
          emptyText="No failed tracks"
          loading={failed.isLoading}
          showError
          tracks={failed.data?.items || []}
        />
      )}
      {tab === "logs" && <ActivityLog lines={workflow.activity_log || []} status={eventStatus} />}

      {tab === "results" && workflow.phase !== "finish" && preview && writeCount > 0 && (
        <div className="apply-bar">
          <Button disabled={applyPending} onClick={onApply}>
            {applyPending ? "Applying..." : "Apply changes"}
          </Button>
        </div>
      )}
    </section>
  );
}

function TrackReviewList({
  tracks,
  loading,
  emptyText,
  showError = false,
}: {
  tracks: Track[];
  loading: boolean;
  emptyText: string;
  showError?: boolean;
}) {
  if (loading) return <div className="review-list-state">Loading tracks...</div>;
  if (!tracks.length) return <div className="review-list-state">{emptyText}</div>;

  return (
    <div className="review-track-list">
      {tracks.map((track) => {
        const best = [...track.candidates].sort((a, b) => b.score - a.score)[0];
        return (
          <article className="review-track-row" key={track.id}>
            <div>
              <strong>{track.filename}</strong>
              <span>{track.path}</span>
            </div>
            <div>
              <b>{track.current_title || "Untitled"}</b>
              <span>{track.current_artist || "Unknown artist"}</span>
            </div>
            <div>
              <code>{track.stage}</code>
              <span>{track.stage_message || track.status}</span>
            </div>
            <div>
              {best ? (
                <>
                  <b>
                    {track.candidates.length} candidate{track.candidates.length === 1 ? "" : "s"}
                  </b>
                  <span>
                    Best {Math.round(best.score)}% · {best.artist || "Unknown artist"} -{" "}
                    {best.title || "Untitled"}
                  </span>
                </>
              ) : (
                <>
                  <b>No candidates</b>
                  <span>Nothing was selected for this file.</span>
                </>
              )}
            </div>
            {showError && track.error ? <pre>{track.error}</pre> : null}
          </article>
        );
      })}
    </div>
  );
}
