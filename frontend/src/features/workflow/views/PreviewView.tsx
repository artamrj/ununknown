import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type Candidate, type Track, type TrackPage } from "@/api";
import type { WorkflowViewProps } from "@/features/workflow/types";
import { Button } from "@/shared/components/Button";
import { PreviewList } from "@/features/workflow/components/preview/PreviewList";
import { ActivityLog } from "@/features/workflow/components/ActivityLog";

type PreviewTab = "results" | "review" | "unmatched" | "failed" | "logs";

const tabs: { id: PreviewTab; label: string }[] = [
  { id: "results", label: "Results" },
  { id: "review", label: "Review Required" },
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
  onPreviewStale,
}: WorkflowViewProps) {
  const queryClient = useQueryClient();
  const [tab, setTab] = useState<PreviewTab>("results");
  const items = preview?.items || [];
  const writeCount = preview?.summary?.write_count ?? items.length;
  const empty = workflow.phase !== "finish" && !items.length;
  const unmatched = useQuery({
    queryKey: ["tracks", "view", "unmatched"],
    queryFn: () => api<TrackPage>("/tracks?view=unmatched&page_size=200"),
    enabled: tab === "unmatched",
  });
  const review = useQuery({
    queryKey: ["tracks", "view", "review"],
    queryFn: () => api<TrackPage>("/tracks?view=review&page_size=200"),
    enabled: tab === "review",
  });
  const failed = useQuery({
    queryKey: ["tracks", "view", "failed"],
    queryFn: () => api<TrackPage>("/tracks?view=failed&page_size=200"),
    enabled: tab === "failed",
  });
  const reviewAction = useMutation({
    mutationFn: ({
      trackId,
      candidateId,
      action,
    }: {
      trackId: number;
      candidateId?: number;
      action: "select" | "skip" | "keep";
    }) => {
      if (action === "select") {
        return api(`/tracks/${trackId}/select-candidate`, {
          method: "POST",
          body: JSON.stringify({ candidate_id: candidateId }),
        });
      }
      return api(`/tracks/${trackId}/${action === "skip" ? "skip" : "keep-current"}`, {
        method: "POST",
        body: "{}",
      });
    },
    onSuccess: () => {
      onPreviewStale();
      queryClient.invalidateQueries({ queryKey: ["tracks"] });
      queryClient.invalidateQueries({ queryKey: ["workspace"] });
    },
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
          error={unmatched.error}
          loading={unmatched.isLoading}
          total={unmatched.data?.total}
          tracks={unmatched.data?.items || []}
        />
      )}
      {tab === "review" && (
        <ReviewRequiredList
          error={review.error}
          loading={review.isLoading}
          pending={reviewAction.isPending}
          total={review.data?.total}
          tracks={review.data?.items || []}
          onKeep={(trackId) => reviewAction.mutate({ trackId, action: "keep" })}
          onSelect={(trackId, candidateId) =>
            reviewAction.mutate({ trackId, candidateId, action: "select" })
          }
          onSkip={(trackId) => reviewAction.mutate({ trackId, action: "skip" })}
        />
      )}
      {tab === "failed" && (
        <TrackReviewList
          emptyText="No failed tracks"
          error={failed.error}
          loading={failed.isLoading}
          showError
          total={failed.data?.total}
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

function ReviewRequiredList({
  tracks,
  loading,
  error,
  total,
  pending,
  onSelect,
  onSkip,
  onKeep,
}: {
  tracks: Track[];
  loading: boolean;
  error?: Error | null;
  total?: number;
  pending: boolean;
  onSelect: (trackId: number, candidateId: number) => void;
  onSkip: (trackId: number) => void;
  onKeep: (trackId: number) => void;
}) {
  if (loading) return <div className="review-list-state">Loading review tracks...</div>;
  if (error) return <div className="review-list-state error">{error.message}</div>;
  if (!tracks.length) return <div className="review-list-state">No tracks need review</div>;

  return (
    <>
      {typeof total === "number" && (
        <div className="review-list-count">
          Showing {tracks.length} of {total}
        </div>
      )}
      <div className="review-track-list">
        {tracks.map((track) => (
          <article className="review-track-row review-required-row" key={track.id}>
            <div>
              <strong>{track.filename}</strong>
              <span>{track.stage_message || "Choose the correct release before preview."}</span>
            </div>
            <div className="candidate-options">
              {track.candidates.map((candidate) => (
                <CandidateOption
                  candidate={candidate}
                  disabled={pending}
                  key={candidate.id}
                  onSelect={() => onSelect(track.id, candidate.id)}
                />
              ))}
            </div>
            <div className="review-actions">
              <button disabled={pending} onClick={() => onKeep(track.id)} type="button">
                Keep current tags
              </button>
              <button disabled={pending} onClick={() => onSkip(track.id)} type="button">
                Skip
              </button>
            </div>
          </article>
        ))}
      </div>
    </>
  );
}

function CandidateOption({
  candidate,
  disabled,
  onSelect,
}: {
  candidate: Candidate;
  disabled: boolean;
  onSelect: () => void;
}) {
  const why = parseWhy(candidate.score_breakdown);
  const sourceBadges = Array.isArray(why?.sources)
    ? why.sources.map((source) => String(source))
    : [providerLabel(candidate.provider)];
  const badges = [
    ...sourceBadges,
    why?.acoustid ? "AcoustID" : undefined,
    candidate.is_compilation ? "Compilation" : undefined,
  ].filter(Boolean);
  const releaseType = [
    candidate.release_type,
    candidate.release_secondary_types,
    candidate.is_compilation ? "Compilation" : undefined,
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <div className="candidate-option">
      {candidate.cover_url ? <img alt="" src={candidate.cover_url} /> : <span className="cover-fallback" />}
      <div>
        <strong>{candidate.album || candidate.title || "Unknown release"}</strong>
        <span className="source-badges">
          {badges.map((badge) => (
            <i key={badge}>{badge}</i>
          ))}
        </span>
        <span>{candidate.artist || "Unknown artist"}</span>
        <small>
          {[candidate.release_date || candidate.year, candidate.release_country, releaseType]
            .filter(Boolean)
            .join(" · ")}
        </small>
        <small>
          {Math.round(candidate.score)}%
          {typeof candidate.duration_delta === "number"
            ? ` · ${candidate.duration_delta.toFixed(1)}s duration delta`
            : ""}
        </small>
      </div>
      <button disabled={disabled} onClick={onSelect} type="button">
        Select
      </button>
      <details className="why-match">
        <summary>Why this match?</summary>
        <dl>
          <div>
            <dt>Fingerprint match</dt>
            <dd>
              {typeof why?.acoustid === "number"
                ? `${Math.round(why.acoustid * 100)}%`
                : "Not used"}
            </dd>
          </div>
          <div>
            <dt>Duration match</dt>
            <dd>{quality(why?.duration)}</dd>
          </div>
          <div>
            <dt>Title similarity</dt>
            <dd>{quality(why?.title)}</dd>
          </div>
          <div>
            <dt>Album context</dt>
            <dd>{quality(why?.album_context)}</dd>
          </div>
          <div>
            <dt>Source agreement</dt>
            <dd>{sourceBadges.length ? sourceBadges.join(" + ") : "None"}</dd>
          </div>
        </dl>
      </details>
    </div>
  );
}

function parseWhy(value?: string) {
  if (!value) return undefined;
  try {
    return JSON.parse(value) as Record<string, unknown>;
  } catch {
    return undefined;
  }
}

function quality(value?: unknown) {
  if (typeof value !== "number") return "Unknown";
  if (value >= 0.85) return "Good";
  if (value >= 0.55) return "Medium";
  return "Weak";
}

function providerLabel(value?: string) {
  if (!value) return "MusicBrainz";
  const known: Record<string, string> = {
    musicbrainz: "MusicBrainz",
    discogs: "Discogs",
    acoustid: "AcoustID",
    lastfm: "Last.fm",
    theaudiodb: "TheAudioDB",
    wikidata: "Wikidata",
  };
  return known[value] || value;
}

function TrackReviewList({
  tracks,
  loading,
  error,
  emptyText,
  total,
  showError = false,
}: {
  tracks: Track[];
  loading: boolean;
  error?: Error | null;
  emptyText: string;
  total?: number;
  showError?: boolean;
}) {
  if (loading) return <div className="review-list-state">Loading tracks...</div>;
  if (error) return <div className="review-list-state error">{error.message}</div>;
  if (!tracks.length) return <div className="review-list-state">{emptyText}</div>;

  return (
    <>
      {typeof total === "number" && (
        <div className="review-list-count">
          Showing {tracks.length} of {total}
        </div>
      )}
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
    </>
  );
}
