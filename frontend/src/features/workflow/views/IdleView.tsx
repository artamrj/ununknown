import type { WorkflowViewProps } from "@/features/workflow/types";
import { Button } from "@/shared/components/Button";

export function IdleView({ onScan }: WorkflowViewProps) {
  return (
    <section className="hero state-card">
      <span className="eyebrow">Local metadata repair</span>
      <h1>Start a clean metadata run.</h1>
      <p>Ununknown scans first, then fetches one track at a time with visible provider diagnostics.</p>
      <Button onClick={onScan}>Scan music</Button>
    </section>
  );
}
