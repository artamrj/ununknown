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
    <nav className="flowline">
      {steps.map((step, index) => (
        <div
          className={`flow-step ${index < active ? "done" : index === active ? "active" : "wait"}`}
          key={step}
        >
          <span>{index + 1}</span>
          <strong>{step}</strong>
          {index < steps.length - 1 && <i />}
        </div>
      ))}
    </nav>
  );
}
