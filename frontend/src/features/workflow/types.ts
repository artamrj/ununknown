import type { Preview, Workflow } from "@/api";
import type { EventStatus } from "@/shared/hooks/useEvents";

export type WorkflowViewProps = {
  workflow: Workflow;
  preview?: Preview;
  applyPending: boolean;
  eventStatus: EventStatus;
  onScan: () => void;
  onStop: () => void;
  onApply: () => void;
  onPreviewStale: () => void;
};

export type WorkflowPageProps = {
  workflow?: Workflow;
  loading: boolean;
  preview?: Preview;
  eventStatus: EventStatus;
  applyPending: boolean;
  onScan: () => void;
  onStop: () => void;
  onApply: () => void;
  onPreviewStale: () => void;
};
