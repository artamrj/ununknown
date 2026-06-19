import { useEffect, useRef, useState } from "react";
import { Button } from "@/shared/components/Button";

type JsonSettingsProps = {
  settings: any;
  onChange: (value: any) => void;
  onValidityChange: (valid: boolean) => void;
};

export function JsonSettings({ settings, onChange, onValidityChange }: JsonSettingsProps) {
  const editor = useRef<HTMLTextAreaElement>(null);
  const [text, setText] = useState(() => pretty(settings));
  const [error, setError] = useState("");

  useEffect(() => {
    onValidityChange(!error);
  }, [error, onValidityChange]);

  useEffect(() => {
    if (document.activeElement !== editor.current) {
      setText(pretty(settings));
      setError("");
    }
  }, [settings]);

  const update = (value: string) => {
    setText(value);
    try {
      const parsed = JSON.parse(value);
      setError("");
      onChange(parsed);
    } catch (err) {
      setError(jsonError(value, err));
    }
  };

  const format = () => {
    try {
      const formatted = pretty(JSON.parse(text));
      setText(formatted);
      setError("");
    } catch (err) {
      setError(jsonError(text, err));
    }
  };

  const reset = () => {
    setText(pretty(settings));
    setError("");
  };

  return (
    <section className="settings-card json-settings-card">
      <div className="settings-card-head json-settings-head">
        <div>
          <h2>JSON</h2>
          <p>Edit the same draft settings as the visual tabs. Save still uses backend validation.</p>
        </div>
        <div className="json-actions">
          <Button kind="quiet" onClick={format}>
            Format JSON
          </Button>
          <Button kind="quiet" onClick={reset}>
            Reset JSON
          </Button>
        </div>
      </div>
      {error && <div className="json-error">{error}</div>}
      <textarea
        ref={editor}
        className="json-editor"
        spellCheck={false}
        value={text}
        onChange={(event) => update(event.target.value)}
      />
    </section>
  );
}

function pretty(value: unknown) {
  return JSON.stringify(value ?? {}, null, 2);
}

function jsonError(text: string, error: unknown) {
  const message = error instanceof Error ? error.message : "Invalid JSON";
  const match = message.match(/position (\d+)/i);
  if (!match) return message;
  const position = Number(match[1]);
  const before = text.slice(0, position);
  const line = before.split("\n").length;
  const column = before.length - before.lastIndexOf("\n");
  return `${message} at line ${line}, column ${column}`;
}
