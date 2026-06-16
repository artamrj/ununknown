import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import type { TerminalLine, Workflow } from "../api";

type ServerEvent = {
  kind: string;
  stage?: Workflow["phase"] | string;
  level?: string;
  file?: string;
  timestamp?: string;
  phase?: Workflow["phase"] | string;
  current_file?: string;
  processed?: number;
  matched?: number;
  unmatched?: number;
  failed?: number;
  current: number;
  total: number;
  message: string;
};

export type EventStatus = "connecting" | "connected" | "reconnecting";

const phases = new Set(["idle", "scan", "fetch", "preview", "apply", "finish", "failed"]);
const emptyWorkflow = (): Workflow => ({
  phase: "idle",
  message: "Ready to scan",
  current: 0,
  total: 0,
  processed: 0,
  matched: 0,
  unmatched: 0,
  failed: 0,
  terminal_log: [],
});

export function useEvents(): EventStatus {
  const queryClient = useQueryClient();
  const [status, setStatus] = useState<EventStatus>("connecting");
  useEffect(() => {
    const source = new EventSource("/api/events");
    source.onmessage = (message) => {
      setStatus("connected");
      const event = JSON.parse(message.data) as ServerEvent;
      queryClient.setQueryData<Workflow>(["workspace"], (current) => {
        current = current || emptyWorkflow();
        if (event.kind === "terminal") {
          const line: TerminalLine = {
            timestamp: event.timestamp || new Date().toISOString(),
            level: event.level || "info",
            stage: event.stage || "fetch",
            file: event.file,
            message: event.message,
          };
          return {
            ...current,
            phase:
              event.phase && phases.has(event.phase)
                ? (event.phase as Workflow["phase"])
                : current.phase,
            current_file: event.current_file || event.file || current.current_file,
            current: event.current || current.current,
            total: event.total || current.total,
            processed: event.processed ?? current.processed,
            matched: event.matched ?? current.matched,
            unmatched: event.unmatched ?? current.unmatched,
            failed: event.failed ?? current.failed,
            terminal_log: [...(current.terminal_log || []), line].slice(-160),
          };
        }
        const phase =
          event.stage && phases.has(event.stage)
            ? (event.stage as Workflow["phase"])
            : current.phase;
        return {
          ...current,
          phase,
          message: event.message || current.message,
          current: event.current || current.current,
          total: event.total || current.total,
          current_file:
            event.current_file ||
            event.file ||
            (phase === "fetch" && event.message ? event.message : current.current_file),
          processed: event.processed ?? current.processed,
          matched: event.matched ?? current.matched,
          unmatched: event.unmatched ?? current.unmatched,
          failed: event.failed ?? current.failed,
        };
      });
      if (
        ["preview", "finish", "failed", "done"].includes(event.kind) ||
        event.stage === "preview" ||
        event.stage === "finish"
      ) {
        queryClient.invalidateQueries({ queryKey: ["workspace"] });
        queryClient.invalidateQueries({ queryKey: ["tracks"] });
      }
    };
    source.onopen = () => setStatus("connected");
    source.onerror = () => {
      setStatus("reconnecting");
      queryClient.invalidateQueries({ queryKey: ["workspace"] });
    };
    return () => {
      setStatus("connecting");
      source.close();
    };
  }, [queryClient]);
  return status;
}
