import { useEffect, useState } from "react";
import type { PreviewItem } from "../../api";
import { PreviewRow } from "./PreviewRow";

export function PreviewVirtualList({ items }: { items: PreviewItem[] }) {
  const [compact, setCompact] = useState(
    typeof window !== "undefined" ? window.matchMedia("(max-width: 700px)").matches : false,
  );
  const rowHeight = compact ? 204 : 108;
  const overscan = 10;
  const viewport = 620;
  const listHeight = Math.min(items.length * rowHeight, viewport);
  const [scrollTop, setScrollTop] = useState(0);
  useEffect(() => {
    const query = window.matchMedia("(max-width: 700px)");
    const update = () => setCompact(query.matches);
    update();
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  }, []);
  const start = Math.max(0, Math.floor(scrollTop / rowHeight) - overscan);
  const count = Math.ceil(viewport / rowHeight) + overscan * 2;
  const visible = items.slice(start, start + count);
  return (
    <>
      <div className="compact-preview-head">
        <span>Current metadata</span>
        <span>Proposed metadata</span>
      </div>
      <div
        className="virtual-preview"
        style={{ height: listHeight || rowHeight }}
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
      >
        <div style={{ height: items.length * rowHeight, position: "relative" }}>
          {visible.map((item, index) => (
            <div
              className="virtual-row"
              style={{
                height: rowHeight,
                transform: `translateY(${(start + index) * rowHeight}px)`,
              }}
              key={item.track_id}
            >
              <PreviewRow item={item} />
            </div>
          ))}
        </div>
      </div>
      <div className="preview-count">
        {items.length} matched preview rows · rendering {visible.length} visible rows
      </div>
    </>
  );
}
