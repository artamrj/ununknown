export function NavButton({
  active,
  label,
  count,
  className = "",
  onClick,
}: {
  active: boolean;
  label: string;
  count?: number;
  className?: string;
  onClick: () => void;
}) {
  return (
    <button
      className={`${className}${active ? " active" : ""}`.trim()}
      aria-current={active ? "page" : undefined}
      onClick={onClick}
    >
      {label}
      {Boolean(count) && <span>{count}</span>}
    </button>
  );
}

