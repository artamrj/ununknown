import { Spinner } from "@/shared/components/Spinner";

function Progress({ current, total }: { current: number; total: number }) {
  const value = total ? Math.min(100, Math.round((current / total) * 100)) : 0;
  return (
    <div className="live-progress" aria-label={`Progress ${value}%`}>
      <span style={{ width: `${value}%` }} />
    </div>
  );
}

export function ProcessingStatusCard({
  phase,
  message,
  currentFile,
  current,
  total,
  matched,
  unmatched,
  failed,
  onStop,
}: {
  phase: string;
  message: string;
  currentFile?: string;
  current: number;
  total: number;
  matched: number;
  unmatched: number;
  failed: number;
  onStop: () => void;
}) {
  return (
    <div className="process state-card">
      <div className="process-icon">
        <Spinner />
      </div>
      <span className="eyebrow">{phase}</span>
      <h1>{message}</h1>
      <p className="current-file">{currentFile || "Reading mounted music folder..."}</p>
      <div className="counter">
        <strong>{current}</strong>
        <span>of {total || "-"}</span>
      </div>
      <Progress current={current} total={total} />
      <div className="summary">
        <span>{matched} matched</span>
        <span>{unmatched} unmatched</span>
        <span>{failed} failed</span>
      </div>
      <button className="button danger" onClick={onStop}>
        Stop
      </button>
    </div>
  );
}
