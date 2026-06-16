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
  const recent = lines.slice(-120);
  return (
    <aside className={`terminal-card ${status}`}>
      <header>
        <span /> Fetch terminal{" "}
        <small>
          {status} · {recent.length} lines
        </small>
      </header>
      <div ref={body}>
        {recent.length ? (
          recent.map((line, index) => (
            <p className={line.level} key={`${line.timestamp}-${index}`}>
              <time>{new Date(line.timestamp).toLocaleTimeString()}</time>
              <b>{line.stage}</b>
              <span>
                {line.file ? <strong>{line.file}</strong> : null}
                {line.file ? " · " : ""}
                {line.message}
              </span>
            </p>
          ))
        ) : (
          <p className="muted">
            <time>--:--:--</time>
            <b>idle</b>
            <span>
              {status === "reconnecting"
                ? "Reconnecting to live events..."
                : "Waiting for scan output..."}
            </span>
          </p>
        )}
      </div>
    </aside>
  );
}
