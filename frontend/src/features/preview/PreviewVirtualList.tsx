import { useState } from "react";
import type { PreviewItem } from "../../api";
import { PreviewRow } from "./PreviewRow";

export function PreviewVirtualList({ items }: { items: PreviewItem[] }) {
  const rowHeight = 74;
  const overscan = 10;
  const viewport = 620;
  const [scrollTop, setScrollTop] = useState(0);
  const start = Math.max(0, Math.floor(scrollTop / rowHeight) - overscan);
  const count = Math.ceil(viewport / rowHeight) + overscan * 2;
  const visible = items.slice(start, start + count);
  return (
    <>
      <div className="compact-preview-head">
        <span>Current metadata</span>
        <span>Proposed metadata</span>
        <span>Output</span>
      </div>
      <div
        className="virtual-preview"
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
