const steps = ["Scan", "Fetch", "Preview", "Apply", "Finish"];
const phaseIndex = (phase: string) =>
  phase === "idle" || phase === "scan"
    ? 0
    : phase === "fetch"
      ? 1
      : phase === "preview"
        ? 2
        : phase === "apply"
          ? 3
          : 4;

export function Flow({ phase }: { phase: string }) {
  const active = phaseIndex(phase);
  return (
    <nav className="flowline" aria-label="Workflow progress">
      {steps.map((step, index) => (
        <span
          className={`flow-step ${index < active ? "done" : index === active ? "active" : "wait"}`}
          key={step}
          aria-current={index === active ? "step" : undefined}
        >
          <i aria-hidden="true" />
          <b aria-hidden="true">{index + 1}</b>
          <strong>{step}</strong>
        </span>
      ))}
    </nav>
  );
}
