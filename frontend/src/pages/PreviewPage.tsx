import type { Preview, Workflow } from "../api";
import type { EventStatus } from "../hooks";
import { Button } from "../components";
import { PreviewList } from "../features/preview";
import { Terminal } from "../layouts/Terminal";

export function PreviewPage({
  workflow,
  preview,
  applyPending,
  eventStatus,
  onScan,
  onApply,
}: {
  workflow: Workflow;
  preview?: Preview;
  applyPending: boolean;
  eventStatus: EventStatus;
  onScan: () => void;
  onApply: () => void;
}) {
  const items = preview?.items || [];
  const writeCount = preview?.summary?.write_count ?? items.length;
  const empty = workflow.phase !== "finish" && !items.length;
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
      {empty && (
        <div className="empty-preview">
          <div>
            <h2>No writable matches yet</h2>
            <p>
              The fetch terminal explains provider errors, missing AcoustID configuration, low
              confidence scores, and unmatched decisions.
            </p>
          </div>
          <Terminal lines={workflow.terminal_log || []} status={eventStatus} />
        </div>
      )}
      {workflow.phase !== "finish" && !empty && <PreviewList items={items} />}
      {workflow.phase !== "finish" && preview && writeCount > 0 && (
        <div className="apply-bar">
          <Button disabled={applyPending} onClick={onApply}>
            {applyPending ? "Applying..." : "Apply changes"}
          </Button>
        </div>
      )}
    </section>
  );
}
