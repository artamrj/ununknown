import type { MetadataSummary } from "@/api";
import { CoverImage } from "./CoverImage";

export function MusicMetadataCard({
  filename,
  data = {},
  cover,
  changedFrom,
}: {
  filename: string;
  data?: MetadataSummary;
  cover?: string;
  changedFrom?: MetadataSummary;
}) {
  const field = (key: keyof MetadataSummary, name: string) => {
    const changed = changedFrom && String(changedFrom[key] || "") !== String(data[key] || "");
    return (
      <span className={changed ? "changed" : ""}>
        <b>{name}</b>
        {String(data[key] ?? "—")}
      </span>
    );
  };
  return (
    <section className="metadata-card">
      <CoverImage src={cover} title={data.title} />
      <div className="music-meta">
        <h3 className={changedFrom && changedFrom.title !== data.title ? "changed" : ""}>
          {data.title || "Unknown title"}
        </h3>
        <strong className={changedFrom && changedFrom.artist !== data.artist ? "changed" : ""}>
          {data.artist || "Unknown artist"}
        </strong>
        <div className="meta-grid">
          <span>
            <b>File</b>
            {filename}
          </span>
          {field("album", "Album")}
          {field("year", "Date")}
        </div>
      </div>
    </section>
  );
}
