import type { ReactNode } from "react";
import { Button } from "../../components";

export const Section = ({
  title,
  note,
  children,
}: {
  title: string;
  note: string;
  children: ReactNode;
}) => (
  <section className="settings-card">
    <div className="settings-card-head">
      <h2>{title}</h2>
      {note && <p>{note}</p>}
    </div>
    {children}
  </section>
);

export const Reset = ({ onClick }: { onClick: () => void }) => (
  <Button kind="quiet" onClick={() => confirm("Reset this section?") && onClick()}>
    Reset section
  </Button>
);

const Row = ({
  show,
  l,
  d,
  children,
}: {
  show: (s: string) => boolean;
  l: string;
  d: string;
  children: ReactNode;
}) =>
  show(`${l} ${d}`) ? (
    <label className="setting-row">
      <span>
        <strong>{l}</strong>
        <small>{d}</small>
      </span>
      {children}
    </label>
  ) : null;

export const Field = ({
  show,
  l,
  d,
  v,
  set,
}: {
  show: (s: string) => boolean;
  l: string;
  d: string;
  v: string;
  set: (x: string) => void;
}) => (
  <Row show={show} l={l} d={d}>
    <input value={v || ""} onChange={(event) => set(event.target.value)} />
  </Row>
);

export const NumberField = ({
  show,
  l,
  d,
  v,
  set,
}: {
  show: (s: string) => boolean;
  l: string;
  d: string;
  v: number;
  set: (x: number) => void;
}) => (
  <Row show={show} l={l} d={d}>
    <input type="number" value={v} onChange={(event) => set(+event.target.value)} />
  </Row>
);

export const Select = ({
  show,
  l,
  d,
  v,
  set,
  o,
}: {
  show: (s: string) => boolean;
  l: string;
  d: string;
  v: string;
  set: (x: string) => void;
  o: string[];
}) => (
  <Row show={show} l={l} d={d}>
    <select value={v} onChange={(event) => set(event.target.value)}>
      {o.map((x) => (
        <option key={x}>{x}</option>
      ))}
    </select>
  </Row>
);

export const Toggle = ({
  show,
  l,
  d,
  v,
  set,
}: {
  show: (s: string) => boolean;
  l: string;
  d: string;
  v: boolean;
  set: (x: boolean) => void;
}) => (
  <Row show={show} l={l} d={d}>
    <input type="checkbox" checked={v} onChange={(event) => set(event.target.checked)} />
  </Row>
);

export const Choice = ({
  value,
  set,
  items,
}: {
  value: string;
  set: (x: string) => void;
  items: string[][];
}) => (
  <div className="choice-grid">
    {items.map(([id, title, desc]) => (
      <button
        type="button"
        className={value === id ? "selected" : ""}
        onClick={() => set(id)}
        key={id}
      >
        <strong>{title}</strong>
        <span>{desc}</span>
      </button>
    ))}
  </div>
);

export const ToggleCard = ({
  show,
  label,
  desc,
  checked,
  onChange,
}: {
  show: (s: string) => boolean;
  label: string;
  desc: string;
  checked: boolean;
  onChange: (x: boolean) => void;
}) =>
  show(`${label} ${desc}`) ? (
    <label className={`toggle-card ${checked ? "on" : ""}`}>
      <input
        type="checkbox"
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span>
        <strong>{label}</strong>
        <small>{desc}</small>
      </span>
    </label>
  ) : null;

export const TemplateField = ({
  label,
  desc,
  value,
  onChange,
}: {
  label: string;
  desc: string;
  value: string;
  onChange: (x: string) => void;
}) => (
  <label className="template-field">
    <span>
      <strong>{label}</strong>
      <small>{desc}</small>
    </span>
    <input
      value={value || ""}
      onChange={(event) => onChange(event.target.value)}
      spellCheck={false}
    />
  </label>
);
