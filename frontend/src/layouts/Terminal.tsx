import { useEffect, useRef } from "react";
import type { TerminalLine } from "../api";
import type { EventStatus } from "../hooks";

export function Terminal({
  lines,
  status = "connected",
}: {
  lines: TerminalLine[];
  status?: EventStatus;
}) {
  const body = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (body.current) body.current.scrollTop = body.current.scrollHeight;
  }, [lines.length]);
  const recent = lines.slice(-500);
  return (
    <aside className={`terminal-card ${status}`}>
      <header>
        <span /> Developer log{" "}
        <small>
          {status} · {recent.length} lines
        </small>
      </header>
      <div ref={body}>
        {recent.length ? (
          recent.map((line, index) => (
            <p className={line.level} key={`${line.timestamp}-${index}`}>
              <time>{new Date(line.timestamp).toISOString()}</time>
              <b>{line.level.toUpperCase()}</b>
              <code>{line.stage}</code>
              <span>
                <strong>{line.message}</strong>
                {line.file ? <em>{line.file}</em> : null}
                <small>
                  {line.attempt ? `attempt=${line.attempt} ` : ""}
                  {typeof line.duration_ms === "number" ? `duration_ms=${line.duration_ms}` : ""}
                </small>
                {line.detail ? <i>{line.detail}</i> : null}
                {line.error ? <pre>{line.error}</pre> : null}
                {line.context ? <pre>{JSON.stringify(line.context, null, 2)}</pre> : null}
              </span>
            </p>
          ))
        ) : (
          <p className="muted">
            <time>--:--:--</time>
            <b>INFO</b>
            <code>idle</code>
            <span>
              <strong>
                {status === "reconnecting"
                  ? "Reconnecting to live events..."
                  : "Waiting for scan output..."}
              </strong>
            </span>
          </p>
        )}
      </div>
    </aside>
  );
}
