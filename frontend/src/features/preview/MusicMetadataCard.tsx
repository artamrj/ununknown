import type { MetadataSummary } from "../../api";
import { CoverImage } from "./CoverImage";

export function MusicMetadataCard({
  label,
  filename,
  data = {},
  cover,
  confidence,
  path,
  changedFrom,
}: {
  label: string;
  filename: string;
  data?: MetadataSummary;
  cover?: string;
  confidence?: number;
  path?: string;
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
    <section className="music-card">
      <CoverImage src={cover} title={data.title} />
      <div className="music-meta">
        <em>
          {label}
          {confidence !== undefined ? ` · ${Math.round(confidence)}%` : ""}
        </em>
        <h3>{data.title || "Unknown title"}</h3>
        <strong>{data.artist || "Unknown artist"}</strong>
        <div className="meta-grid">
          <span>
            <b>File</b>
            {filename}
          </span>
          {field("album", "Album")}
          {field("album_artist", "Album artist")}
          {field("year", "Date")}
        </div>
        {path && (
          <p className="path-line">
            <span>Output</span>
            <b>{path}</b>
          </p>
        )}
      </div>
    </section>
  );
}
