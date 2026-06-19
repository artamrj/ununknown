import type { WorkflowViewProps } from "@/features/workflow/types";
import { ProcessingStatusCard } from "@/features/workflow/components/ProcessingStatusCard";
import { Terminal } from "@/features/workflow/components/Terminal";

export function ProcessingView({ workflow, eventStatus, onStop }: WorkflowViewProps) {
  return (
    <section className="run-grid">
      <ProcessingStatusCard
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
