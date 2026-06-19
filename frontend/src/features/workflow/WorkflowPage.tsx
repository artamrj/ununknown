import { isProcessingPhase } from "@/features/workflow/workflowPhase";
import type { WorkflowPageProps, WorkflowViewProps } from "@/features/workflow/types";
import { FailedView } from "@/features/workflow/views/FailedView";
import { IdleView } from "@/features/workflow/views/IdleView";
import { PreviewView } from "@/features/workflow/views/PreviewView";
import { ProcessingView } from "@/features/workflow/views/ProcessingView";
import { Spinner } from "@/shared/components/Spinner";

export function WorkflowPage({
  workflow,
  loading,
  preview,
  eventStatus,
  applyPending,
  onScan,
  onStop,
  onApply,
  onPreviewStale,
}: WorkflowPageProps) {
  if (!workflow || loading) {
    return (
      <section className="state-card">
        <Spinner /> Loading workspace
      </section>
    );
  }

  const viewProps: WorkflowViewProps = {
    workflow,
    preview,
    eventStatus,
    applyPending,
    onScan,
    onStop,
    onApply,
    onPreviewStale,
  };

  if (workflow.phase === "idle") return <IdleView {...viewProps} />;
  if (isProcessingPhase(workflow.phase)) return <ProcessingView {...viewProps} />;
  if (workflow.phase === "failed") return <FailedView {...viewProps} />;
  return <PreviewView {...viewProps} />;
}
