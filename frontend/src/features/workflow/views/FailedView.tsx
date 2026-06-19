import type { WorkflowViewProps } from "@/features/workflow/types";
import { Terminal } from "@/features/workflow/components/Terminal";
import { Button } from "@/shared/components/Button";

export function FailedView({ workflow, eventStatus, onScan }: WorkflowViewProps) {
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
