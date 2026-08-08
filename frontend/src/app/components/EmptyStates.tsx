import { Icon, type IconName } from "../Icons";

export function EmptyState({
  icon,
  title,
  description,
  action,
  onAction,
}: {
  icon: IconName;
  title: string;
  description: string;
  action?: string;
  onAction?: () => void;
}) {
  return (
    <div className="empty-state">
      <span>
        <Icon name={icon} size={28} />
      </span>
      <h2>{title}</h2>
      <p>{description}</p>
      {action && (
        <button className="compact-button" onClick={onAction}>
          <Icon name="refresh" size={15} />
          {action}
        </button>
      )}
    </div>
  );
}

export function QueueSkeleton() {
  return (
    <div className="skeleton-list" aria-label="Loading tracks">
      {[1, 2, 3, 4, 5].map((item) => (
        <div key={item}>
          <i />
          <span>
            <b />
            <small />
          </span>
          <em />
        </div>
      ))}
    </div>
  );
}

