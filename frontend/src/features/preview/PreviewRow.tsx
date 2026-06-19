import type { PreviewItem } from "../../api";
import { MusicMetadataCard } from "./MusicMetadataCard";

export function PreviewRow({ item }: { item: PreviewItem }) {
  const oldData = item.old || {};
  const newData = item.new || {};
  const warnings = item.warnings || [];
  const skip = item.duplicate_action === "skip_duplicate";
  return (
    <article className={`preview-row ${skip ? "duplicate-skip" : ""}`}>
      <MusicMetadataCard filename={item.filename} data={oldData} cover={item.current_cover_url} />
      <div className="change-arrow">
        <span>{skip ? "skip" : "→"}</span>
      </div>
      <MusicMetadataCard
        filename={item.filename}
        data={newData}
        cover={item.proposed_cover_url || item.cover_url}
        changedFrom={oldData}
      />
      {warnings.length > 0 && (
        <footer>
          {warnings.map((warning) => (
            <small key={warning}>{warning}</small>
          ))}
        </footer>
      )}
    </article>
  );
}
