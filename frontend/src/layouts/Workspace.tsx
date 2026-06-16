import type { Preview, Workflow } from "../api";
import type { EventStatus } from "../hooks";
import { Button, Spinner } from "../components";
import { PreviewPage } from "../pages/PreviewPage";
import { ProcessingCard } from "./ProcessingCard";
import { Terminal } from "./Terminal";

export function Workspace({
  workflow,
  loading,
  preview,
  eventStatus,
  applyPending,
  onScan,
  onStop,
  onApply,
}: {
  workflow?: Workflow;
  loading: boolean;
  preview?: Preview;
  eventStatus: EventStatus;
  applyPending: boolean;
  onScan: () => void;
  onStop: () => void;
  onApply: () => void;
}) {
  if (!workflow || loading) {
    return (
      <section className="state-card">
        <Spinner /> Loading workspace
      </section>
    );
  }
  if (workflow.phase === "idle") {
    return (
      <section className="hero state-card">
        <span className="eyebrow">Local metadata repair</span>
        <h1>Start a clean metadata run.</h1>
        <p>
          Ununknown scans first, then fetches one track at a time with visible provider diagnostics.
        </p>
        <Button onClick={onScan}>Scan music</Button>
      </section>
    );
  }
  if (["scan", "fetch", "apply"].includes(workflow.phase)) {
    return (
      <section className="run-grid">
        <ProcessingCard
          phase={workflow.phase}
          message={workflow.message}
          currentFile={workflow.current_file}
          current={workflow.current}
          total={workflow.total}
          matched={workflow.matched}
          unmatched={workflow.unmatched}
          failed={workflow.failed}
          onStop={onStop}
        />
        <Terminal lines={workflow.terminal_log || []} status={eventStatus} />
      </section>
    );
  }
  if (workflow.phase === "failed") {
    return (
      <section className="run-grid">
        <div className="state-card hero">
          <span className="eyebrow error">Workflow error</span>
          <h1>Processing stopped</h1>
          <p>{workflow.message}</p>
          <Button onClick={onScan}>Start new scan</Button>
        </div>
        <Terminal lines={workflow.terminal_log || []} status={eventStatus} />
      </section>
    );
  }
  return (
    <PreviewPage
      workflow={workflow}
      preview={preview}
      applyPending={applyPending}
      eventStatus={eventStatus}
      onScan={onScan}
      onApply={onApply}
    />
  );
}
