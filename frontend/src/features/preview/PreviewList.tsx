import type { PreviewItem } from "../../api";
import { PreviewRow } from "./PreviewRow";

export function PreviewList({ items }: { items: PreviewItem[] }) {
  return (
    <>
      <div className="compact-preview-head">
        <span>Current metadata</span>
        <span>Proposed metadata</span>
      </div>
      <div className="preview-list">
        {items.map((item) => (
          <PreviewRow item={item} key={item.track_id} />
        ))}
      </div>
    </>
  );
}
